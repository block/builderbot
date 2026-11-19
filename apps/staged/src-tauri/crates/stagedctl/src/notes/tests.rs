use std::cell::RefCell;
use std::collections::HashMap;

use super::*;

// -----------------------------------------------------------------------
// A scripted app
// -----------------------------------------------------------------------

/// Answers each command from a table and records every call, so a test can
/// say what the app was asked and in what order.
#[derive(Default)]
struct App {
    answers: HashMap<&'static str, Result<Value, (&'static str, &'static str)>>,
    calls: RefCell<Vec<(String, Value)>>,
}

impl App {
    /// An app whose caller reaches "Source" (p1) and "Target" (p2).
    fn with_projects() -> Self {
        Self::default().answer(
            "list_projects",
            json!([
                { "id": "p1", "name": "Source", "location": "local" },
                { "id": "p2", "name": "Target", "location": "local" },
            ]),
        )
    }

    fn answer(mut self, command: &'static str, value: Value) -> Self {
        self.answers.insert(command, Ok(value));
        self
    }

    /// `command` fails the way the app's own refusal does (HTTP 400).
    fn refuse(mut self, command: &'static str, message: &'static str) -> Self {
        self.answers.insert(command, Err(("app_error", message)));
        self
    }

    /// `command` is refused by scope (HTTP 403, exit 4).
    fn deny(mut self, command: &'static str, message: &'static str) -> Self {
        self.answers.insert(command, Err(("scope_denied", message)));
        self
    }

    fn invoke(&self) -> impl FnMut(&str, Value) -> Result<Value, CtlError> + '_ {
        move |command, args| {
            self.calls.borrow_mut().push((command.to_string(), args));
            match self.answers.get(command) {
                Some(Ok(value)) => Ok(value.clone()),
                Some(Err(("scope_denied", message))) => Err(CtlError::Authorization {
                    code: "scope_denied".to_string(),
                    message: message.to_string(),
                }),
                Some(Err((code, message))) => Err(CtlError::command(code, *message)),
                None => Err(CtlError::Transport(format!("{command}: connection reset"))),
            }
        }
    }

    fn commands(&self) -> Vec<String> {
        self.calls.borrow().iter().map(|(c, _)| c.clone()).collect()
    }

    fn args_of(&self, command: &str) -> Value {
        self.calls
            .borrow()
            .iter()
            .find(|(c, _)| c == command)
            .map(|(_, args)| args.clone())
            .unwrap_or_else(|| panic!("{command} was not called"))
    }
}

fn branch_note() -> NoteRef {
    NoteRef::Branch("n1".to_string())
}

fn project_note() -> NoteRef {
    NoteRef::Project("pn1".to_string())
}

fn note_row(id: &str, title: &str) -> Value {
    json!({ "id": id, "title": title, "content": "body", "sessionId": null })
}

fn branch_row() -> Value {
    json!({
        "id": "b9",
        "projectId": "p2",
        "projectRepoId": "r9",
        "branchName": "max/target",
        "baseBranch": "origin/main",
        "worktreePath": null,
    })
}

fn repo_row() -> Value {
    json!({
        "id": "r9",
        "projectId": "p2",
        "githubRepo": "acme/widgets",
        "subpath": "apps/web",
        "branchName": "max/target",
        "isPrimary": true,
    })
}

/// The app's answer for a branch note that landed on `max/target`.
fn landed_on_branch(note: Value, added: Value) -> Value {
    json!({ "note": note, "branch": branch_row(), "repo": repo_row(), "addedRepos": added })
}

fn message(err: &CtlError) -> String {
    err.to_string()
}

// -----------------------------------------------------------------------
// The <NOTE> argument
// -----------------------------------------------------------------------

