use std::cell::RefCell;
use std::collections::HashMap;

use clap::Parser;

use super::*;
use crate::cli::{Cli, Noun, ProjectVerb};

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

fn project_row(id: &str, name: &str, location: &str) -> Value {
    json!({ "id": id, "name": name, "location": location, "createdBy": "session:s-1" })
}

fn repo_row(id: &str, repo: &str, subpath: Option<&str>, branch: &str, primary: bool) -> Value {
    json!({
        "id": id,
        "projectId": "p1",
        "githubRepo": repo,
        "subpath": subpath,
        "branchName": branch,
        "isPrimary": primary,
        "reason": null,
        "headRepo": null,
    })
}

fn local_branch(id: &str, repo_id: &str, name: &str) -> Value {
    json!({
        "id": id,
        "projectId": "p1",
        "projectRepoId": repo_id,
        "branchName": name,
        "baseBranch": "origin/main",
    })
}

fn remote_branch(id: &str, repo_id: &str, name: &str, workspace: &str) -> Value {
    json!({
        "id": id,
        "projectId": "p1",
        "projectRepoId": repo_id,
        "branchName": name,
        "baseBranch": "origin/main",
        "workspaceName": workspace,
        "workspaceStatus": "starting",
    })
}

fn repo_flags(repo: &str) -> RepoRequest {
    RepoRequest::from_flags(repo, None, None, None, false).unwrap()
}

fn project(id: &str, name: &str) -> Project {
    serde_json::from_value(project_row(id, name, "local")).unwrap()
}

fn attached(rows: Vec<Value>) -> Vec<RepoRow> {
    serde_json::from_value(Value::Array(rows)).unwrap()
}

fn message(err: &CtlError) -> String {
    err.to_string()
}

// -----------------------------------------------------------------------
// Flag pairing
// -----------------------------------------------------------------------

#[test]
fn create_without_a_repo_takes_no_repo_flags() {
    assert_eq!(
        RepoRequest::for_create(None, None, None, None, false).unwrap(),
        None
    );
    for (flag, result) in [
        (
            "--branch",
            RepoRequest::for_create(None, Some("fix"), None, None, false),
        ),
        (
            "--pr",
            RepoRequest::for_create(None, None, Some(42), None, false),
        ),
        (
            "--base",
            RepoRequest::for_create(None, None, None, Some("develop"), false),
        ),
        (
            "--allow-monorepo-root",
            RepoRequest::for_create(None, None, None, None, true),
        ),
    ] {
        let err = result.unwrap_err();
        assert_eq!(err.exit_code(), 1, "{flag}");
        let text = message(&err);
        assert!(
            text.starts_with(&format!("invalid_arguments: {flag} ")) && text.contains("--repo"),
            "{flag}: {text}"
        );
    }
}

#[test]
fn a_pull_request_goes_with_neither_a_branch_nor_a_base() {
    let err =
        RepoRequest::from_flags("acme/widgets", Some("fix"), Some(42), None, false).unwrap_err();
    assert!(
        message(&err).starts_with("invalid_arguments: --branch and --pr cannot be combined"),
        "{err}"
    );
    let err = RepoRequest::from_flags("acme/widgets", None, Some(42), Some("develop"), false)
        .unwrap_err();
    assert!(
        message(&err).starts_with("invalid_arguments: --base and --pr cannot be combined"),
        "{err}"
    );
    // The same rules hold for `project create`, which shares them.
    let err = RepoRequest::for_create(Some("acme/widgets"), Some("fix"), Some(42), None, false)
        .unwrap_err();
    assert!(message(&err).contains("--branch and --pr"), "{err}");

    // Each alone, and a branch with a base, are fine.
    for (branch, pr, base) in [
        (Some("fix"), None, None),
        (None, Some(42), None),
        (None, None, Some("develop")),
        (Some("fix"), None, Some("develop")),
    ] {
        let repo =
            RepoRequest::from_flags("acme/widgets:services/auth", branch, pr, base, true).unwrap();
        assert_eq!(repo.address.github_repo, "acme/widgets");
        assert_eq!(repo.address.subpath.as_deref(), Some("services/auth"));
        assert_eq!(repo.branch.as_deref(), branch);
        assert_eq!(repo.pr, pr);
        assert_eq!(repo.base.as_deref(), base);
        assert!(repo.allow_monorepo_root);
    }
}

#[test]
fn a_blank_branch_or_base_and_a_malformed_repo_are_refused() {
    let err = RepoRequest::from_flags("acme/widgets", Some("  "), None, None, false).unwrap_err();
    assert_eq!(
        message(&err),
        "invalid_arguments: --branch must not be empty"
    );
    let err = RepoRequest::from_flags("acme/widgets", None, None, Some(""), false).unwrap_err();
    assert_eq!(message(&err), "invalid_arguments: --base must not be empty");
    for bad in ["widgets", "", ":services/auth"] {
        let err = RepoRequest::from_flags(bad, None, None, None, false).unwrap_err();
        let text = message(&err);
        assert!(
            text.starts_with("invalid_arguments: --repo must look like"),
            "{bad:?}: {text}"
        );
        // The repo is not attached anywhere yet, so the address has no
        // `project get` to be copied from.
        assert!(!text.contains("project get"), "{text}");
    }
}

