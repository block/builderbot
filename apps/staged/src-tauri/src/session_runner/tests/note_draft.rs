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

/// A reply that quotes a diagram which does not parse, as an agent
/// explaining the block it just fixed would.
const REPLY_QUOTING_A_BROKEN_BLOCK: &str =
    "Saved. The block was:\n```pikchr\nbox \"unterminated\n```";
const VALID_NOTE_BODY: &str = "Intro\n\n```pikchr\nbox \"fixed\" fit\n```";
const BROKEN_NOTE_BODY: &str = "Intro\n\n```pikchr\nbox \"unterminated\n```";

fn tool_submission(body: &str) -> NoteSubmission {
    NoteSubmission {
        title: "Plan".to_string(),
        body: body.to_string(),
        suggested_next_steps: None,
        source: NoteSubmissionSource::Tool,
    }
}

/// With a `write_note` tool attached and a published note in the slot the
/// reply is never saved, so a broken block in it is not an error. Without a
/// tool, or with nothing published (the `---` fallback), the reply is still
/// what gets checked.
#[test]
fn a_published_note_is_validated_instead_of_the_unsaved_reply() {
    let store = Arc::new(Store::in_memory().unwrap());
    let session_id = note_session(&store, false, "");
    store
        .add_session_message(&session_id, MessageRole::User, "Write a note")
        .unwrap();
    store
        .add_session_message(
            &session_id,
            MessageRole::Assistant,
            REPLY_QUOTING_A_BROKEN_BLOCK,
        )
        .unwrap();

    // Tool attached, valid note published: nothing to repair.
    let slot = new_submission_slot();
    crate::note_tool::store_submission(&slot, tool_submission(VALID_NOTE_BODY));
    assert_eq!(
        turn_pikchr_error(&store, &session_id, &slot, true).unwrap(),
        None
    );

    // Tool attached, nothing published: the reply may be the `---`
    // fallback's note, so it is checked.
    let empty = new_submission_slot();
    let (error, origin) = turn_pikchr_error(&store, &session_id, &empty, true)
        .unwrap()
        .expect("the reply is validated when no note was published");
    assert_eq!(origin, PikchrErrorOrigin::FinalMessage);
    assert_eq!(error.line_number, 2);

    // No tool attached: the reply is checked, with or without a slot.
    for slot in [&empty, &slot] {
        let (_, origin) = turn_pikchr_error(&store, &session_id, slot, false)
            .unwrap()
            .expect("the reply is validated when the run has no tool");
        assert_eq!(origin, PikchrErrorOrigin::FinalMessage);
    }

    // The published note itself is still checked, and its line number is
    // the body's, not the reply's.
    crate::note_tool::store_submission(&slot, tool_submission(BROKEN_NOTE_BODY));
    let (error, origin) = turn_pikchr_error(&store, &session_id, &slot, true)
        .unwrap()
        .expect("a broken published note is an error");
    assert_eq!(origin, PikchrErrorOrigin::SubmittedNote);
    assert_eq!(error.line_number, 3);
}

