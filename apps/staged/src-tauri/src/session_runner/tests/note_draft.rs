//! The note draft file's run lifecycle: seeded or removed before the agent
//! starts, removed (locally) when the run is over, and — on a workstation —
//! prepared in the round trip the outbox recovery already makes.

use super::*;
use crate::remote_staged_mcp::note_outbox::{
    clear_outbox_and_seed_script, recover_uncollected_outbox, DraftSeed, OutboxRecovery,
    RunStartRecovery,
};
use crate::store::{Branch, Note, Project, ProjectNote, Session};
use std::cell::RefCell;

/// A stored note body as an earlier turn left it: it opens with a heading
/// of its own, which is body text (the title is stored apart), and carries a
/// media reference the amendment must not lose. The seed is this body
/// verbatim.
const STORED_NOTE: &str = "# Section\n\nFirst paragraph.\n\n![Shot](staged-media://abc123.png)\n";
/// What `write_note` publishes from a draft seeded with [`STORED_NOTE`] and
/// left untouched: the same text, trimmed.
const REPUBLISHED_BODY: &str =
    "# Section\n\nFirst paragraph.\n\n![Shot](staged-media://abc123.png)";

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

        // The stored body verbatim: its opening heading and the
        // `staged-media://` reference included.
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            STORED_NOTE,
            "project_note={project_note}"
        );
        // What the seed holds is what `write_note` would publish unchanged.
        assert_eq!(
            crate::note_draft::read_draft(&draft).unwrap(),
            REPUBLISHED_BODY
        );
    }
}

/// The stored body of the session's note, branch or project.
fn stored_note_content(store: &Store, session_id: &str, project_note: bool) -> (String, String) {
    if project_note {
        let note = store
            .get_project_note_by_session(session_id)
            .unwrap()
            .unwrap();
        (note.title, note.content)
    } else {
        let note = store.get_note_by_session(session_id).unwrap().unwrap();
        (note.title, note.content)
    }
}

/// Seed and publish agree that a heading at the top of the draft is body
/// text: a stored body that opens with `# Section` goes through the seed,
/// an untouched republish and the store, twice over, and comes back with
/// the heading in place and the title the call passed.
#[test]
fn a_body_that_opens_with_a_heading_survives_seed_republish_and_store() {
    const BODY: &str = "# Section\n\nFirst paragraph.\n\n## Details\n\nMore.";
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session_id = note_session(&store, project_note, BODY);
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");

        for round in 1..=2 {
            // Run start: the draft is seeded from the store.
            prepare_local_note_draft_for_run(&store, &session_id, &draft);
            assert_eq!(
                std::fs::read_to_string(&draft).unwrap(),
                BODY,
                "round {round}, project_note={project_note}"
            );

            // The agent changes nothing and republishes.
            let slot = crate::note_tool::new_submission_slot();
            let result = crate::note_tool::handle_write_note(
                &session_id,
                &slot,
                &draft,
                crate::note_tool::WriteNoteParams {
                    title: format!("Kept title {round}"),
                    suggested_next_steps: None,
                },
            );
            assert_ne!(result.is_error, Some(true), "round {round}");
            let submission = crate::note_tool::current_submission(&slot).unwrap();
            assert_eq!(submission.body, BODY, "round {round}");

            // Run end: the hooks store what the call published.
            remove_local_note_draft_after_run(&session_id, &draft);
            let outcome = run_post_completion_hooks(
                &session_id,
                std::path::Path::new("."),
                None,
                None,
                None,
                &store,
                0,
                Some(submission),
            );
            assert!(outcome.note_stored, "round {round}");
            let (title, content) = stored_note_content(&store, &session_id, project_note);
            assert_eq!(title, format!("Kept title {round}"));
            assert_eq!(
                content, BODY,
                "round {round}, project_note={project_note}: the heading is body text"
            );
        }
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
    assert_eq!(recovery.outbox, OutboxRecovery::Empty);
    assert_eq!(recovery.draft, DraftSeed::Absent);
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
        clear_outbox_and_seed_script(&session_id, Some(&b64(STORED_NOTE.as_bytes())))
    );
}