#[test]
fn a_note_is_named_by_the_hashtag_reference_the_cli_prints() {
    for (input, expected) in [
        ("#note:n1", branch_note()),
        ("#project-note:pn1", project_note()),
        // Unquoted, a shell drops everything from the #, so it may be left
        // off.
        ("note:n1", branch_note()),
        ("project-note:pn1", project_note()),
        ("  #note:n1\n", branch_note()),
    ] {
        assert_eq!(NoteRef::parse(input).unwrap(), expected, "{input:?}");
    }
    // What is printed back is the form that was read.
    assert_eq!(branch_note().reference_to("n1"), "#note:n1");
    assert_eq!(project_note().reference_to("pn1"), "#project-note:pn1");
}

#[test]
fn anything_but_a_note_reference_is_refused() {
    for input in [
        "",
        "#",
        // A bare id does not say which kind of note it is.
        "n1",
        "#note:",
        "#project-note:",
        "#note: n1",
        "#Note:n1",
        "#image:i1",
        "##note:n1",
    ] {
        let err = NoteRef::parse(input).unwrap_err();
        assert_eq!(err.exit_code(), 1, "{input:?}");
        assert_eq!(err.code(), "invalid_arguments", "{input:?}");
        let text = message(&err);
        assert!(
            text.contains("'#note:<id>' or '#project-note:<id>'"),
            "{text}"
        );
        assert!(text.contains(&format!("got {input:?}")), "{text}");
        assert!(text.contains("stagedctl timeline show"), "{text}");
    }
    // The other things a timeline prints are references too, and do not
    // move.
    for input in ["#review:r1", "#commit:abc123"] {
        let err = NoteRef::parse(input).unwrap_err();
        assert_eq!(err.code(), "invalid_arguments", "{input:?}");
        assert!(
            message(&err).contains("commits and reviews stay on their branch"),
            "{err}"
        );
    }
}

// -----------------------------------------------------------------------
// The request
// -----------------------------------------------------------------------

#[test]
fn each_kind_and_verb_sends_its_own_command_with_the_destinations_id() {
    for (transfer, note, command, args) in [
        (
            Transfer::Move,
            branch_note(),
            "move_note_to_project",
            json!({ "noteId": "n1", "targetProjectId": "p2" }),
        ),
        (
            Transfer::Copy,
            branch_note(),
            "copy_note_to_project",
            json!({ "noteId": "n1", "targetProjectId": "p2" }),
        ),
        (
            Transfer::Move,
            project_note(),
            "move_project_note_to_project",
            json!({ "projectNoteId": "pn1", "targetProjectId": "p2" }),
        ),
        (
            Transfer::Copy,
            project_note(),
            "copy_project_note_to_project",
            json!({ "projectNoteId": "pn1", "targetProjectId": "p2" }),
        ),
    ] {
        // The destination by name, by id, and by a name in another case.
        for to in ["Target", "p2", "target"] {
            let app = App::with_projects().answer(
                command,
                json!({ "note": note_row("x", "Findings"), "addedRepos": [] }),
            );
            transfer_with(app.invoke(), transfer, &note, to).unwrap();
            // One read to find the destination, then the one write. Nothing
            // is read afterwards: the answer says where the note landed.
            assert_eq!(app.commands(), vec!["list_projects", command], "{to}");
            assert_eq!(app.args_of(command), args, "{command} --to {to}");
        }
    }
}

#[test]
fn a_copy_may_name_the_notes_own_project() {
    // The CLI does not know which project the note is in, and a copy into
    // it is a duplicate, which the app allows.
    let app = App::with_projects().answer(
        "copy_note_to_project",
        landed_on_branch(note_row("n9", "Findings (2)"), json!([])),
    );
    transfer_with(app.invoke(), Transfer::Copy, &branch_note(), "Source").unwrap();
    assert_eq!(
        app.args_of("copy_note_to_project"),
        json!({ "noteId": "n1", "targetProjectId": "p1" })
    );
}

// -----------------------------------------------------------------------
// The output
// -----------------------------------------------------------------------

