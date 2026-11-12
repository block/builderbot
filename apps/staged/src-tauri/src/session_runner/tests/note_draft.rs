//! The note draft file's run lifecycle: seeded or removed before the agent
//! starts, removed (locally) when the run is over, and — on a workstation —
//! prepared in the round trip the outbox recovery already makes.

use super::*;
use crate::remote_staged_mcp::note_outbox::{
    clear_outbox_and_seed_script, recover_uncollected_outbox, OutboxRecovery,
};
use crate::store::{Branch, Note, Project, ProjectNote, Session};
use std::cell::RefCell;

/// A stored note as an earlier turn left it: an H1 an older note may still
/// carry, and a media reference the amendment must not lose.
const STORED_NOTE: &str = "# Old title\n\nFirst paragraph.\n\n![Shot](staged-media://abc123.png)\n";
const SEEDED_BODY: &str = "First paragraph.\n\n![Shot](staged-media://abc123.png)";

/// A note session with a branch note or a project note holding `content`.
fn note_session(store: &Arc<Store>, project_note: bool, content: &str) -> String {
    let session = Session::new_running("write a note", std::path::Path::new("."));
    store.create_session(&session).unwrap();
    let project = Project::new("test/note-draft");
    store.create_project(&project).unwrap();
    if project_note {
        let note = ProjectNote::new(&project.id, "Old title", content).with_session(&session.id);
        store.create_project_note(&note).unwrap();
    } else {
        let branch = Branch::new(&project.id, "notes", "main");
        store.create_branch(&branch).unwrap();
        let note = Note::new(&branch.id, "Old title", content).with_session(&session.id);
        store.create_note(&note).unwrap();
    }
    session.id
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Records every workstation command; the collect script reads `collect_output`.
fn recording_exec<'a>(
    collect_output: &'a str,
    log: &'a RefCell<Vec<String>>,
) -> impl Fn(&[&str]) -> Result<String, String> + 'a {
    move |args: &[&str]| {
        assert_eq!(args[..2], ["sh", "-c"], "{args:?}");
        log.borrow_mut().push(args[2].to_string());
        if args[2].contains("echo done; else echo pending") {
            Ok(collect_output.to_string())
        } else {
            Ok(String::new())
        }
    }
}

#[test]
fn a_fresh_local_note_run_removes_a_stale_draft_and_creates_none() {
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session_id = note_session(&store, project_note, "");
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");

        // Nothing there: nothing is created (an empty file would trip
        // Claude Code's read-before-write gate).
        prepare_local_note_draft_for_run(&store, &session_id, &draft);
        assert!(!draft.exists());

        // A draft an earlier run left behind is not this run's note.
        std::fs::write(&draft, "stale body from an earlier run").unwrap();
        prepare_local_note_draft_for_run(&store, &session_id, &draft);
        assert!(!draft.exists(), "project_note={project_note}");
    }
}

#[test]
fn an_amending_local_note_run_seeds_the_draft_with_the_stored_body() {
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session_id = note_session(&store, project_note, STORED_NOTE);
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");
        std::fs::write(&draft, "stale body from an earlier run").unwrap();

        prepare_local_note_draft_for_run(&store, &session_id, &draft);

        // H1 stripped, the `staged-media://` reference verbatim.
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            SEEDED_BODY,
            "project_note={project_note}"
        );
        // What the seed holds is what `write_note` would publish unchanged.
        assert_eq!(crate::note_draft::read_draft(&draft).unwrap(), SEEDED_BODY);
    }
}

#[test]
fn the_local_draft_is_removed_when_the_run_is_over() {
    let dir = tempfile::tempdir().unwrap();
    let draft = dir.path().join("staged-note-draft.md");
    std::fs::write(&draft, "published body").unwrap();

    remove_local_note_draft_after_run("sess-1", &draft);
    assert!(!draft.exists());

    // A run whose agent never wrote a draft ends the same way.
    remove_local_note_draft_after_run("sess-1", &draft);
    assert!(!draft.exists());
}

#[test]
fn a_session_without_a_note_has_no_seed() {
    let store = Arc::new(Store::in_memory().unwrap());
    let session = Session::new_running("commit", std::path::Path::new("."));
    store.create_session(&session).unwrap();
    assert_eq!(stored_note_seed(&store, &session.id), None);
    assert_eq!(remote_note_draft_seed(&store, &session.id), None);
}