// -----------------------------------------------------------------------
// Requests
// -----------------------------------------------------------------------

#[test]
fn a_bare_create_sends_the_name_and_a_local_location_and_nothing_else() {
    // An unset repo flag must be absent, not `false` or `null`: the app
    // refuses `create_project` when a repo argument is given without a repo.
    assert_eq!(
        create_args("  Fix login ", ProjectLocation::Local, None),
        json!({ "name": "Fix login", "location": "local" })
    );
    assert_eq!(
        create_args("Fix login", ProjectLocation::Remote, None),
        json!({ "name": "Fix login", "location": "remote" })
    );
}

#[test]
fn location_defaults_to_local_in_the_request() {
    // From the command line to the wire: no --location means "local" is
    // sent, so the app never reads the location the form remembers.
    let cli = Cli::try_parse_from(["stagedctl", "project", "create", "Fix login"]).unwrap();
    let Noun::Project(ProjectVerb::Create { name, location, .. }) = cli.command else {
        panic!("parsed something else");
    };
    assert_eq!(create_args(&name, location, None)["location"], "local");

    let cli = Cli::try_parse_from([
        "stagedctl",
        "project",
        "create",
        "Fix login",
        "--location",
        "remote",
    ])
    .unwrap();
    let Noun::Project(ProjectVerb::Create { name, location, .. }) = cli.command else {
        panic!("parsed something else");
    };
    assert_eq!(create_args(&name, location, None)["location"], "remote");
}

#[test]
fn repo_flags_map_onto_the_control_surfaces_argument_names() {
    let repo = RepoRequest::from_flags(
        "Acme/Widgets:/services/auth/",
        Some("fix-login"),
        None,
        Some("develop"),
        true,
    )
    .unwrap();
    // Sent as typed: the app canonicalises the name and trims the subpath.
    assert_eq!(
        create_args("Fix login", ProjectLocation::Local, Some(&repo)),
        json!({
            "name": "Fix login",
            "location": "local",
            "githubRepo": "Acme/Widgets",
            "subpath": "/services/auth/",
            "branchName": "fix-login",
            "defaultBranch": "develop",
            "allowMonorepoRoot": true,
        })
    );

    let repo = RepoRequest::from_flags("acme/widgets", None, Some(42), None, false).unwrap();
    assert_eq!(
        add_repo_args("p1", &repo, Some("needs the auth service")),
        json!({
            "projectId": "p1",
            "githubRepo": "acme/widgets",
            "prNumber": 42,
            "reason": "needs the auth service",
        })
    );
    assert_eq!(
        add_repo_args("p1", &repo_flags("acme/widgets"), None),
        json!({ "projectId": "p1", "githubRepo": "acme/widgets" })
    );
}

#[test]
fn requests_use_only_arguments_the_control_surface_takes() {
    // The app's rules for a control caller's arguments. Every key the CLI
    // can send must be one that file reads, and the two it refuses must
    // never be sent.
    let rules = include_str!("../../../../src/web_server/control_project_args.rs");
    let repo = RepoRequest::from_flags(
        "acme/widgets:services/auth",
        Some("fix"),
        None,
        Some("develop"),
        true,
    )
    .unwrap();
    let with_pr = RepoRequest::from_flags("acme/widgets", None, Some(42), None, false).unwrap();
    let requests = [
        create_args("n", ProjectLocation::Remote, Some(&repo)),
        create_args("n", ProjectLocation::Local, Some(&with_pr)),
        add_repo_args("p1", &repo, Some("why")),
        add_repo_args("p1", &with_pr, None),
    ];
    for request in &requests {
        for key in request.as_object().unwrap().keys() {
            assert!(
                rules.contains(&format!("\"{key}\"")),
                "the control surface does not read {key}"
            );
            assert!(
                key != "headRepo" && key != "setAsPrimary",
                "{key} is refused from a control caller"
            );
        }
    }
    assert!(rules.contains("refuse_head_repo") && rules.contains("\"setAsPrimary\""));
}

// -----------------------------------------------------------------------
// project create
// -----------------------------------------------------------------------