#[test]
fn a_moved_branch_note_prints_where_it_landed() {
    let answer = landed_on_branch(note_row("n1", "Findings"), json!([]));
    let app = App::with_projects().answer("move_note_to_project", answer.clone());
    let out = transfer_with(app.invoke(), Transfer::Move, &branch_note(), "Target").unwrap();

    assert_eq!(
        out.human,
        "note #note:n1 moved to project \"Target\"\n\
         \x20 project id: p2\n\
         \x20 title: Findings\n\
         \x20 landed on: acme/widgets:apps/web, branch max/target\n\
         next: stagedctl timeline show p2 --repo acme/widgets:apps/web --branch max/target"
    );
    assert_eq!(
        out.json,
        json!({
            "ok": true,
            "mode": "move",
            "projectId": "p2",
            "reference": "#note:n1",
            "note": answer["note"],
            "branch": branch_row(),
            "repo": repo_row(),
            "addedRepos": [],
        })
    );
}

#[test]
fn a_transfer_that_added_a_repo_to_the_destination_says_so() {
    let added = json!([{ "githubRepo": "acme/widgets", "subpath": "apps/web" }]);
    for (transfer, command) in [
        (Transfer::Move, "move_note_to_project"),
        (Transfer::Copy, "copy_note_to_project"),
    ] {
        let app = App::with_projects().answer(
            command,
            landed_on_branch(note_row("n1", "Findings"), added.clone()),
        );
        let out = transfer_with(app.invoke(), transfer, &branch_note(), "Target").unwrap();
        assert!(
            out.human.contains(
                "\n  added repo: acme/widgets:apps/web (the project did not track it; its \
                 branch is set up in the background)\n"
            ),
            "{}",
            out.human
        );
        assert_eq!(out.json["addedRepos"], added);
    }
}

#[test]
fn a_copy_prints_the_new_notes_reference() {
    // The copy has its own id, and the title the app gave it on a branch
    // that already had the original's.
    let answer = landed_on_branch(note_row("n9", "Findings (2)"), json!([]));
    let app = App::with_projects().answer("copy_note_to_project", answer.clone());
    let out = transfer_with(app.invoke(), Transfer::Copy, &branch_note(), "Target").unwrap();

    assert_eq!(
        out.human,
        "note #note:n1 copied to project \"Target\" as #note:n9\n\
         \x20 project id: p2\n\
         \x20 title: Findings (2)\n\
         \x20 landed on: acme/widgets:apps/web, branch max/target\n\
         next: stagedctl timeline show p2 --repo acme/widgets:apps/web --branch max/target"
    );
    assert_eq!(out.json["mode"], "copy");
    assert_eq!(out.json["reference"], "#note:n9");
    assert_eq!(out.json["note"]["id"], "n9");
}

