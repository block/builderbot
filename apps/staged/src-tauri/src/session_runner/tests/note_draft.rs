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
/// media reference the amendment must not lose. The seed is the stored
/// title's line, a blank line, then this body verbatim.
const STORED_NOTE: &str = "# Section\n\nFirst paragraph.\n\n![Shot](staged-media://abc123.png)\n";
/// The title [`note_session`] stores its note under.
const STORED_TITLE: &str = "Old title";
/// The draft a run that amends [`STORED_NOTE`] starts with.
const SEEDED_DRAFT: &str =
    "# Old title\n\n# Section\n\nFirst paragraph.\n\n![Shot](staged-media://abc123.png)\n";
/// The body `write_note` publishes from [`SEEDED_DRAFT`] left untouched:
/// the stored body, trimmed, without the title line.
const REPUBLISHED_BODY: &str =
    "# Section\n\nFirst paragraph.\n\n![Shot](staged-media://abc123.png)";

/// A note session with a branch note or a project note titled
/// [`STORED_TITLE`] and holding `content`.
fn note_session(store: &Arc<Store>, project_note: bool, content: &str) -> String {
    let session = Session::new_running("write a note", std::path::Path::new("."));
    store.create_session(&session).unwrap();
    let project = Project::new("test/note-draft");
    store.create_project(&project).unwrap();
    if project_note {
        let note = ProjectNote::new(&project.id, STORED_TITLE, content).with_session(&session.id);
        store.create_project_note(&note).unwrap();
    } else {
        let branch = Branch::new(&project.id, "notes", "main");
        store.create_branch(&branch).unwrap();
        let note = Note::new(&branch.id, STORED_TITLE, content).with_session(&session.id);
        store.create_note(&note).unwrap();
    }
    session.id
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A published note the start-of-run recovery turned down.
fn rejected_note() -> note_outbox::RejectedNote {
    note_outbox::RejectedNote {
        title: "Plan".to_string(),
        error: crate::pikchr_validation::PikchrValidationError {
            line_number: 12,
            message: "syntax error near boxx".to_string(),
        },
    }
}

/// Records every workstation command; the collect script reads `collect_output`.
fn recording_exec<'a>(
    collect_output: &'a str,
    log: &'a RefCell<Vec<String>>,
) -> impl Fn(&[&str]) -> Result<String, String> + 'a {
    move |args: &[&str]| {
        assert_eq!(args[..2], ["sh", "-c"], "{args:?}");
        log.borrow_mut().push(args[2].to_string());
        if note_outbox::is_collect_script(args[2]) {
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
fn an_amending_local_note_run_seeds_the_draft_with_the_stored_title_and_body() {
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session_id = note_session(&store, project_note, STORED_NOTE);
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");
        std::fs::write(&draft, "stale body from an earlier run").unwrap();

        prepare_local_note_draft_for_run(&store, &session_id, &draft);

        // The stored title as the first line, a blank line, then the stored
        // body verbatim: its opening heading and the `staged-media://`
        // reference included.
        assert_eq!(
            std::fs::read_to_string(&draft).unwrap(),
            SEEDED_DRAFT,
            "project_note={project_note}"
        );
        // What the seed holds is what `write_note` would publish unchanged:
        // the stored title and the stored body.
        assert_eq!(
            crate::note_draft::split_title(&crate::note_draft::read_draft(&draft).unwrap()),
            Ok((STORED_TITLE.to_string(), REPUBLISHED_BODY.to_string()))
        );
    }
}

/// Both transports are seeded with the same text: the local draft file
/// holds what a workstation's `draft.md` is sent, the stored title line
/// above the stored body.
#[test]
fn an_amending_run_seeds_the_same_title_and_body_on_both_transports() {
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session_id = note_session(&store, project_note, STORED_NOTE);
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");

        // Local: the file `write_note` reads.
        prepare_local_note_draft_for_run(&store, &session_id, &draft);
        let local = std::fs::read(&draft).unwrap();
        assert_eq!(local, SEEDED_DRAFT.as_bytes());

        // Remote: the bytes the run-start clear decodes into `draft.md`.
        assert_eq!(
            remote_note_draft_seed(&store, &session_id).as_deref(),
            Some(local.as_slice()),
            "project_note={project_note}"
        );
        let log = RefCell::new(Vec::new());
        let exec = recording_exec("pending\n", &log);
        let recovery = recover_uncollected_outbox(
            &exec,
            &session_id,
            &mut |_| panic!("nothing to ingest"),
            &mut || remote_note_draft_seed(&store, &session_id),
        )
        .unwrap();
        assert_eq!(recovery.draft, DraftSeed::Seeded);
        assert_eq!(
            log.borrow()[1],
            clear_outbox_and_seed_script(&session_id, Some(&b64(SEEDED_DRAFT.as_bytes())))
        );
    }

    // A fresh note is seeded on neither, whatever title its stub row has.
    let store = Arc::new(Store::in_memory().unwrap());
    let session_id = note_session(&store, false, "");
    assert_eq!(stored_note_seed(&store, &session_id), None);
    assert_eq!(remote_note_draft_seed(&store, &session_id), None);
}