#[test]
fn create_reads_the_new_project_back_by_its_id_and_prints_it() {
    let app = App::default()
        .answer("create_project", project_row("p1", "Fix login", "local"))
        .answer(
            "list_project_repos",
            json!([repo_row(
                "r1",
                "acme/widgets",
                Some("services/auth"),
                "mt/fix-login",
                true
            )]),
        )
        .answer(
            "list_branches_for_project",
            json!([local_branch("b1", "r1", "mt/fix-login")]),
        );
    let repo = repo_flags("Acme/Widgets:services/auth");
    let out = create_with(
        app.invoke(),
        "Fix login",
        ProjectLocation::Local,
        Some(&repo),
    )
    .unwrap();

    // The id comes from the create's answer. `list_projects` is never
    // needed, and no notes are read when no pull request is involved.
    assert_eq!(
        app.commands(),
        vec![
            "create_project",
            "list_project_repos",
            "list_branches_for_project"
        ]
    );
    assert_eq!(
        app.args_of("list_project_repos"),
        json!({ "projectId": "p1" })
    );
    assert_eq!(
        app.args_of("list_branches_for_project"),
        json!({ "projectId": "p1" })
    );
    assert_eq!(
        out.human,
        "project \"Fix login\" created\n\
         \x20 id: p1\n\
         \x20 location: local\n\
         \x20 repos:\n\
         \x20   acme/widgets:services/auth  (primary)\n\
         \x20     branch mt/fix-login  base:origin/main\n\
         \x20 setup: worktree and setup actions are running in the background\n\
         next: stagedctl project get p1"
    );
    // The `project get --json` shape, with the app's rows untouched.
    assert_eq!(
        out.json,
        json!({
            "ok": true,
            "project": project_row("p1", "Fix login", "local"),
            "repos": [{
                "repo": repo_row("r1", "acme/widgets", Some("services/auth"), "mt/fix-login", true),
                "branches": [local_branch("b1", "r1", "mt/fix-login")],
            }],
        })
    );
}

#[test]
fn a_project_created_without_a_repo_points_at_add_repo() {
    let app = App::default()
        .answer("create_project", project_row("p1", "Scratch", "local"))
        .answer("list_project_repos", json!([]))
        .answer("list_branches_for_project", json!([]));
    let out = create_with(app.invoke(), "Scratch", ProjectLocation::Local, None).unwrap();
    assert_eq!(
        out.human,
        "project \"Scratch\" created\n\
         \x20 id: p1\n\
         \x20 location: local\n\
         \x20 repos:\n\
         \x20   (none)\n\
         next: stagedctl project add-repo p1 --repo <owner>/<repo>[:<subpath>]"
    );
    assert_eq!(out.json["repos"], json!([]));
    assert_eq!(out.json.get("note"), None);
}

#[test]
fn a_remote_project_says_its_workspace_is_being_provisioned() {
    let app = App::default()
        .answer("create_project", project_row("p1", "Fix login", "remote"))
        .answer(
            "list_project_repos",
            json!([repo_row("r1", "acme/widgets", None, "fix-login", true)]),
        )
        .answer(
            "list_branches_for_project",
            json!([remote_branch("b1", "r1", "fix-login", "fix-login-1a2b3c")]),
        );
    let repo = repo_flags("acme/widgets");
    let out = create_with(
        app.invoke(),
        "Fix login",
        ProjectLocation::Remote,
        Some(&repo),
    )
    .unwrap();
    assert_eq!(app.args_of("create_project")["location"], "remote");
    assert!(
        out.human.contains("\n  location: remote\n"),
        "{}",
        out.human
    );
    assert!(
        out.human.contains(
            "\n  setup: workspace fix-login-1a2b3c is being provisioned; it starts while a \
             Staged window is open\n"
        ),
        "{}",
        out.human
    );
}

#[test]
fn a_pull_request_adds_the_note_its_description_was_saved_as() {
    let mut branch = local_branch("b1", "r1", "fix-login");
    branch["prNumber"] = json!(42);
    let app = App::default()
        .answer("create_project", project_row("p1", "Fix login", "local"))
        .answer(
            "list_project_repos",
            json!([repo_row("r1", "acme/widgets", None, "fix-login", true)]),
        )
        .answer("list_branches_for_project", json!([branch]))
        .answer(
            "list_project_notes",
            json!([
                // Another pull request's note, and one whose number only
                // starts the same way.
                { "id": "n0", "title": "PR #4: Other", "createdAt": 5, "content": "x" },
                { "id": "n9", "title": "PR #420: Unrelated", "createdAt": 9, "content": "x" },
                { "id": "n1", "title": "PR #42: Fix login", "createdAt": 7, "content": "Why." },
            ]),
        );
    let repo = RepoRequest::from_flags("acme/widgets", None, Some(42), None, false).unwrap();
    let out = create_with(
        app.invoke(),
        "Fix login",
        ProjectLocation::Local,
        Some(&repo),
    )
    .unwrap();

    assert_eq!(app.args_of("create_project")["prNumber"], 42);
    assert_eq!(
        app.args_of("list_project_notes"),
        json!({ "projectId": "p1" })
    );
    assert!(
        out.human.contains(
            "\n      branch fix-login  base:origin/main  PR #42\n\
             \x20 note: #project-note:n1  PR #42: Fix login\n\
             \x20 setup: worktree"
        ),
        "{}",
        out.human
    );
    // The note's body is not echoed: only where to find it.
    assert_eq!(
        out.json["note"],
        json!({ "id": "n1", "title": "PR #42: Fix login" })
    );
}