#[test]
fn a_project_note_lands_in_the_project_and_lists_its_children_and_their_repos() {
    let answer = json!({
        "note": { "id": "pn1", "projectId": "p2", "title": "Plan", "content": "body" },
        "children": [
            {
                "reference": "#note:c1",
                "title": "Web",
                "branch": branch_row(),
                "repo": repo_row(),
            },
            {
                "reference": "#note:c2",
                "title": "",
                "branch": {
                    "id": "b10",
                    "projectId": "p2",
                    "projectRepoId": "r10",
                    "branchName": "max/gadgets",
                    "baseBranch": "origin/main",
                    "worktreePath": null,
                },
                "repo": {
                    "id": "r10",
                    "projectId": "p2",
                    "githubRepo": "acme/gadgets",
                    "subpath": null,
                    "branchName": "max/gadgets",
                    "isPrimary": false,
                },
            },
        ],
        "addedRepos": [
            { "githubRepo": "acme/widgets", "subpath": "apps/web" },
            { "githubRepo": "acme/gadgets", "subpath": null },
        ],
    });
    let app = App::with_projects().answer("move_project_note_to_project", answer.clone());
    let out = transfer_with(app.invoke(), Transfer::Move, &project_note(), "Target").unwrap();

    assert_eq!(
        out.human,
        "project note #project-note:pn1 moved to project \"Target\"\n\
         \x20 project id: p2\n\
         \x20 title: Plan\n\
         \x20 child note #note:c1 \"Web\" landed on: acme/widgets:apps/web, branch max/target\n\
         \x20 child note #note:c2 landed on: acme/gadgets, branch max/gadgets\n\
         \x20 added repo: acme/widgets:apps/web (the project did not track it; its branch is \
         set up in the background)\n\
         \x20 added repo: acme/gadgets (the project did not track it; its branch is set up in \
         the background)\n\
         next: stagedctl timeline show p2"
    );
    // A project note is on no branch, so the app answers neither row and
    // none is invented. Its children are the app's rows as they came.
    assert_eq!(
        out.json,
        json!({
            "ok": true,
            "mode": "move",
            "projectId": "p2",
            "reference": "#project-note:pn1",
            "note": answer["note"],
            "children": answer["children"],
            "addedRepos": answer["addedRepos"],
        })
    );

    let copy = json!({
        "note": { "id": "pn9", "projectId": "p2", "title": "Plan", "content": "body" },
        "children": [],
        "addedRepos": [],
    });
    let app = App::with_projects().answer("copy_project_note_to_project", copy);
    let out = transfer_with(app.invoke(), Transfer::Copy, &project_note(), "Target").unwrap();
    assert_eq!(
        out.human,
        "project note #project-note:pn1 copied to project \"Target\" as #project-note:pn9\n\
         \x20 project id: p2\n\
         \x20 title: Plan\n\
         next: stagedctl timeline show p2"
    );
    assert_eq!(out.json["children"], json!([]));
}

#[test]
fn a_child_whose_landing_rows_could_not_be_read_back_is_still_listed() {
    // As for the note itself: the child moved, and the output does not
    // guess where a row the app could not read would have put it.
    let answer = json!({
        "note": { "id": "pn1", "projectId": "p2", "title": "Plan" },
        "children": [
            { "reference": "#note:c1", "title": "Web", "branch": null, "repo": null },
            { "reference": "#note:c2", "title": "Api", "branch": branch_row(), "repo": null },
        ],
        "addedRepos": [],
    });
    let app = App::with_projects().answer("move_project_note_to_project", answer);
    let out = transfer_with(app.invoke(), Transfer::Move, &project_note(), "Target").unwrap();
    assert_eq!(
        out.human,
        "project note #project-note:pn1 moved to project \"Target\"\n\
         \x20 project id: p2\n\
         \x20 title: Plan\n\
         \x20 child note #note:c1 \"Web\"\n\
         \x20 child note #note:c2 \"Api\" landed on: branch max/target\n\
         next: stagedctl timeline show p2"
    );
}

#[test]
fn an_answer_without_children_lists_none() {
    // A branch note's answer has no `children`, and none is added to it.
    let answer = landed_on_branch(note_row("n1", "Findings"), json!([]));
    let app = App::with_projects().answer("move_note_to_project", answer);
    let out = transfer_with(app.invoke(), Transfer::Move, &branch_note(), "Target").unwrap();
    assert!(!out.human.contains("child note"), "{}", out.human);
    assert_eq!(out.json.get("children"), None);
}

#[test]
fn a_landing_row_the_app_could_not_read_back_is_left_out() {
    // The app answers `null` for a row it could not read after the transfer
    // committed. The note still landed, and the output does not guess.
    for (branch, repo, line) in [
        (Value::Null, Value::Null, None),
        (
            branch_row(),
            Value::Null,
            Some("  landed on: branch max/target"),
        ),
    ] {
        let app = App::with_projects().answer(
            "move_note_to_project",
            json!({
                "note": note_row("n1", ""),
                "branch": branch,
                "repo": repo,
                "addedRepos": [],
            }),
        );
        let out = transfer_with(app.invoke(), Transfer::Move, &branch_note(), "Target").unwrap();
        let mut expected = vec![
            "note #note:n1 moved to project \"Target\"",
            "  project id: p2",
        ];
        expected.extend(line);
        expected.push("next: stagedctl timeline show p2");
        // An untitled note prints no title line either.
        assert_eq!(out.human, expected.join("\n"));
        assert_eq!(out.json["ok"], true);
    }
}