/// The stored title, content and timestamp of the session's note, branch or
/// project.
fn stored_note_content(
    store: &Store,
    session_id: &str,
    project_note: bool,
) -> (String, String, i64) {
    if project_note {
        let note = store
            .get_project_note_by_session(session_id)
            .unwrap()
            .unwrap();
        (note.title, note.content, note.updated_at)
    } else {
        let note = store.get_note_by_session(session_id).unwrap().unwrap();
        (note.title, note.content, note.updated_at)
    }
}

/// One run of a local note session over `draft`: seed, let `edit` change
/// the seeded text, publish with no arguments, store. Returns the title
/// and body the call published.
fn seed_edit_publish_and_store(
    store: &Arc<Store>,
    session_id: &str,
    draft: &std::path::Path,
    edit: impl Fn(String) -> String,
) -> (String, String) {
    // Run start: the draft is seeded from the store.
    prepare_local_note_draft_for_run(store, session_id, draft);
    let seeded = std::fs::read_to_string(draft).unwrap();
    std::fs::write(draft, edit(seeded)).unwrap();

    // The agent publishes: the call names no title.
    let slot = crate::note_tool::new_submission_slot();
    let result = crate::note_tool::handle_write_note(
        session_id,
        &slot,
        draft,
        crate::note_tool::WriteNoteParams::default(),
    );
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let submission = crate::note_tool::current_submission(&slot).unwrap();
    let published = (submission.title.clone(), submission.body.clone());

    // Run end: the hooks store what the call published.
    remove_local_note_draft_after_run(session_id, draft);
    let outcome = run_post_completion_hooks(
        session_id,
        std::path::Path::new("."),
        None,
        None,
        None,
        store,
        0,
        Some(submission),
    );
    assert!(outcome.note_stored);
    published
}

/// Seed and publish mirror each other: the seed puts the stored title on
/// one line above the stored body and the publish takes one line off. A
/// stored body that opens with `# Section` goes through the seed, an
/// untouched republish and the store, twice over, and comes back with that
/// heading in place under the stored title. Nothing changed, so the store
/// does not restamp the note.
#[test]
fn a_body_that_opens_with_a_heading_survives_seed_republish_and_store() {
    const BODY: &str = "# Section\n\nFirst paragraph.\n\n## Details\n\nMore.";
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session_id = note_session(&store, project_note, BODY);
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");
        let (_, _, stamped) = stored_note_content(&store, &session_id, project_note);
        // A write from here on would carry a later timestamp.
        std::thread::sleep(std::time::Duration::from_millis(5));

        for round in 1..=2 {
            let what = format!("round {round}, project_note={project_note}");
            let published = seed_edit_publish_and_store(&store, &session_id, &draft, |seeded| {
                // The seed is the title line, a blank line and the body.
                assert_eq!(seeded, format!("# {STORED_TITLE}\n\n{BODY}"), "{what}");
                // The agent changes nothing.
                seeded
            });
            assert_eq!(
                published,
                (STORED_TITLE.to_string(), BODY.to_string()),
                "{what}"
            );
            assert_eq!(
                stored_note_content(&store, &session_id, project_note),
                (STORED_TITLE.to_string(), BODY.to_string(), stamped),
                "{what}: the same title and body, heading included, and no new timestamp"
            );
        }
    }
}