/// The repair loop of a remote run: the collected note is broken, the
/// repair turn fixes the draft and republishes, and its reply quotes the
/// block it fixed. Once the outbox is re-collected the turn has nothing
/// left to repair, so the quoted block cannot spend another attempt or end
/// the run in `PikchrRepairExhausted` with a valid note published.
#[test]
fn a_repair_turn_is_judged_by_the_recollected_note_not_by_its_reply() {
    let store = Arc::new(Store::in_memory().unwrap());
    let session_id = note_session(&store, false, "");
    let slot = new_submission_slot();
    let memory = note_outbox::new_collected_note_memory();
    let log = RefCell::new(Vec::new());
    let done = |written_at: u64, body: &str| {
        let meta = format!(
            r#"{{"title":"Plan","suggestedNextSteps":null,"turn":"t1","writtenAt":{written_at},"helperVersion":5}}"#
        );
        format!("done\n{}\n{}\n", b64(meta.as_bytes()), b64(body.as_bytes()))
    };
    let collect = |output: &str| {
        let exec = recording_exec(output, &log);
        note_outbox::collect_turn_into_slot(&exec, &session_id, "t1", &slot, &memory).unwrap()
    };

    // First turn: a clean reply, a broken note in the outbox.
    store
        .add_session_message(&session_id, MessageRole::User, "Write a note")
        .unwrap();
    store
        .add_session_message(&session_id, MessageRole::Assistant, "Saved.")
        .unwrap();
    assert!(matches!(
        collect(&done(1, BROKEN_NOTE_BODY)),
        OutboxState::Delivered(_)
    ));
    let (error, origin) = turn_pikchr_error(&store, &session_id, &slot, true)
        .unwrap()
        .expect("the collected note is broken");
    assert_eq!(origin, PikchrErrorOrigin::SubmittedNote);

    // The repair turn: the reply quotes the broken block.
    let correction = build_pikchr_correction_prompt(&error, origin, true, Some("/home/u/draft.md"));
    store
        .add_session_message(&session_id, MessageRole::User, &correction)
        .unwrap();
    store
        .add_session_message(
            &session_id,
            MessageRole::Assistant,
            REPLY_QUOTING_A_BROKEN_BLOCK,
        )
        .unwrap();

    // A repair turn that did not republish leaves the broken note in the
    // outbox: the error is still the note's, not the reply's.
    assert!(matches!(
        collect(&done(1, BROKEN_NOTE_BODY)),
        OutboxState::Delivered(_)
    ));
    let (still, origin) = turn_pikchr_error(&store, &session_id, &slot, true)
        .unwrap()
        .expect("the note was not republished");
    assert_eq!(origin, PikchrErrorOrigin::SubmittedNote);
    assert_eq!(still.line_number, 3);

    // Re-collected after the republish: the note parses, and the reply's
    // quoted block is not an error.
    assert!(matches!(
        collect(&done(2, VALID_NOTE_BODY)),
        OutboxState::Delivered(_)
    ));
    assert_eq!(
        crate::note_tool::current_submission(&slot).unwrap().body,
        VALID_NOTE_BODY
    );
    assert_eq!(
        turn_pikchr_error(&store, &session_id, &slot, true).unwrap(),
        None
    );

    // The same transcript in a run with no tool is a `---` fallback run:
    // its reply is the note and is still checked.
    let (_, origin) = turn_pikchr_error(&store, &session_id, &new_submission_slot(), false)
        .unwrap()
        .expect("a run with no tool validates its reply");
    assert_eq!(origin, PikchrErrorOrigin::FinalMessage);
}

/// The first prompt is built before the run knows whether a `write_note`
/// tool attached. A note run with none (the local note server did not
/// start, or the Staged MCP on Blox could not be installed) is told so at
/// once, in the follow-up's wording; a run with the tool, and a session
/// that writes no note, get the prompt as built.
#[test]
fn a_note_run_without_the_tool_is_told_so_in_its_first_prompt() {
    let built = "<action>\nWrite the note in your draft file, then call `write_note`.\n</action>";
    let local_draft = crate::note_draft::local_draft_path("sess-1");
    let remote_draft = "/home/u/.staged/sessions/sess-1/draft.md";

    // Local: the draft path is fixed by the session id, but the note server
    // did not start. Remote: the install failed, so no draft path is known.
    let local_without_tool = attached_note_draft_path(Some(&local_draft), false, None);
    let remote_without_tool = attached_note_draft_path(None, false, None);
    assert_eq!(local_without_tool, None);
    assert_eq!(remote_without_tool, None);

    let followup = note_write_followup_prompt(None);
    for draft in [local_without_tool, remote_without_tool] {
        let prompt = first_turn_prompt(built.to_string(), true, draft.is_some());
        let notice = prompt
            .strip_prefix(built)
            .expect("the built prompt is kept, with the notice after it");
        assert!(notice.starts_with("\n\n<action>\n"), "{notice}");
        assert!(notice.ends_with("\n</action>"), "{notice}");
        // The follow-up's wording: the same sentence and the same format.
        for shared in [NOTE_TOOL_UNAVAILABLE, NOTE_SEPARATOR_FALLBACK_FORMAT] {
            assert!(notice.contains(shared), "{shared}: {notice}");
            assert!(followup.contains(shared), "{shared}: {followup}");
        }
        assert!(notice.contains("do not write the draft file"), "{notice}");
        for ask in ["call `write_note`", "call the `write_note`", "then call"] {
            assert!(!notice.contains(ask), "{ask}: {notice}");
        }
    }

    // With the tool attached, locally or on a workstation, nothing is added.
    let local_with_tool = attached_note_draft_path(Some(&local_draft), true, None);
    let remote_with_tool = attached_note_draft_path(None, false, Some(remote_draft));
    assert_eq!(
        local_with_tool.as_deref(),
        Some(local_draft.to_string_lossy().as_ref())
    );
    assert_eq!(remote_with_tool.as_deref(), Some(remote_draft));
    for draft in [local_with_tool, remote_with_tool] {
        assert_eq!(
            first_turn_prompt(built.to_string(), true, draft.is_some()),
            built
        );
    }

    // A session that writes no note never has the tool and gets no notice.
    assert_eq!(first_turn_prompt(built.to_string(), false, false), built);
}