// -----------------------------------------------------------------------
// Failures
// -----------------------------------------------------------------------

#[test]
fn a_destination_the_credential_does_not_list_is_not_found_and_nothing_is_written() {
    // A session's project list holds its own project and the ones it
    // created. Any other project is unknown to it, by name and by id.
    for to in ["Elsewhere", "p3"] {
        let app = App::with_projects();
        let err = transfer_with(app.invoke(), Transfer::Move, &branch_note(), to).unwrap_err();
        assert_eq!(err.exit_code(), 1, "{to}");
        assert_eq!(err.code(), "not_found", "{to}");
        assert_eq!(app.commands(), vec!["list_projects"], "{to}");
    }

    let app = App::default().answer(
        "list_projects",
        json!([{ "id": "p1", "name": "Target" }, { "id": "p2", "name": "TARGET" }]),
    );
    let err = transfer_with(app.invoke(), Transfer::Copy, &project_note(), "target").unwrap_err();
    assert_eq!(err.code(), "ambiguous");
    assert_eq!(app.commands(), vec!["list_projects"]);
}

#[test]
fn the_transfers_refusals_pass_through_as_command_failures() {
    for (transfer, note, command, refusal) in [
        (
            Transfer::Move,
            branch_note(),
            "move_note_to_project",
            "\"Findings\" is already in Target.",
        ),
        (
            Transfer::Move,
            branch_note(),
            "move_note_to_project",
            "\"Findings\"'s session is running again; wait for it to finish before moving \
             the note.",
        ),
        (
            Transfer::Copy,
            branch_note(),
            "copy_note_to_project",
            "\"Findings\" is still being generated; wait for its session to finish before \
             copying it.",
        ),
        (
            Transfer::Copy,
            branch_note(),
            "copy_note_to_project",
            "Branch 'scratch' is not bound to a repository, so its notes cannot be copied to \
             another project.",
        ),
        (
            Transfer::Move,
            project_note(),
            "move_project_note_to_project",
            "Project 'Target' runs on a remote workspace; \"Plan\" has child notes, which can \
             only move between local projects.",
        ),
        (
            Transfer::Move,
            project_note(),
            "move_project_note_to_project",
            "Could not add acme/gadgets to Target: clone failed. acme/widgets at apps/web was \
             added to Target along the way and remains tracked there.",
        ),
        (
            Transfer::Copy,
            project_note(),
            "copy_project_note_to_project",
            "Project note not found: pn1",
        ),
    ] {
        let app = App::with_projects().refuse(command, refusal);
        let err = transfer_with(app.invoke(), transfer, &note, "Target").unwrap_err();
        assert_eq!(err.exit_code(), 1, "{refusal}");
        assert_eq!(message(&err), format!("app_error: {refusal}"));
        // Nothing landed, so nothing is reported as applied.
        assert_eq!(err.partial_output(false), None);
        assert_eq!(err.partial_output(true), None);
    }
}

#[test]
fn a_scope_refusal_keeps_its_code_and_exit_4() {
    for refusal in [
        // The note is in a project the credential does not reach.
        "the note belongs to another project; this credential covers project p1 and the \
         projects it created",
        // The destination is one, which the project list hid from this
        // caller but not from the app.
        "the destination is a project outside this credential's reach; this credential \
         covers project p1 and the projects it created",
        "move_note_to_project changes the app and this credential is read-only",
    ] {
        for (note, command) in [
            (branch_note(), "move_note_to_project"),
            (project_note(), "move_project_note_to_project"),
        ] {
            let app = App::with_projects().deny(command, refusal);
            let err = transfer_with(app.invoke(), Transfer::Move, &note, "Target").unwrap_err();
            assert_eq!(err.exit_code(), 4, "{refusal}");
            assert_eq!(message(&err), format!("scope_denied: {refusal}"));
            assert_eq!(err.partial_output(false), None);
        }
    }
}