/// An amendment that edits the title line renames the note and leaves the
/// body alone; one that edits the body keeps the title the store had. Each
/// is a change, so the store restamps the note.
#[test]
fn an_amendment_edits_the_title_line_or_the_body_and_the_store_follows() {
    const BODY: &str = "# Section\n\nFirst paragraph.";
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session_id = note_session(&store, project_note, BODY);
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");
        let (_, _, stamped) = stored_note_content(&store, &session_id, project_note);
        std::thread::sleep(std::time::Duration::from_millis(5));

        // The title line alone.
        seed_edit_publish_and_store(&store, &session_id, &draft, |seeded| {
            seeded.replacen("# Old title", "# New title (2)", 1)
        });
        let (title, content, renamed_at) = stored_note_content(&store, &session_id, project_note);
        assert_eq!((title.as_str(), content.as_str()), ("New title (2)", BODY));
        assert!(renamed_at > stamped, "project_note={project_note}");
        std::thread::sleep(std::time::Duration::from_millis(5));

        // The body alone: the seed carries the new title, and it is kept.
        seed_edit_publish_and_store(&store, &session_id, &draft, |seeded| {
            assert!(seeded.starts_with("# New title (2)\n\n"), "{seeded}");
            format!("{seeded}\n\nSecond paragraph.")
        });
        let (title, content, edited_at) = stored_note_content(&store, &session_id, project_note);
        assert_eq!(title, "New title (2)");
        assert_eq!(content, format!("{BODY}\n\nSecond paragraph."));
        assert!(edited_at > renamed_at, "project_note={project_note}");
    }
}

/// A stored title with whitespace of any kind inside it is a title
/// `write_note` can store, and the seed writes it as stored. Seeded,
/// published untouched and stored, twice over, the note keeps its title,
/// its body and its timestamp: the republish renames nothing.
#[test]
fn a_title_with_inner_whitespace_survives_seed_republish_and_store() {
    for stored_title in [
        "Two  spaces",
        "Tab\tand   spaces",
        "No\u{a0}break",
        "Ideographic\u{3000}space",
        "Line\u{2028}separator",
        "Paragraph\u{2029}next\u{85}tab\u{b}feed\u{c}end",
    ] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session = Session::new_running("write a note", std::path::Path::new("."));
        store.create_session(&session).unwrap();
        let project = Project::new("test/note-draft");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "notes", "main");
        store.create_branch(&branch).unwrap();
        let note = Note::new(&branch.id, stored_title, "Body.").with_session(&session.id);
        store.create_note(&note).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");
        let (_, _, stamped) = stored_note_content(&store, &session.id, false);
        // A write from here on would carry a later timestamp.
        std::thread::sleep(std::time::Duration::from_millis(5));

        for round in 1..=2 {
            let what = format!("round {round}, {stored_title:?}");
            let published = seed_edit_publish_and_store(&store, &session.id, &draft, |seeded| {
                assert_eq!(seeded, format!("# {stored_title}\n\nBody."), "{what}");
                // The agent changes nothing.
                seeded
            });
            assert_eq!(
                published,
                (stored_title.to_string(), "Body.".to_string()),
                "{what}"
            );
            assert_eq!(
                stored_note_content(&store, &session.id, false),
                (stored_title.to_string(), "Body.".to_string(), stamped),
                "{what}"
            );
        }
    }
}