/// Remote run start is the outbox recovery: collect, then clear. The draft
/// rides the clear, so a fresh note costs the two commands it always did and
/// an amendment costs the same two.
#[test]
fn a_remote_run_start_prepares_the_draft_without_another_round_trip() {
    // Fresh note: the clear removes any stale `draft.md` and writes none.
    let store = Arc::new(Store::in_memory().unwrap());
    let session_id = note_session(&store, false, "");
    let log = RefCell::new(Vec::new());
    let exec = recording_exec("pending\n", &log);
    let recovery = recover_uncollected_outbox(
        &exec,
        &session_id,
        &mut |_| panic!("nothing to ingest"),
        &mut || remote_note_draft_seed(&store, &session_id),
    )
    .unwrap();
    assert_eq!(recovery, OutboxRecovery::Empty);
    {
        let commands = log.borrow();
        assert_eq!(commands.len(), 2, "collect, then clear: {commands:?}");
        assert_eq!(commands[1], clear_outbox_and_seed_script(&session_id, None));
        assert!(commands[1].contains("draft.md"), "{}", commands[1]);
        assert!(!commands[1].contains("base64"), "{}", commands[1]);
    }

    // Amendment: the same two commands; the clear also writes the seed.
    let store = Arc::new(Store::in_memory().unwrap());
    let session_id = note_session(&store, false, STORED_NOTE);
    let log = RefCell::new(Vec::new());
    let exec = recording_exec("pending\n", &log);
    recover_uncollected_outbox(
        &exec,
        &session_id,
        &mut |_| panic!("nothing to ingest"),
        &mut || remote_note_draft_seed(&store, &session_id),
    )
    .unwrap();
    let commands = log.borrow();
    assert_eq!(commands.len(), 2, "collect, then clear+seed: {commands:?}");
    assert_eq!(
        commands[1],
        clear_outbox_and_seed_script(&session_id, Some(&b64(SEEDED_BODY.as_bytes())))
    );
}

#[test]
fn the_note_follow_up_and_pikchr_repair_name_the_draft_file() {
    let draft = "/tmp/staged-note-draft-sess-1.md";

    let followup = note_write_followup_prompt(Some(draft));
    assert!(
        followup.contains(&format!("Write or finish the draft at `{draft}`")),
        "{followup}"
    );
    assert!(
        followup.contains("then call the `write_note` tool"),
        "{followup}"
    );
    assert!(followup.contains("`title`"), "{followup}");
    assert!(followup.contains("`suggested_next_steps`"), "{followup}");
    assert!(!followup.contains("`body`"), "{followup}");
    assert_eq!(followup.matches(draft).count(), 1, "{followup}");

    let error = crate::pikchr_validation::PikchrValidationError {
        line_number: 7,
        message: "syntax error near boxx".to_string(),
    };
    let repair = build_pikchr_correction_prompt(&error, true, Some(draft));
    assert!(
        repair.contains(&format!("Your draft file `{draft}`")),
        "{repair}"
    );
    assert!(
        repair.contains("Edit the Pikchr block starting at line 7 of the note body in that file"),
        "{repair}"
    );
    assert!(
        repair.contains("call `write_note` again with the title and suggested next steps"),
        "{repair}"
    );
    assert!(repair.contains("syntax error near boxx"), "{repair}");
    assert!(!repair.contains("complete corrected note"), "{repair}");
    // Editing the draft is the repair, so only *other* files are off limits.
    assert!(repair.contains("Do not edit any other file"), "{repair}");

    // A run that never learned the path points at the file the prompt named.
    let unnamed = note_write_followup_prompt(None);
    assert!(
        unnamed.contains("Write or finish your note draft file (its path is in your instructions"),
        "{unnamed}"
    );
    assert!(!unnamed.contains("`body`"), "{unnamed}");

    // A plain session's repair is still a corrected reply and names no file.
    let response = build_pikchr_correction_prompt(&error, false, Some(draft));
    assert!(response.contains("Return a complete corrected final response."));
    assert!(!response.contains(draft), "{response}");
}