#[test]
fn a_failed_project_list_stops_before_the_write() {
    let app = App::default();
    let err = transfer_with(app.invoke(), Transfer::Move, &branch_note(), "Target").unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert_eq!(app.commands(), vec!["list_projects"]);
}

#[test]
fn a_transfer_whose_answer_cannot_be_read_is_still_reported_as_landed() {
    // The app answered, so the transfer ran. The caller is told, with the
    // read's own error and exit code, and with what the app said.
    let app = App::with_projects().answer("copy_note_to_project", json!({ "unexpected": true }));
    let err = transfer_with(app.invoke(), Transfer::Copy, &branch_note(), "Target").unwrap_err();

    assert_eq!(err.exit_code(), 2);
    assert!(
        message(&err).starts_with("transport: unexpected note transfer answer shape"),
        "{err}"
    );
    assert_eq!(
        err.partial_output(false).unwrap(),
        "note #note:n1 copied to project \"Target\"\n\
         \x20 (copied; the app's answer could not be read)\n\
         next: stagedctl timeline show p2"
    );
    let partial: Value = serde_json::from_str(&err.partial_output(true).unwrap()).unwrap();
    assert_eq!(partial["ok"], false);
    assert_eq!(
        partial["applied"],
        json!([{
            "command": "copy_note_to_project",
            "args": { "noteId": "n1", "targetProjectId": "p2" },
            "result": { "unexpected": true },
        }])
    );
}

// -----------------------------------------------------------------------
// What the help promises, held to the app
// -----------------------------------------------------------------------

#[test]
fn the_commands_are_allowlisted_by_the_control_server() {
    let source = include_str!("../../../../src/control_server.rs");
    let start = source.find("pub const ALLOWED_COMMANDS").unwrap();
    let end = start + source[start..].find("];").unwrap();
    let listed: Vec<&str> = source[start..end]
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"'))
        .filter_map(|rest| rest.split('"').next())
        .collect();
    for command in NOTE_TRANSFER_COMMANDS {
        assert!(listed.contains(command), "{command} is not allowlisted");
        assert!(is_note_transfer_command(command));
    }
    // The read both verbs make first.
    assert!(listed.contains(&"list_projects"));
    assert!(!is_note_transfer_command("list_projects"));

    // Every kind and verb goes out as one of them.
    for note in [branch_note(), project_note()] {
        for transfer in [Transfer::Move, Transfer::Copy] {
            assert!(is_note_transfer_command(note.command(transfer)));
        }
    }
}

#[test]
fn the_requests_use_the_argument_names_the_apps_commands_read() {
    // `dispatch` hands each pair of commands to a body in
    // `note_transfer_arms`, which reads the arguments.
    let dispatch = include_str!("../../../../src/web_server.rs");
    let bodies = include_str!("../../../../src/web_server/note_transfer_arms.rs");
    for (arm, body, id_arg) in [
        (
            "\"move_note_to_project\" | \"copy_note_to_project\" => {",
            "branch_note",
            "\"noteId\"",
        ),
        (
            "\"move_project_note_to_project\" | \"copy_project_note_to_project\" => {",
            "project_note",
            "\"projectNoteId\"",
        ),
    ] {
        let start = dispatch.find(arm).unwrap_or_else(|| panic!("no arm {arm}"));
        let call = format!("note_transfer_arms::{body}(&store, command, &args,");
        assert!(
            dispatch[start..start + 300].contains(&call),
            "{arm} no longer runs {body}"
        );
        let start = bodies
            .find(&format!("pub(super) async fn {body}<"))
            .unwrap_or_else(|| panic!("no body {body}"));
        let code = &bodies[start..start + 700];
        assert!(code.contains(id_arg), "{body} no longer reads {id_arg}");
        assert!(code.contains("\"targetProjectId\""), "{body}");
    }
}