/// A stored title that does not fit on one line still seeds a draft
/// `write_note` accepts: a line break becomes a space, with the whitespace
/// touching it, and an empty title becomes `Untitled Note`. Whitespace
/// elsewhere in the title is kept. The publish stores that title, and the
/// next seed leaves it alone.
#[test]
fn a_stored_title_that_is_not_one_line_seeds_a_draft_that_publishes() {
    // (stored title, the title the seeded draft publishes)
    let cases = [
        ("Two\nlines  of title", "Two lines  of title"),
        ("Return\rin \r\n the\ttitle", "Return in the\ttitle"),
        ("", "Untitled Note"),
        ("  \n", "Untitled Note"),
    ];
    for (stored_title, safe_title) in cases {
        let store = Arc::new(Store::in_memory().unwrap());
        let session = Session::new_running("write a note", std::path::Path::new("."));
        store.create_session(&session).unwrap();
        let project = Project::new("test/note-draft");
        store.create_project(&project).unwrap();
        let branch = Branch::new(&project.id, "notes", "main");
        store.create_branch(&branch).unwrap();
        let note = Note::new(&branch.id, stored_title, "Body.").with_session(&session.id);
        store.create_note(&note).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("staged-note-draft.md");

        let published = seed_edit_publish_and_store(&store, &session.id, &draft, |seeded| {
            assert_eq!(
                seeded,
                format!("# {safe_title}\n\nBody."),
                "{stored_title:?}"
            );
            seeded
        });
        assert_eq!(
            published,
            (safe_title.to_string(), "Body.".to_string()),
            "{stored_title:?}"
        );
        let stored = store.get_note_by_session(&session.id).unwrap().unwrap();
        assert_eq!(
            (stored.title.as_str(), stored.content.as_str()),
            (safe_title, "Body."),
            "{stored_title:?}"
        );

        // The title the first publish stored is one the seed writes back
        // unchanged, so a second, untouched run restamps nothing.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let republished = seed_edit_publish_and_store(&store, &session.id, &draft, |seeded| {
            assert_eq!(
                seeded,
                format!("# {safe_title}\n\nBody."),
                "{stored_title:?}"
            );
            seeded
        });
        assert_eq!(
            republished,
            (safe_title.to_string(), "Body.".to_string()),
            "{stored_title:?}"
        );
        let again = store.get_note_by_session(&session.id).unwrap().unwrap();
        assert_eq!(
            (again.title, again.content, again.updated_at),
            (stored.title, stored.content, stored.updated_at),
            "{stored_title:?}"
        );
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
    assert!(!stored_note_has_body(&store, &session.id));
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
    // The seed is the stored note as a draft: the title line, then the body.
    assert_eq!(
        commands[1],
        clear_outbox_and_seed_script(&session_id, Some(&b64(SEEDED_DRAFT.as_bytes())))
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
        DraftSeed::Kept,
    ] {
        for outbox in [
            OutboxRecovery::Empty,
            OutboxRecovery::Ingested,
            OutboxRecovery::Discarded,
            OutboxRecovery::Rejected(rejected_note()),
        ] {
            assert!(
                remote_outbox_recovery_settled_for_run(
                    "sess-1",
                    Ok(RunStartRecovery {
                        outbox: outbox.clone(),
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
        if note_outbox::is_collect_script(args[2]) {
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

/// The automatic follow-up guards against storing an empty note, so it is
/// due only for a turn that delivered nothing on a note with no stored
/// body, and only while the run has its one attempt left.
#[test]
fn the_automatic_follow_up_is_due_only_for_an_undelivered_note_with_no_stored_body() {
    // (turn delivered, stored note has a body, attempts used) -> due
    let cases = [
        // A fresh note's silent turn: asked once, then left alone.
        (false, false, 0, true),
        (false, false, NOTE_WRITE_FOLLOWUP_MAX_ATTEMPTS, false),
        // A chat answer on a note that has a body: the stored note stands.
        (false, true, 0, false),
        (false, true, NOTE_WRITE_FOLLOWUP_MAX_ATTEMPTS, false),
        // A delivered note needs no prompt, fresh or amended.
        (true, false, 0, false),
        (true, true, 0, false),
        (true, false, NOTE_WRITE_FOLLOWUP_MAX_ATTEMPTS, false),
        (true, true, NOTE_WRITE_FOLLOWUP_MAX_ATTEMPTS, false),
    ];
    for (turn_delivered, stored_note_has_body, attempts_used, due) in cases {
        assert_eq!(
            note_write_followup_due(turn_delivered, stored_note_has_body, attempts_used),
            due,
            "delivered={turn_delivered} stored_body={stored_note_has_body} \
             attempts={attempts_used}"
        );
    }
}

/// The decision as the run loop makes it, from the store: a turn that
/// answered in chat and called nothing. On a fresh note (a blank stored
/// body) the follow-up is sent; on an amended one it is not, so the hooks
/// keep the stored note and its timestamp. A branch note and a project note
/// are read the same way.
#[test]
fn a_chat_only_turn_gets_the_follow_up_on_a_fresh_note_and_none_on_an_amended_one() {
    // (stored body, follow-up due)
    let notes = [("", true), ("  \n", true), (STORED_NOTE, false)];
    for project_note in [false, true] {
        for (content, due) in notes {
            let store = Arc::new(Store::in_memory().unwrap());
            let session_id = note_session(&store, project_note, content);
            let turn_start = store
                .add_session_message(&session_id, MessageRole::User, "Why does the test pass?")
                .unwrap();
            store
                .add_session_message(
                    &session_id,
                    MessageRole::Assistant,
                    "It never asks for a second page.",
                )
                .unwrap();
            let slot = new_submission_slot();
            let follow_up = |attempts_used| {
                note_write_followup_due(
                    note_turn_delivered(&store, &session_id, turn_start, &slot),
                    stored_note_has_body(&store, &session_id),
                    attempts_used,
                )
            };

            assert_eq!(
                stored_note_has_body(&store, &session_id),
                !due,
                "project_note={project_note} content={content:?}"
            );
            assert_eq!(
                follow_up(0),
                due,
                "project_note={project_note} content={content:?}"
            );
            // Fresh or amended, the run never asks twice.
            assert!(!follow_up(NOTE_WRITE_FOLLOWUP_MAX_ATTEMPTS));

            // A turn that did publish is not asked on either kind of note.
            crate::note_tool::store_submission(&slot, tool_submission(VALID_NOTE_BODY));
            assert!(
                !follow_up(0),
                "project_note={project_note} content={content:?}"
            );
        }
    }
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
    // The title is asked for as the draft's first line, not as an argument.
    assert!(
        followup.contains(
            "(the full note in markdown: the title as a `# ` heading on the first line, then \
             the body), then call the `write_note` tool with `suggested_next_steps`"
        ),
        "{followup}"
    );
    assert!(!followup.contains("`title`"), "{followup}");
    assert!(!followup.contains("without the title"), "{followup}");
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
    // The line number counts the body's lines, which start below the
    // draft's title line, so the prompt says where the count starts.
    assert!(
        repair.contains(
            "Edit the Pikchr block starting at line 7 of the note body in that file, counting \
             from the first non-blank line after the `# ` title line, and change nothing else"
        ),
        "{repair}"
    );
    assert!(
        repair.contains("call `write_note` again with the suggested next steps"),
        "{repair}"
    );
    for stale in ["with the title", "the file's first non-blank line)"] {
        assert!(!repair.contains(stale), "{stale}: {repair}");
    }
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
    // What the file must hold names the title line, and the call names no
    // title.
    assert!(
        from_reply.contains(
            "holds the full note, the title as a `# ` heading on its first line and the body \
             below it, with this diagram corrected"
        ),
        "{from_reply}"
    );
    assert!(
        from_reply.contains("then call `write_note` with the suggested next steps"),
        "{from_reply}"
    );
    assert!(!from_reply.contains("with the title"), "{from_reply}");
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
        // "In this run": a later Resume of the session may attach the tool,
        // and this sentence is still in the agent's context then.
        assert!(
            prompt.contains("The `write_note` tool is not available in this run"),
            "{prompt}"
        );
        assert!(!prompt.contains("in this session"), "{prompt}");
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
    // A draft the agent already wrote is the note in the reply's own shape:
    // the title line, then the body.
    assert!(
        followup.contains(
            "If you already wrote the note into your note draft file, use its contents as the \
             note: the file has that shape, the `# ` title line and then the body."
        ),
        "{followup}"
    );
    assert!(!followup.contains("as the body"), "{followup}");
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
        let prompt = first_turn_prompt(built.to_string(), true, draft.is_some(), None);
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
        // The tool is missing from this run only; a Resume may attach it.
        assert!(
            notice.contains("The `write_note` tool is not available in this run"),
            "{notice}"
        );
        assert!(!notice.contains("in this session"), "{notice}");
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
            first_turn_prompt(built.to_string(), true, draft.is_some(), None),
            built
        );
    }

    // A session that writes no note never has the tool and gets no notice.
    assert_eq!(
        first_turn_prompt(built.to_string(), false, false, None),
        built
    );
}

/// The instructions of a note session describe the suggested next steps
/// only as `write_note` arguments. A run without the tool is told, in its
/// first-turn notice and in its follow-up, to give them in the fenced block
/// the `---` fallback reads, and the block those prompts show is one
/// `extract_suggested_next_steps` parses.
#[test]
fn the_no_tool_prompts_show_a_next_steps_block_the_fallback_parses() {
    let example_steps = crate::note_tool::SuggestedNextSteps {
        suggested_next_commit_step: Some("Implement this plan".to_string()),
        suggested_next_note_step: None,
    };
    assert_eq!(
        extract_suggested_next_steps(NOTE_SEPARATOR_FALLBACK_NEXT_STEPS_EXAMPLE),
        Some(example_steps.clone())
    );

    let built = "<action>\nWrite the note in your draft file, then call `write_note`.\n</action>";
    let notice = first_turn_prompt(built.to_string(), true, false, None);
    let followup = note_write_followup_prompt(None);
    let next_steps = note_separator_fallback_next_steps();
    for prompt in [&notice, &followup] {
        assert!(prompt.contains(&next_steps), "{prompt}");
        assert!(prompt.ends_with("```\n</action>"), "{prompt}");
        assert!(
            prompt.contains("above the `---` line, in a `suggested-next-steps` fenced block"),
            "{prompt}"
        );
        for key in ["`suggestedNextCommitStep`", "`suggestedNextNoteStep`"] {
            assert!(prompt.contains(key), "{key}: {prompt}");
        }
        // The prompt's own text through the parser: the prose that names the
        // block is not taken for a fence, so the block found is the example.
        assert_eq!(
            extract_suggested_next_steps(prompt),
            Some(example_steps.clone()),
            "{prompt}"
        );
    }

    // A reply laid out as the notice asks: the block above the `---` line,
    // then the title and the body. The steps are read and stay out of the
    // note.
    let reply = format!(
        "The note is below.\n\n{NOTE_SEPARATOR_FALLBACK_NEXT_STEPS_EXAMPLE}\n\n---\n# Plan\n\nThe body."
    );
    assert_eq!(extract_suggested_next_steps(&reply), Some(example_steps));
    assert_eq!(
        extract_note_content(&reply).as_deref(),
        Some("# Plan\n\nThe body.")
    );

    // The Pikchr correction of such a run asks for the same reply format and
    // leaves the steps alone: the hooks read them from the run's earlier
    // reply.
    let error = crate::pikchr_validation::PikchrValidationError {
        line_number: 3,
        message: "syntax error near boxx".to_string(),
    };
    let repair =
        build_pikchr_correction_prompt(&error, PikchrErrorOrigin::FinalMessage, true, None);
    assert!(!repair.contains("suggested-next-steps"), "{repair}");

    // A run with the tool attached is never shown the block.
    assert_eq!(
        first_turn_prompt(built.to_string(), true, true, None),
        built
    );
    assert!(
        !note_write_followup_prompt(Some("/tmp/draft.md")).contains("```suggested-next-steps"),
        "the follow-up of a run with the tool asks for the argument"
    );
}

/// A run whose start-of-run recovery turned down an earlier run's published
/// note opens with the repair request: the prompt as built, a notice of what
/// Staged found, then the correction a live run sends for a submitted note,
/// pointing at the block's line in this run's draft. The recovery kept that
/// draft, so the agent is asked to edit it, not to write the note again.
#[test]
fn a_run_whose_recovery_rejected_a_note_opens_with_the_repair_request() {
    let built = "Continue.";
    let draft = "/home/u/.staged/sessions/sess-1/draft.md";
    let rejected = rejected_note();

    let prompt = first_turn_prompt(built.to_string(), true, true, Some((&rejected, draft)));
    let added = prompt
        .strip_prefix(built)
        .expect("the built prompt is kept, with the repair request after it");
    let correction = build_pikchr_correction_prompt(
        &rejected.error,
        PikchrErrorOrigin::SubmittedNote,
        true,
        Some(draft),
    );
    let notice = added
        .strip_suffix(correction.as_str())
        .expect("the request ends with the submitted-note correction, unchanged");
    assert!(notice.starts_with("\n\n<action>\n"), "{notice}");
    assert!(notice.ends_with("\n</action>\n\n"), "{notice}");
    for said in [
        "an earlier run of this session published, \"Plan\"",
        "It was not stored",
        "Your draft file was left as that run wrote it",
        "instead of writing the note again",
    ] {
        assert!(notice.contains(said), "{said}: {notice}");
    }
    // The correction is the one that may say what the draft holds: the
    // file, the line in it, the parser's message and the call to make.
    for said in [
        "Your draft file `/home/u/.staged/sessions/sess-1/draft.md` holds the note you saved",
        "starting at line 12 of the note body in that file, counting from the first non-blank \
         line after the `# ` title line",
        "syntax error near boxx",
        "call `write_note` again",
    ] {
        assert!(correction.contains(said), "{said}: {correction}");
    }

    // Only a note run with the tool is asked for the repair. The recovery
    // runs only once the Staged MCP on Blox is installed, so the other two
    // states do not occur; they keep their own prompt all the same.
    assert_eq!(
        first_turn_prompt(built.to_string(), false, false, Some((&rejected, draft))),
        built
    );
    assert_eq!(
        first_turn_prompt(built.to_string(), true, false, Some((&rejected, draft))),
        first_turn_prompt(built.to_string(), true, false, None)
    );
}

/// Which recoveries hand the first prompt a note to repair, and that such a
/// recovery still counts as having settled the outbox for the end-of-run
/// clear.
#[test]
fn only_a_rejected_recovery_carries_a_note_to_the_first_prompt() {
    let rejected = Ok(RunStartRecovery {
        outbox: OutboxRecovery::Rejected(rejected_note()),
        draft: DraftSeed::Kept,
    });
    assert_eq!(note_rejected_by_recovery(&rejected), Some(rejected_note()));
    assert!(remote_outbox_recovery_settled_for_run("sess-1", rejected));

    for outbox in [
        OutboxRecovery::Empty,
        OutboxRecovery::Ingested,
        OutboxRecovery::Discarded,
    ] {
        let recovery = Ok(RunStartRecovery {
            outbox,
            draft: DraftSeed::Absent,
        });
        assert_eq!(note_rejected_by_recovery(&recovery), None, "{recovery:?}");
    }
    assert_eq!(
        note_rejected_by_recovery(&Err("workstation suspended".to_string())),
        None
    );
}