/// The end-of-run clear policy follows what the recovery observed of the
/// outbox, not whether the draft could be seeded.
#[test]
fn a_failed_draft_seed_leaves_the_recovery_settled() {
    for draft in [
        DraftSeed::Absent,
        DraftSeed::Seeded,
        DraftSeed::Failed("no space left on device".to_string()),
    ] {
        for outbox in [
            OutboxRecovery::Empty,
            OutboxRecovery::Ingested,
            OutboxRecovery::Discarded,
        ] {
            assert!(
                remote_outbox_recovery_settled_for_run(
                    "sess-1",
                    Ok(RunStartRecovery {
                        outbox,
                        draft: draft.clone()
                    })
                ),
                "{outbox:?} {draft:?}"
            );
        }
    }
    assert!(!remote_outbox_recovery_settled_for_run(
        "sess-1",
        Err("workstation suspended".to_string())
    ));

    // End to end through the recovery: the outbox was read and cleared, the
    // seed was refused, and the run is still settled.
    let store = Arc::new(Store::in_memory().unwrap());
    let session_id = note_session(&store, false, STORED_NOTE);
    let exec = |args: &[&str]| -> Result<String, String> {
        if args[2].contains("echo done; else echo pending") {
            Ok("pending\n".to_string())
        } else {
            Ok("staged-draft-seed-failed\n".to_string())
        }
    };
    let recovery = recover_uncollected_outbox(
        &exec,
        &session_id,
        &mut |_| panic!("nothing to ingest"),
        &mut || remote_note_draft_seed(&store, &session_id),
    );
    assert!(
        matches!(
            &recovery,
            Ok(RunStartRecovery {
                outbox: OutboxRecovery::Empty,
                draft: DraftSeed::Failed(_)
            })
        ),
        "{recovery:?}"
    );
    assert!(remote_outbox_recovery_settled_for_run(
        &session_id,
        recovery
    ));
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
    let repair =
        build_pikchr_correction_prompt(&error, PikchrErrorOrigin::SubmittedNote, true, Some(draft));
    assert!(
        repair.contains("The note you saved contains an invalid"),
        "{repair}"
    );
    assert!(
        repair.contains(&format!(
            "Your draft file `{draft}` holds the note you saved."
        )),
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

    // A plain session's repair is still a corrected reply and names no file.
    let response =
        build_pikchr_correction_prompt(&error, PikchrErrorOrigin::FinalMessage, false, Some(draft));
    assert!(response.contains("Return a complete corrected final response."));
    assert!(!response.contains(draft), "{response}");
}

/// The line number of an error found in the final assistant message counts
/// the reply's lines, and a fresh note may have no draft at all, so only an
/// error found in the submitted note may claim the draft holds the saved
/// note at that line.
#[test]
fn a_pikchr_error_in_the_reply_makes_no_claim_about_the_draft_file() {
    let draft = "/tmp/staged-note-draft-sess-1.md";
    let error = crate::pikchr_validation::PikchrValidationError {
        line_number: 7,
        message: "syntax error near boxx".to_string(),
    };

    let from_reply =
        build_pikchr_correction_prompt(&error, PikchrErrorOrigin::FinalMessage, true, Some(draft));
    assert!(
        from_reply.contains("The final assistant response contains an invalid"),
        "{from_reply}"
    );
    assert!(
        from_reply.contains("The Pikchr block starting at line 7 failed to parse"),
        "{from_reply}"
    );
    assert!(
        from_reply.contains("counts the lines of your reply, not of your draft file"),
        "{from_reply}"
    );
    for claim in [
        "holds the note you saved",
        "The note you saved",
        "of the note body in that file",
        "in that file",
    ] {
        assert!(!from_reply.contains(claim), "{claim}: {from_reply}");
    }
    // It still routes the fix through the draft and the tool, by path.
    assert!(
        from_reply.contains(&format!("the note is your draft file `{draft}`")),
        "{from_reply}"
    );
    assert!(
        from_reply.contains("write the file if it does not exist yet"),
        "{from_reply}"
    );
    assert!(from_reply.contains("call `write_note`"), "{from_reply}");
    assert!(
        from_reply.contains("Do not edit any other file"),
        "{from_reply}"
    );

    let from_note =
        build_pikchr_correction_prompt(&error, PikchrErrorOrigin::SubmittedNote, true, Some(draft));
    assert!(
        from_note.contains("holds the note you saved"),
        "{from_note}"
    );
    assert!(
        from_note.contains("line 7 of the note body in that file"),
        "{from_note}"
    );
}

/// A note run with no `write_note` tool attached — the local server did not
/// start, or the Staged MCP on Blox could not be installed — can only
/// deliver through the `---` fallback. Neither prompt may ask for a call
/// that cannot be made.
#[test]
fn a_run_without_the_write_note_tool_is_pointed_at_the_separator_fallback() {
    let error = crate::pikchr_validation::PikchrValidationError {
        line_number: 3,
        message: "syntax error near boxx".to_string(),
    };
    let followup = note_write_followup_prompt(None);
    let repair =
        build_pikchr_correction_prompt(&error, PikchrErrorOrigin::FinalMessage, true, None);

    for prompt in [&followup, &repair] {
        assert!(
            prompt.contains("The `write_note` tool is not available in this session"),
            "{prompt}"
        );
        assert!(prompt.contains("a line containing only `---`"), "{prompt}");
        assert!(prompt.contains("`# <Title>`"), "{prompt}");
        for ask in [
            "call `write_note`",
            "call the `write_note`",
            "`write_note` again",
            "then call",
            "republish",
        ] {
            assert!(!prompt.contains(ask), "{ask}: {prompt}");
        }
        assert_eq!(prompt.matches("write_note").count(), 1, "{prompt}");
    }
    assert!(followup.contains("no note was saved"), "{followup}");
    assert!(
        repair.contains("Return a complete corrected final response"),
        "{repair}"
    );
    assert!(repair.contains("syntax error near boxx"), "{repair}");
    // The repair is a reply, so no file is to be edited at all.
    assert!(repair.contains("Do not edit files"), "{repair}");

    // What the follow-up asks for is what the fallback parser accepts.
    let reply = "Here is the note.\n\n---\n# Design\n\nBody.";
    assert_eq!(
        extract_note_content(reply).as_deref().map(str::trim),
        Some("# Design\n\nBody.")
    );
}