/// What the output reads from the app's answer is what the app puts there.
#[test]
fn the_answer_is_read_under_the_names_the_app_writes() {
    let control = include_str!("../../../../src/note_transfer/control.rs");
    for written in [
        "\"note\": note,",
        "\"branch\": branch,",
        "\"repo\": repo,",
        "\"children\": children,",
        "\"addedRepos\": added.into_json(),",
        "\"reference\": format!(\"#note:{}\", child.id),",
        "\"title\": child.title,",
        "json!({ \"githubRepo\": key.github_repo, \"subpath\": key.subpath })",
    ] {
        assert!(
            control.contains(written),
            "the app's answer no longer has {written}; read `Landed` again"
        );
    }
}

/// The help of one verb, as `--help` prints it.
fn help_of(verb: &str) -> String {
    let help = crate::cli::render_all_help();
    help.split(&format!("=== stagedctl note {verb} --help ==="))
        .nth(1)
        .and_then(|rest| rest.split("\n=== ").next())
        .unwrap_or_else(|| panic!("note {verb} has help"))
        .to_string()
}

#[test]
fn help_states_the_transfers_rules_as_the_app_has_them() {
    let move_help = help_of("move");
    let copy_help = help_of("copy");
    let transfer = include_str!("../../../../src/note_transfer.rs");

    // The refusals the help lists are the ones the transfer makes. If one
    // of these is reworded or removed there, read the help again.
    for (refusal, verbs, promise) in [
        (
            "\"{} is already in {}.\"",
            &[&move_help][..],
            "already in the destination project",
        ),
        (
            "is still being generated; wait for its session to finish before",
            &[&move_help][..],
            "still being written",
        ),
        (
            "is still being generated; wait for its session to finish before",
            &[&copy_help][..],
            "never finished being written",
        ),
        (
            "'s session is {} again; wait for it to finish before",
            &[&move_help][..],
            "queued or running",
        ),
        (
            "is not bound to a repository",
            &[&move_help, &copy_help][..],
            "tracks no repo",
        ),
        (
            "runs on a remote workspace",
            &[&move_help][..],
            "either project runs on a remote\n    workspace",
        ),
        (
            "notes on remote branches can't move between projects",
            &[&move_help][..],
            "on a remote branch",
        ),
        (
            "which is in a different project",
            &[&move_help][..],
            "outside the project note's own project",
        ),
    ] {
        assert!(
            transfer.contains(refusal),
            "the transfer no longer says {refusal:?}; update `note move`/`note copy` --help"
        );
        for help in verbs {
            assert!(help.contains(promise), "help lacks {promise:?}:\n{help}");
        }
    }
    // A copy is refused only for a note that never finished: the second
    // check is skipped for it.
    assert!(transfer.contains("if mode == TransferMode::Copy {\n        return Ok(());"));
    assert!(
        copy_help.contains("is running again can be copied"),
        "{copy_help}"
    );

    // What a copy is: a standalone note of the user's own.
    assert!(transfer.contains(".with_subtype(Note::SUBTYPE_WRITTEN)"));
    for promise in [
        "new note with its own id",
        "no session",
        "editable in the app like a note the user wrote",
        "no link to a parent project note and no child notes",
        "no suggested next steps",
        "never adds a repo",
    ] {
        assert!(copy_help.contains(promise), "copy help lacks {promise:?}");
    }

    // What a move carries.
    for promise in [
        "The note keeps\nits id",
        "Its session\n                 comes with it",
        "takes its child notes along",
        "Commits and code reviews are not part of a note",
        "moving it back does not restore that link",
    ] {
        assert!(move_help.contains(promise), "move help lacks {promise:?}");
    }

    // The title a note gets on a branch that has one by that name.
    let notes = include_str!("../../../../src/store/notes.rs");
    assert!(notes.contains("Ok(format!(\"{base} ({})\", max_n + 1))"));
    for help in [&move_help, &copy_help] {
        assert!(help.contains("\"<title> (2)\""), "{help}");
    }

    // The repo is added with nothing but its name and subpath, which is
    // `project add-repo` without --branch.
    assert!(transfer.contains("github_repo: key.github_repo,\n            subpath: key.subpath,\n            ..Default::default()"));
    assert!(
        move_help.contains("the way `stagedctl project add-repo` adds one without --branch"),
        "{move_help}"
    );

    // Both verbs are writes on two projects.
    for help in [&move_help, &copy_help] {
        assert!(
            help.contains("Needs write access to both projects"),
            "{help}"
        );
        assert!(help.contains("`scope_denied`"), "{help}");
        assert!(help.contains("`not_found`"), "{help}");
        assert!(help.contains("stagedctl has no delete"), "{help}");
    }

    // The scope refusals beyond the two projects, as the app's scope check
    // makes them. A confined credential is refused a note it cannot be
    // shown in the words of a note that is not there, with exit 4.
    let scope = include_str!("../../../../src/web_server/control_scope.rs");
    for refusal in [
        "denied(format!(\"note {note_id} not found\"))",
        "denied(format!(\"project note {note_id} not found\"))",
        "embeds media {media_id}, which belongs to another project",
        "its child note #note:{}",
    ] {
        assert!(
            scope.contains(refusal),
            "the scope check no longer says {refusal:?}; update `note move`/`note copy` --help"
        );
    }
    for help in [&move_help, &copy_help] {
        for promise in [
            "A credential confined to some",
            "a <NOTE> that names no note of that kind",
            "says \"not found\"",
            "`app_error`",
            "embeds an image or video from a project it does\n    not reach",
        ] {
            assert!(help.contains(promise), "help lacks {promise:?}:\n{help}");
        }
    }
    // A moved project note's children travel, so their media counts. A
    // copy leaves them, so theirs does not.
    assert!(
        scope.contains("with_children: true"),
        "the move's scope row"
    );
    assert!(
        move_help.contains("or a project note with such a child note"),
        "{move_help}"
    );
    assert!(
        copy_help.contains("A project note's child notes are not copied"),
        "{copy_help}"
    );

    // A repo the transfer added stays when it then fails, and the error
    // names it, for a branch note as for a project note.
    assert!(transfer.contains("along the way and {remain} tracked there."));
    assert!(
        move_help.contains("along the way and remains tracked there."),
        "{move_help}"
    );
    assert!(
        copy_help.contains(
            "The repo stays in the project when the copy then fails, and\nthe error names it"
        ),
        "{copy_help}"
    );
    for help in [&move_help, &copy_help] {
        assert!(
            !help.contains("stagedctl project get"),
            "the error names the repo; nothing else has to be looked up: {help}"
        );
    }

    // The output of a project note's move lists its children.
    assert!(
        move_help.contains("The output lists each child by its\n                 reference"),
        "{move_help}"
    );
}

#[test]
fn the_guide_describes_both_verbs() {
    // The guide ships with the app and is named in every session's prompt.
    let guide = include_str!("../../../../resources/stagedctl/guide.md");
    // One paragraph per line of thought, whatever the wrapping.
    let flat = guide.split_whitespace().collect::<Vec<_>>().join(" ");
    for expected in [
        "`stagedctl note move`",
        "`stagedctl note copy`",
        "'#note:<id>'",
        "'#project-note:<id>'",
        "an unquoted `#` starts a shell comment",
        "both projects",
        "the user can move the note back or delete the copy",
        "The output lists each child by its reference",
        "the error names it",
        "Exit 4 (`scope_denied`)",
        "A reference that names no note of its kind also exits 4",
        "a message that says \"not found\"",
        "embeds an image or video from such a project",
        "refused for media a child note embeds too",
    ] {
        assert!(flat.contains(expected), "the guide lacks {expected:?}");
    }
}