#[test]
fn a_branch_whose_pull_request_the_app_found_gets_its_note_too() {
    // `--branch` names a branch with an open pull request: the app links it
    // and saves the note, so the note is looked for by what the branch row
    // carries, not by which flag was given.
    let mut branch = local_branch("b1", "r1", "fix-login");
    branch["prNumber"] = json!(7);
    let app = App::default()
        .answer("create_project", project_row("p1", "Fix login", "local"))
        .answer(
            "list_project_repos",
            json!([repo_row("r1", "acme/widgets", None, "fix-login", true)]),
        )
        .answer("list_branches_for_project", json!([branch]))
        .answer("list_project_notes", json!([]));
    let repo =
        RepoRequest::from_flags("acme/widgets", Some("fix-login"), None, None, false).unwrap();
    let out = create_with(
        app.invoke(),
        "Fix login",
        ProjectLocation::Local,
        Some(&repo),
    )
    .unwrap();
    assert!(app.commands().contains(&"list_project_notes".to_string()));
    // A note the app could not save is not an error: the line is left out.
    assert!(!out.human.contains("note:"), "{}", out.human);
    assert_eq!(out.json.get("note"), None);
}

#[test]
fn a_name_in_use_and_the_creation_limit_pass_through_as_the_apps_error() {
    for refusal in [
        "A project named \"Fix login\" already exists.",
        "This session and the sessions in its chain have created 10 projects, the limit they \
         share. A chain is a session and every session started from it, directly or through \
         another. Deleting one of those projects in Staged frees a slot.",
    ] {
        let app = App::default().refuse("create_project", refusal);
        let err = create_with(app.invoke(), "Fix login", ProjectLocation::Local, None).unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(message(&err), format!("app_error: {refusal}"));
        // Nothing landed, so nothing is reported as applied and nothing
        // more is asked.
        assert_eq!(err.partial_output(false), None);
        assert_eq!(app.commands(), vec!["create_project"]);
    }
}

#[test]
fn the_cli_does_not_check_the_name_against_the_project_list() {
    // A session sees only the projects it covers, so the check would miss
    // most clashes. The app answers instead.
    let app = App::default()
        .answer("create_project", project_row("p1", "Fix login", "local"))
        .answer("list_project_repos", json!([]))
        .answer("list_branches_for_project", json!([]));
    create_with(app.invoke(), "Fix login", ProjectLocation::Local, None).unwrap();
    assert!(!app.commands().contains(&"list_projects".to_string()));
}

#[test]
fn a_scope_refusal_of_create_keeps_its_code_and_exit_4() {
    let app = App::default().deny(
        "create_project",
        "this credential is approved for project p0 only. Reconnect without --project to \
         create projects",
    );
    let err = create_with(app.invoke(), "Fix login", ProjectLocation::Local, None).unwrap_err();
    assert_eq!(err.exit_code(), 4);
    assert!(message(&err).starts_with("scope_denied: "), "{err}");
    assert_eq!(err.partial_output(false), None);
}

#[test]
fn a_create_that_landed_is_reported_when_the_re_read_fails() {
    // The project exists whether or not it can be shown, so the caller is
    // told, with the failing read's own error and exit code.
    let app = App::default()
        .answer("create_project", project_row("p1", "Fix login", "local"))
        .answer(
            "list_project_repos",
            json!([repo_row("r1", "acme/widgets", None, "fix-login", true)]),
        );
    let repo = repo_flags("acme/widgets");
    let err = create_with(
        app.invoke(),
        "Fix login",
        ProjectLocation::Local,
        Some(&repo),
    )
    .unwrap_err();

    assert_eq!(err.exit_code(), 2, "the branch read's transport fault");
    assert_eq!(
        message(&err),
        "transport: list_branches_for_project: connection reset"
    );
    assert_eq!(
        err.partial_output(false).unwrap(),
        "project \"Fix login\" created\n\
         \x20 id: p1\n\
         \x20 (created; re-reading its repos failed)\n\
         next: stagedctl project get p1"
    );
    let partial: Value = serde_json::from_str(&err.partial_output(true).unwrap()).unwrap();
    assert_eq!(
        partial,
        json!({
            "ok": false,
            "applied": [{
                "command": "create_project",
                "args": { "name": "Fix login", "location": "local", "githubRepo": "acme/widgets" },
                "result": project_row("p1", "Fix login", "local"),
            }],
            "error": {
                "code": "transport",
                "message": "list_branches_for_project: connection reset",
            },
        })
    );
}

#[test]
fn a_create_whose_answer_cannot_be_read_is_still_reported_as_landed() {
    let app = App::default().answer("create_project", json!({ "unexpected": true }));
    let err = create_with(app.invoke(), " Fix login ", ProjectLocation::Local, None).unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert_eq!(
        err.partial_output(false).unwrap(),
        "project \"Fix login\" created\n  (created; the app's answer could not be read)"
    );
    assert_eq!(app.commands(), vec!["create_project"]);
}

// -----------------------------------------------------------------------
// project add-repo: the already-attached check
// -----------------------------------------------------------------------

#[test]
fn a_repo_and_subpath_the_project_has_is_refused_naming_its_branch() {
    let project = project("p1", "Fix login");
    let rows = attached(vec![
        repo_row(
            "r1",
            "acme/widgets",
            Some("services/auth"),
            "mt/fix-login",
            true,
        ),
        repo_row("r2", "acme/gadgets", None, "mt/fix-login", false),
    ]);

    let address = RepoAddress::parse("acme/widgets:services/auth").unwrap();
    let err = ensure_not_attached(&project, &rows, &address).unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert_eq!(
        message(&err),
        "already_attached: acme/widgets:services/auth is already attached to project \
         \"Fix login\", on branch mt/fix-login. A project holds a repo once per subpath: \
         address the existing one with --repo acme/widgets:services/auth, or give a different \
         subpath"
    );

    // GitHub names compare without case, and the address printed is the
    // attached one, which is what the other verbs' --repo takes.
    for typed in [
        "Acme/Widgets:services/auth",
        "ACME/WIDGETS:/services/auth/",
        "acme/widgets: services/auth ",
    ] {
        let address = RepoAddress::parse(typed).unwrap();
        let err = ensure_not_attached(&project, &rows, &address).unwrap_err();
        assert!(
            message(&err).starts_with("already_attached: acme/widgets:services/auth is"),
            "{typed}: {err}"
        );
    }
    // The repo root, however it is spelt.
    for typed in ["Acme/Gadgets", "acme/gadgets:", "acme/gadgets:/"] {
        let address = RepoAddress::parse(typed).unwrap();
        let err = ensure_not_attached(&project, &rows, &address).unwrap_err();
        assert!(
            message(&err).starts_with("already_attached: acme/gadgets is"),
            "{typed}: {err}"
        );
    }
}

#[test]
fn another_subpath_of_an_attached_repo_is_not_a_duplicate() {
    let project = project("p1", "Fix login");
    let rows = attached(vec![repo_row(
        "r1",
        "acme/widgets",
        Some("services/auth"),
        "mt/fix-login",
        true,
    )]);
    for typed in [
        "acme/widgets",
        "acme/widgets:services/billing",
        // Paths are case-sensitive, unlike repo names.
        "acme/widgets:Services/Auth",
        "acme/widgets-two:services/auth",
    ] {
        let address = RepoAddress::parse(typed).unwrap();
        assert!(
            ensure_not_attached(&project, &rows, &address).is_ok(),
            "{typed}"
        );
    }
    assert!(
        ensure_not_attached(&project, &[], &RepoAddress::parse("acme/widgets").unwrap()).is_ok()
    );
}

#[test]
fn a_duplicate_is_refused_before_anything_is_written() {
    let app = App::default()
        .answer(
            "list_projects",
            json!([project_row("p1", "Fix login", "local")]),
        )
        .answer(
            "list_project_repos",
            json!([repo_row("r1", "acme/widgets", None, "mt/fix-login", true)]),
        );
    let err =
        add_repo_with(app.invoke(), "Fix login", &repo_flags("Acme/Widgets"), None).unwrap_err();
    assert!(message(&err).starts_with("already_attached: "), "{err}");
    assert_eq!(app.commands(), vec!["list_projects", "list_project_repos"]);
}

// -----------------------------------------------------------------------
// project add-repo
// -----------------------------------------------------------------------

#[test]
fn a_project_outside_the_credentials_reach_is_not_found_by_name() {
    // `list_projects` holds what the credential covers: for a session, its
    // own project and the ones it created. A name outside that resolves to
    // nothing, and the app is asked nothing more.
    let app = App::default().answer(
        "list_projects",
        json!([
            project_row("p1", "Mine", "local"),
            project_row("p2", "One I created", "local"),
        ]),
    );
    let err = add_repo_with(
        app.invoke(),
        "Someone else's",
        &repo_flags("acme/widgets"),
        None,
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert_eq!(
        message(&err),
        "not_found: no project named \"Someone else's\". Projects: Mine, One I created"
    );
    assert_eq!(app.commands(), vec!["list_projects"]);
}

/// The app as `add-repo` meets it: the projects the credential covers, the
/// repos the project has before and after the add, and what the add and
/// the reads after it answer. `notes: None` makes the note read fail.
struct AddScript {
    projects: Value,
    before: Value,
    added: Value,
    after: Value,
    branches: Value,
    notes: Option<Value>,
}

impl AddScript {
    fn run(
        &self,
        needle: &str,
        repo: &RepoRequest,
        reason: Option<&str>,
    ) -> (Result<CmdOutput, CtlError>, Vec<(String, Value)>) {
        let calls = RefCell::new(Vec::new());
        let mut repo_reads = 0;
        let result = add_repo_with(
            |command: &str, args: Value| -> Result<Value, CtlError> {
                calls.borrow_mut().push((command.to_string(), args));
                match command {
                    "list_projects" => Ok(self.projects.clone()),
                    "list_project_repos" => {
                        repo_reads += 1;
                        Ok(if repo_reads == 1 {
                            self.before.clone()
                        } else {
                            self.after.clone()
                        })
                    }
                    "add_project_repo" => Ok(self.added.clone()),
                    "list_branches_for_project" => Ok(self.branches.clone()),
                    "list_project_notes" => self.notes.clone().ok_or_else(|| {
                        CtlError::Transport("list_project_notes: connection reset".into())
                    }),
                    other => panic!("unexpected command {other}"),
                }
            },
            needle,
            repo,
            reason,
        );
        (result, calls.into_inner())
    }
}

fn args_of(calls: &[(String, Value)], command: &str) -> Value {
    calls
        .iter()
        .find(|(c, _)| c == command)
        .map(|(_, args)| args.clone())
        .unwrap_or_else(|| panic!("{command} was not called"))
}

#[test]
fn add_repo_resolves_a_created_project_by_name_or_id_and_prints_the_repo() {
    let added = repo_row("r2", "acme/gadgets", Some("api"), "created", false);
    let script = AddScript {
        projects: json!([
            project_row("p1", "Mine", "local"),
            project_row("p2", "One I created", "local"),
        ]),
        before: json!([repo_row("r1", "acme/widgets", None, "created", true)]),
        added: added.clone(),
        after: json!([
            repo_row("r1", "acme/widgets", None, "created", true),
            added.clone()
        ]),
        branches: json!([
            local_branch("b1", "r1", "created"),
            local_branch("b2", "r2", "created"),
        ]),
        notes: None,
    };
    for needle in ["One I created", "one i created", "p2"] {
        let (out, calls) = script.run(needle, &repo_flags("Acme/Gadgets:api"), None);
        let out = out.unwrap();
        // One write, between the reads. Every read after `list_projects`
        // names the resolved project.
        assert_eq!(
            calls.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
            vec![
                "list_projects",
                "list_project_repos",
                "add_project_repo",
                "list_project_repos",
                "list_branches_for_project",
            ],
            "{needle}"
        );
        assert_eq!(
            args_of(&calls, "list_project_repos"),
            json!({ "projectId": "p2" })
        );
        assert_eq!(
            args_of(&calls, "add_project_repo"),
            json!({ "projectId": "p2", "githubRepo": "Acme/Gadgets", "subpath": "api" })
        );
        assert_eq!(
            out.human,
            "repo acme/gadgets:api added to project \"One I created\"\n\
             \x20 branch created  base:origin/main\n\
             \x20 setup: worktree and setup actions are running in the background\n\
             next: stagedctl project get p2",
            "{needle}"
        );
        // The app's rows, untouched, for the new repo only.
        assert_eq!(
            out.json,
            json!({
                "ok": true,
                "projectId": "p2",
                "repo": added,
                "branches": [local_branch("b2", "r2", "created")],
            }),
            "{needle}"
        );
    }
}

#[test]
fn a_local_add_starts_a_worktree_and_shows_the_reason() {
    let mut added = repo_row("r2", "acme/gadgets", None, "fix-login", false);
    added["reason"] = json!("needs the client library");
    let script = AddScript {
        projects: json!([project_row("p1", "Fix login", "local")]),
        before: json!([repo_row("r1", "acme/widgets", None, "fix-login", true)]),
        added: added.clone(),
        after: json!([
            repo_row("r1", "acme/widgets", None, "fix-login", true),
            added
        ]),
        branches: json!([
            local_branch("b1", "r1", "fix-login"),
            local_branch("b2", "r2", "fix-login"),
        ]),
        notes: None,
    };
    let (out, calls) = script.run(
        "Fix login",
        &repo_flags("acme/gadgets"),
        Some("needs the client library"),
    );
    assert_eq!(
        out.unwrap().human,
        "repo acme/gadgets added to project \"Fix login\"\n\
         \x20 branch fix-login  base:origin/main\n\
         \x20 reason: needs the client library\n\
         \x20 setup: worktree and setup actions are running in the background\n\
         next: stagedctl project get p1"
    );
    assert_eq!(
        args_of(&calls, "add_project_repo")["reason"],
        "needs the client library"
    );
}

#[test]
fn the_first_repo_of_a_remote_project_provisions_its_workspace() {
    let added = repo_row("r1", "acme/widgets", None, "fix-login", true);
    let script = AddScript {
        projects: json!([project_row("p1", "Fix login", "remote")]),
        before: json!([]),
        added: added.clone(),
        after: json!([added]),
        branches: json!([remote_branch("b1", "r1", "fix-login", "fix-login-1a2b3c")]),
        notes: None,
    };
    let (out, _) = script.run("Fix login", &repo_flags("acme/widgets"), None);
    assert_eq!(
        out.unwrap().human,
        "repo acme/widgets added to project \"Fix login\"  (primary)\n\
         \x20 branch fix-login  base:origin/main  remote workspace:fix-login-1a2b3c\n\
         \x20 setup: workspace fix-login-1a2b3c is being provisioned; it starts while a Staged \
         window is open\n\
         next: stagedctl project get p1"
    );
}

#[test]
fn a_repo_joining_a_remote_projects_workspace_is_cloned_into_it() {
    let added = repo_row("r2", "acme/gadgets", None, "fix-login", false);
    let script = AddScript {
        projects: json!([project_row("p1", "Fix login", "remote")]),
        before: json!([repo_row("r1", "acme/widgets", None, "fix-login", true)]),
        added: added.clone(),
        after: json!([
            repo_row("r1", "acme/widgets", None, "fix-login", true),
            added
        ]),
        branches: json!([
            remote_branch("b1", "r1", "fix-login", "fix-login-1a2b3c"),
            remote_branch("b2", "r2", "fix-login", "fix-login-1a2b3c"),
        ]),
        notes: None,
    };
    let (out, _) = script.run("Fix login", &repo_flags("acme/gadgets"), None);
    assert_eq!(
        out.unwrap().human,
        "repo acme/gadgets added to project \"Fix login\"\n\
         \x20 branch fix-login  base:origin/main  remote workspace:fix-login-1a2b3c\n\
         \x20 setup: cloning into workspace fix-login-1a2b3c in the background\n\
         next: stagedctl project get p1"
    );
}

#[test]
fn setup_is_read_off_the_projects_branches() {
    let rows: Vec<BranchRow> = serde_json::from_value(json!([
        local_branch("b1", "r1", "one"),
        remote_branch("b2", "r2", "two", "ws-1"),
        remote_branch("b3", "r3", "three", "ws-1"),
    ]))
    .unwrap();
    assert_eq!(Setup::of(&rows[0], &rows), Setup::Worktree);
    assert_eq!(
        Setup::of(&rows[1], &rows),
        Setup::CloneIntoWorkspace("ws-1".into())
    );
    // Alone in the project, the same branch provisions the workspace.
    assert_eq!(
        Setup::of(&rows[1], &rows[..2]),
        Setup::ProvisionWorkspace("ws-1".into())
    );
    // A blank workspace name is no workspace, as the app reads it.
    let blank: Vec<BranchRow> = serde_json::from_value(json!([
        remote_branch("b1", "r1", "one", " "),
        remote_branch("b2", "r2", "two", "ws-1"),
    ]))
    .unwrap();
    assert_eq!(Setup::of(&blank[0], &blank), Setup::Worktree);
    assert_eq!(
        Setup::of(&blank[1], &blank),
        Setup::ProvisionWorkspace("ws-1".into())
    );
}

#[test]
fn add_repo_with_a_pull_request_names_the_note() {
    let added = repo_row("r1", "acme/widgets", None, "fix-login", true);
    let mut branch = local_branch("b1", "r1", "fix-login");
    branch["prNumber"] = json!(42);
    let repo = RepoRequest::from_flags("acme/widgets", None, Some(42), None, false).unwrap();
    let script = AddScript {
        projects: json!([project_row("p1", "Fix login", "local")]),
        before: json!([]),
        added: added.clone(),
        after: json!([added]),
        branches: json!([branch]),
        notes: Some(json!([
            { "id": "n1", "title": "PR #42: Fix login", "createdAt": 3 },
            // The same pull request attached again later: the newest note.
            { "id": "n2", "title": "PR #42: Fix login", "createdAt": 8 },
        ])),
    };
    let (out, calls) = script.run("Fix login", &repo, None);
    let out = out.unwrap();
    assert!(
        out.human
            .contains("\n  note: #project-note:n2  PR #42: Fix login\n"),
        "{}",
        out.human
    );
    assert_eq!(
        out.json["note"],
        json!({ "id": "n2", "title": "PR #42: Fix login" })
    );
    assert_eq!(
        args_of(&calls, "add_project_repo"),
        json!({ "projectId": "p1", "githubRepo": "acme/widgets", "prNumber": 42 })
    );
}

#[test]
fn an_add_that_landed_is_reported_when_the_re_read_fails() {
    let added = repo_row(
        "r1",
        "acme/widgets",
        Some("services/auth"),
        "fix-login",
        true,
    );
    let mut branch = local_branch("b1", "r1", "fix-login");
    branch["prNumber"] = json!(42);
    let repo =
        RepoRequest::from_flags("Acme/Widgets:services/auth", None, Some(42), None, false).unwrap();
    // The note read fails after the repo and its branch were read.
    let script = AddScript {
        projects: json!([project_row("p1", "Fix login", "local")]),
        before: json!([]),
        added: added.clone(),
        after: json!([added.clone()]),
        branches: json!([branch]),
        notes: None,
    };
    let (out, _) = script.run("Fix login", &repo, None);
    let err = out.unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert_eq!(
        message(&err),
        "transport: list_project_notes: connection reset"
    );
    // The address is the app's spelling of it, from the add's own answer.
    assert_eq!(
        err.partial_output(false).unwrap(),
        "repo acme/widgets:services/auth added to project \"Fix login\"\n\
         \x20 (added; re-reading it failed)\n\
         next: stagedctl project get p1"
    );
    let partial: Value = serde_json::from_str(&err.partial_output(true).unwrap()).unwrap();
    assert_eq!(partial["ok"], false);
    assert_eq!(
        partial["applied"],
        json!([{
            "command": "add_project_repo",
            "args": {
                "projectId": "p1",
                "githubRepo": "Acme/Widgets",
                "subpath": "services/auth",
                "prNumber": 42,
            },
            "result": added,
        }])
    );
}

#[test]
fn the_apps_refusals_of_an_add_pass_through() {
    for refusal in [
        "'acme/widgets' appears to be a monorepo (31 modules in MODULES.yaml). Give the \
         subpath of the service or package to work in, or pass --allow-monorepo-root to use \
         the repository root.",
        "Workspace 'ws-1' is not ready (status: starting). Wait until it is running, then \
         retry.",
    ] {
        let app = App::default()
            .answer(
                "list_projects",
                json!([project_row("p1", "Fix login", "remote")]),
            )
            .answer("list_project_repos", json!([]))
            .refuse("add_project_repo", refusal);
        let err = add_repo_with(app.invoke(), "p1", &repo_flags("acme/widgets"), None).unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(message(&err), format!("app_error: {refusal}"));
        assert_eq!(err.partial_output(false), None);
    }

    // A read-only credential reads the project and is refused the write.
    let app = App::default()
        .answer(
            "list_projects",
            json!([project_row("p1", "Fix login", "local")]),
        )
        .answer("list_project_repos", json!([]))
        .deny("add_project_repo", "this credential is read-only");
    let err = add_repo_with(app.invoke(), "p1", &repo_flags("acme/widgets"), None).unwrap_err();
    assert_eq!(err.exit_code(), 4);
    assert_eq!(message(&err), "scope_denied: this credential is read-only");
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
    for command in PROJECT_WRITE_COMMANDS {
        assert!(listed.contains(command), "{command} is not allowlisted");
        assert!(is_project_write_command(command));
    }
    // The reads the two verbs make afterwards.
    for command in [
        "list_projects",
        "list_project_repos",
        "list_branches_for_project",
        "list_project_notes",
    ] {
        assert!(listed.contains(&command), "{command} is not allowlisted");
        assert!(!is_project_write_command(command));
    }
}

#[test]
fn help_states_the_apps_limits_as_the_app_has_them() {
    let help = crate::cli::render_all_help();
    let create = help
        .split("=== stagedctl project create --help ===")
        .nth(1)
        .and_then(|rest| rest.split("\n=== ").next())
        .expect("project create has help");
    let add_repo = help
        .split("=== stagedctl project add-repo --help ===")
        .nth(1)
        .and_then(|rest| rest.split("\n=== ").next())
        .expect("project add-repo has help");

    // The creation limit.
    let control = include_str!("../../../../src/project_commands/control.rs");
    assert!(
        control.contains("pub(crate) const CREATED_PROJECT_LIMIT: usize = 10;"),
        "the creation limit changed; update `project create --help`"
    );
    assert!(create.contains("at most 10 created projects"), "{create}");
    // Whose limit it is: a chain's, so a session it starts gets no more.
    assert!(
        create.contains("shared along a chain of sessions"),
        "{create}"
    );

    // The monorepo rule, and the flag the app's refusal names.
    let resolver = include_str!("../../../../src/project_commands/repo_spec.rs");
    assert!(
        resolver.contains("const MONOREPO_MODULE_THRESHOLD: u32 = 20;"),
        "the monorepo threshold changed; update the help of both verbs"
    );
    assert!(resolver.contains("--allow-monorepo-root"));
    for text in [create, add_repo] {
        assert!(text.contains("monorepo (20 or more modules)"), "{text}");
        assert!(text.contains("--allow-monorepo-root"), "{text}");
    }

    // The location default, and that remote never falls back.
    assert!(
        create.contains("--location is local unless given"),
        "{create}"
    );
    assert!(create.contains("[default: local]"), "{create}");
    assert!(create.contains("never\n"), "{create}");

    // The note's title, which is how the note is found afterwards.
    let note = include_str!("../../../../src/project_commands/pr_note.rs");
    assert!(note.contains("format!(\"PR #{number}: {title}\")"));
    assert!(create.contains("PR #<number>: <title>"), "{create}");
}

#[test]
fn no_help_says_a_session_is_confined_to_its_own_project() {
    // A session's credential covers its own project and the projects it
    // created. Help that still calls it confined to one project would send
    // an agent away from `project create`.
    let help = crate::cli::render_all_help();
    for stale in [
        "confined to the session's own project",
        "own project-scoped credential",
        "at most the\ncredential's own project",
    ] {
        assert!(!help.contains(stale), "help still says {stale:?}");
    }
    assert!(help.contains("projects the session created"), "{help}");
}

#[test]
fn the_guide_describes_both_verbs_and_what_a_session_covers() {
    // The guide ships with the app and is named in every session's prompt.
    let guide = include_str!("../../../../resources/stagedctl/guide.md");
    // One paragraph per line of thought, whatever the wrapping.
    let flat = guide.split_whitespace().collect::<Vec<_>>().join(" ");

    // A session's credential covers its own project and the projects it
    // created. A guide that calls it confined to one project would send an
    // agent away from `project create`.
    assert!(!flat.contains("confined"), "the guide still says confined");
    assert!(
        flat.contains("covers the session's own project and the projects the session created"),
        "{flat}"
    );

    // Both verbs, under the safety invariant, with what they start.
    for expected in [
        "`stagedctl project create`",
        "`stagedctl project add-repo`",
        "the user can delete the project or remove the repo",
        "cloned under `~/.staged/repos`",
        "setup actions run",
        "provisions a Blox workspace",
        "--allow-monorepo-root",
    ] {
        assert!(flat.contains(expected), "the guide lacks {expected:?}");
    }

    // The limit, as the app has it.
    let control = include_str!("../../../../src/project_commands/control.rs");
    assert!(
        control.contains("pub(crate) const CREATED_PROJECT_LIMIT: usize = 10;"),
        "the creation limit changed; update the guide"
    );
    assert!(flat.contains("at most 10 created projects"), "{flat}");
    assert!(flat.contains("shared along a chain of sessions"), "{flat}");
}
