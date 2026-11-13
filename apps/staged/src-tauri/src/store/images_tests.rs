//! Tests for the image-reference checks in `images.rs`: what counts as a
//! reference, that the batched pass answers exactly what the per-image checks
//! answer, that the chat statements read the partial index from migration
//! 0049 instead of scanning `session_messages`, and that a session's
//! attachment lookup returns what its one-statement form returned.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{params, Connection};

use super::images::{
    take_reference_passes, ReferencePasses, CHAT_ATTACHMENTS_SQL, CHAT_REFERENCE_SQL,
};
use super::models::*;
use super::Store;

const IMAGE_IDS_INDEX: &str = "idx_session_messages_image_ids";

/// The `detail` column of each `EXPLAIN QUERY PLAN` row for `sql`, in plan
/// order.
fn query_plan(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let details = stmt
        .query_map(params, |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    details
}

/// Assert the chat reference checks read `session_messages` only through the
/// partial index, and as a covering index, so they never touch a table row:
/// the statement the batched pass runs, and the per-image statement the tests
/// hold it to.
///
/// Plan details read `SCAN <table> ...` or `SEARCH <table> ...`; the table is
/// matched as a whole word because `queued_session_messages` contains the
/// other table's name.
pub(super) fn assert_chat_reference_check_reads_the_index(conn: &Connection) {
    let statements: [(&str, &[&dyn rusqlite::ToSql]); 2] = [
        (CHAT_ATTACHMENTS_SQL[0], params![]),
        (CHAT_REFERENCE_SQL, params!["\"some-image\""]),
    ];
    for (sql, params) in statements {
        let plan = query_plan(conn, sql, params);
        let message_steps: Vec<&str> = plan
            .iter()
            .map(String::as_str)
            .filter(|detail| detail.split_whitespace().nth(1) == Some("session_messages"))
            .collect();
        assert_eq!(
            message_steps.len(),
            1,
            "expected one step over session_messages in {plan:?} for {sql}"
        );
        assert!(
            message_steps[0].contains(&format!("USING COVERING INDEX {IMAGE_IDS_INDEX}")),
            "the chat reference check should read {IMAGE_IDS_INDEX}, not scan \
             session_messages; plan: {plan:?} for {sql}"
        );
    }
}

fn running_session(store: &Store) -> Session {
    let session = Session::new_running("work", Path::new("/tmp"));
    store.create_session(&session).unwrap();
    session
}

#[test]
fn test_chat_reference_check_reads_the_image_ids_index() {
    let store = Store::in_memory().unwrap();
    let conn = store.conn.lock().unwrap();
    // The app never runs ANALYZE, so this is the plan a real store gets.
    assert_chat_reference_check_reads_the_index(&conn);
}

#[test]
fn test_chat_reference_check_keeps_the_index_once_the_table_has_statistics() {
    let store = Store::in_memory().unwrap();
    let session = running_session(&store);
    // Mostly messages without attachments, as in a real transcript.
    for n in 0..600 {
        let images: Vec<String> = if n % 100 == 0 {
            vec![format!("image-{n}")]
        } else {
            Vec::new()
        };
        store
            .add_session_message_with_images(
                &session.id,
                MessageRole::User,
                "a message long enough to give the table wider rows than the index",
                &images,
            )
            .unwrap();
    }

    let conn = store.conn.lock().unwrap();
    conn.execute_batch("ANALYZE;").unwrap();
    assert_chat_reference_check_reads_the_index(&conn);

    // Only the attaching messages are in the index.
    let indexed: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM session_messages INDEXED BY {IMAGE_IDS_INDEX}
                 WHERE image_ids IS NOT NULL"
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 6);
}

#[test]
fn test_image_has_chat_references_matches_sent_and_queued_attachments_only() {
    let store = Store::in_memory().unwrap();
    let session = running_session(&store);

    let sent = "11111111-aaaa-4aaa-8aaa-000000000001".to_string();
    let also_sent = "11111111-aaaa-4aaa-8aaa-000000000002".to_string();
    let queued = "22222222-bbbb-4bbb-8bbb-000000000001".to_string();
    let mentioned = "33333333-cccc-4ccc-8ccc-000000000001";

    store
        .add_session_message_with_images(
            &session.id,
            MessageRole::User,
            "two attachments",
            &[sent.clone(), also_sent.clone()],
        )
        .unwrap();
    // No attachments: `image_ids` is NULL, and naming an id in the text is
    // not a reference.
    store
        .add_session_message(&session.id, MessageRole::User, mentioned)
        .unwrap();
    store
        .add_queued_session_message(&session.id, "later", std::slice::from_ref(&queued), None)
        .unwrap();

    assert!(store.image_has_chat_references(&sent).unwrap());
    assert!(store.image_has_chat_references(&also_sent).unwrap());
    assert!(store.image_has_chat_references(&queued).unwrap());
    assert!(!store.image_has_chat_references(mentioned).unwrap());
    // The needle is the quoted id, so part of an attached id does not match.
    assert!(!store
        .image_has_chat_references(&sent[..sent.len() - 1])
        .unwrap());
    assert!(!store
        .image_has_chat_references("44444444-dddd-4ddd-8ddd-000000000001")
        .unwrap());
}

/// A store holding one of everything a reference check has to tell apart,
/// the candidate ids to ask about, and the notes a caller might leave out.
struct References {
    store: Store,
    candidates: Vec<String>,
    /// `None`, every branch and project note, and an id that names no note.
    exclusions: Vec<Option<String>>,
    only_one: String,
    in_two: String,
    in_project: String,
    in_example: String,
    in_blob: String,
    sent: String,
    queued: String,
    note_and_chat: String,
    unused: String,
    bare: String,
    mentioned: String,
    owner: String,
    blob_note: String,
}

fn references() -> References {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test/references");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let session = running_session(&store);

    let id = |n: u32| format!("aaaaaaaa-0000-4000-8000-{n:012}");
    let embed = |id: &str| format!("![shot](staged-media://{id}.png)");
    let only_one = id(1);
    let in_two = id(2);
    let in_project = id(3);
    let in_example = id(4);
    let in_blob = id(5);
    let sent = id(6);
    let queued = id(7);
    let note_and_chat = id(8);
    let unused = id(9);
    let bare = id(10);
    let mentioned = id(11);

    // Embeds three ids, names `mentioned` without the scheme, and ends on a
    // reference with no extension: neither of those two is a reference.
    let owner = Note::new_standalone(
        &branch.id,
        "Owner",
        &format!(
            "{} then {} and {}\n\nsee {mentioned}.png and staged-media://{bare}",
            embed(&only_one),
            embed(&in_two),
            embed(&note_and_chat),
        ),
    );
    let sharer = Note::new_standalone(&branch.id, "Sharer", &embed(&in_two));
    // The conservative rule: a reference inside an example still counts.
    let example = Note::new_standalone(
        &branch.id,
        "Example",
        &format!("```md\n{}\n```", embed(&in_example)),
    );
    // No media at all, so the pass never reads it.
    let plain = Note::new_standalone(&branch.id, "Plain", "nothing embedded");
    // A body that is not valid UTF-8, stored as a BLOB below.
    let blob_note = Note::new_standalone(&branch.id, "Blob", "placeholder");
    for note in [&owner, &sharer, &example, &plain, &blob_note] {
        store.create_note(note).unwrap();
    }
    let project_note = ProjectNote::new(&project.id, "Plan", &embed(&in_project));
    store.create_project_note(&project_note).unwrap();

    store
        .add_session_message_with_images(
            &session.id,
            MessageRole::User,
            "two attachments",
            &[sent.clone(), note_and_chat.clone()],
        )
        .unwrap();
    // Naming an id in a message's text attaches nothing.
    store
        .add_session_message(&session.id, MessageRole::User, &mentioned)
        .unwrap();
    store
        .add_queued_session_message(&session.id, "later", std::slice::from_ref(&queued), None)
        .unwrap();
    {
        let conn = store.conn.lock().unwrap();
        let mut body = vec![0xff, 0xfe];
        body.extend_from_slice(embed(&in_blob).as_bytes());
        conn.execute(
            "UPDATE notes SET content = ?1 WHERE id = ?2",
            params![body, blob_note.id],
        )
        .unwrap();
    }

    let mut candidates = vec![
        only_one.clone(),
        in_two.clone(),
        in_project.clone(),
        in_example.clone(),
        in_blob.clone(),
        sent.clone(),
        queued.clone(),
        note_and_chat.clone(),
        unused.clone(),
        bare.clone(),
        mentioned.clone(),
    ];
    // Part of an id that is embedded and attached, and an id one of them is
    // part of: the needles end at the id's delimiter, so neither matches.
    for whole in [&only_one, &sent, &note_and_chat] {
        candidates.push(whole[..whole.len() - 1].to_string());
        candidates.push(whole[1..].to_string());
        candidates.push(format!("{whole}0"));
    }
    let exclusions = [None, Some("no-such-note".to_string())]
        .into_iter()
        .chain(
            [
                &owner.id,
                &sharer.id,
                &example.id,
                &plain.id,
                &blob_note.id,
                &project_note.id,
            ]
            .map(|id| Some(id.clone())),
        )
        .collect();

    References {
        store,
        candidates,
        exclusions,
        only_one,
        in_two,
        in_project,
        in_example,
        in_blob,
        sent,
        queued,
        note_and_chat,
        unused,
        bare,
        mentioned,
        owner: owner.id,
        blob_note: blob_note.id,
    }
}

/// The batched pass must answer, for every candidate and every note left
/// out, exactly what the per-image checks answer one id at a time.
#[test]
fn test_media_references_match_the_per_image_checks() {
    let f = references();
    let ids = || f.candidates.iter().map(String::as_str);
    let conn = f.store.conn.lock().unwrap();
    let (mut in_notes, mut in_chat) = (0, 0);
    for except in &f.exclusions {
        let except = except.as_deref();
        let batched = Store::media_references_on(&conn, ids(), except).unwrap();
        for id in ids() {
            let by_notes = Store::referenced_by_notes(&conn, id, except).unwrap();
            let by_chat = Store::referenced_by_chat(&conn, id).unwrap();
            assert_eq!(
                batched.in_notes(id),
                by_notes,
                "notes: {id} leaving out {except:?}"
            );
            assert_eq!(
                batched.in_chat(id),
                by_chat,
                "chat: {id} leaving out {except:?}"
            );
            assert_eq!(batched.unused(id), !by_notes && !by_chat);
            in_notes += usize::from(by_notes);
            in_chat += usize::from(by_chat);

            // An id's answer does not depend on what else was asked about.
            let alone = Store::media_references_on(&conn, [id], except).unwrap();
            assert_eq!(
                (alone.in_notes(id), alone.in_chat(id)),
                (by_notes, by_chat),
                "alone: {id} leaving out {except:?}"
            );
        }
    }
    // The comparison saw both answers from both halves.
    let asked = f.exclusions.len() * f.candidates.len();
    assert!(0 < in_notes && in_notes < asked);
    assert!(0 < in_chat && in_chat < asked);
}

/// The same comparison through the locking wrappers the callers use.
#[test]
fn test_media_reference_wrappers_match_the_per_image_wrappers() {
    let f = references();
    let ids = || f.candidates.iter().map(String::as_str);

    let all = f.store.media_references(ids(), None).unwrap();
    let embedded = f.store.media_embedded_in_notes(ids()).unwrap();
    for id in ids() {
        let referenced = f.store.media_is_referenced(id).unwrap();
        assert_eq!(all.in_notes(id), referenced, "{id}");
        assert_eq!(embedded.contains(id), referenced, "{id}");
        assert_eq!(
            all.in_chat(id),
            f.store.image_has_chat_references(id).unwrap(),
            "{id}"
        );
    }
    for note_id in f.exclusions.iter().flatten() {
        let except = f.store.media_references(ids(), Some(note_id)).unwrap();
        for id in ids() {
            assert_eq!(
                except.unused(id),
                f.store.media_is_exclusive_to(id, note_id).unwrap(),
                "{id} leaving out {note_id}"
            );
        }
    }
}

/// What the fixture is meant to exercise, spelled out, so the comparison
/// above cannot pass on a store where nothing matches.
#[test]
fn test_media_references_tell_users_apart() {
    let f = references();
    let ids = || f.candidates.iter().map(String::as_str);
    let set = |ids: &[&String]| -> HashSet<String> { ids.iter().map(|id| (*id).clone()).collect() };
    let embedded = |references: &super::images::MediaReferences| -> HashSet<String> {
        ids()
            .filter(|id| references.in_notes(id))
            .map(str::to_owned)
            .collect()
    };
    let attached = |references: &super::images::MediaReferences| -> HashSet<String> {
        ids()
            .filter(|id| references.in_chat(id))
            .map(str::to_owned)
            .collect()
    };

    let all = f.store.media_references(ids(), None).unwrap();
    assert_eq!(
        embedded(&all),
        set(&[
            &f.only_one,
            &f.in_two,
            &f.in_project,
            &f.in_example,
            &f.in_blob,
            &f.note_and_chat,
        ])
    );
    assert_eq!(attached(&all), set(&[&f.sent, &f.queued, &f.note_and_chat]));
    for id in [&f.unused, &f.bare, &f.mentioned] {
        assert!(all.unused(id), "{id}");
    }

    // Leaving the owner out frees what only it embeds; media another note
    // shares, or a chat message attaches, is still in use.
    let without_owner = f.store.media_references(ids(), Some(&f.owner)).unwrap();
    assert_eq!(
        embedded(&without_owner),
        set(&[&f.in_two, &f.in_project, &f.in_example, &f.in_blob])
    );
    assert!(without_owner.unused(&f.only_one));
    assert!(!without_owner.unused(&f.in_two));
    assert!(!without_owner.unused(&f.note_and_chat));
    assert_eq!(attached(&without_owner), attached(&all));

    let without_blob = f.store.media_references(ids(), Some(&f.blob_note)).unwrap();
    assert!(without_blob.unused(&f.in_blob));
}

#[test]
fn test_media_references_take_one_pass_however_many_ids_are_asked_about() {
    let f = references();
    let ids = || f.candidates.iter().map(String::as_str);
    take_reference_passes();

    f.store.media_references(ids(), Some(&f.owner)).unwrap();
    assert_eq!(
        take_reference_passes(),
        ReferencePasses { notes: 1, chat: 1 }
    );

    f.store.media_embedded_in_notes(ids()).unwrap();
    assert_eq!(
        take_reference_passes(),
        ReferencePasses { notes: 1, chat: 0 }
    );

    // Nothing to ask about: no pass.
    let none = std::iter::empty::<&str>;
    assert!(f
        .store
        .media_references(none(), None)
        .unwrap()
        .unused("any"));
    assert!(f.store.media_embedded_in_notes(none()).unwrap().is_empty());
    assert_eq!(take_reference_passes(), ReferencePasses::default());
}

/// A store holding one of every kind of row the session attachment lookup
/// has to tell apart: linked to the session or not, an attachment's mime type
/// or not, embedded by a branch note, a project note, both or neither.
struct Attachments {
    store: Store,
    project: String,
    session: String,
    other_session: String,
    /// What the lookup should return for `session`, in the order the rows
    /// were inserted.
    expected: Vec<String>,
    /// What it should return for `other_session`.
    other_expected: Vec<String>,
    /// What it should return for the pending-session marker.
    pending_expected: Vec<String>,
}

fn attachments() -> Attachments {
    let store = Store::in_memory().unwrap();
    let project = Project::new("test/attachments");
    store.create_project(&project).unwrap();
    let branch = Branch::new(&project.id, "feature", "main");
    store.create_branch(&branch).unwrap();
    let queued = Session::new_queued("work");
    store.create_session(&queued).unwrap();
    let other = running_session(&store);

    // Ids are handed out high to low, so the order the rows are inserted in
    // is the reverse of the order of their ids.
    let next = std::cell::Cell::new(900u32);
    let add = |session: Option<&str>, mime: &str| -> String {
        let n = next.replace(next.get() - 1);
        let mut image = Image::new(Some(&branch.id), &project.id, "file.bin", mime, 1, false);
        image.id = format!("bbbbbbbb-0000-4000-8000-{n:012}");
        image.session_id = session.map(str::to_owned);
        store.create_image(&image).unwrap();
        image.id
    };
    let mine = Some(queued.id.as_str());
    let theirs = Some(other.id.as_str());
    let embed = |id: &str| format!("![shot](staged-media://{id}.png)");

    // Unreferenced attachments: returned.
    let plain = add(mine, "image/png");
    let pdf = add(mine, "application/pdf");
    // `LIKE` ignores ASCII case; `=` does not.
    let upper_image = add(mine, "IMAGE/JPEG");
    add(mine, "application/PDF");
    // Referenced by a branch note, a project note, or both: dropped.
    let in_note = add(mine, "image/jpeg");
    let in_project = add(mine, "image/png");
    let in_both = add(mine, "image/webp");
    let pdf_in_note = add(mine, "application/pdf");
    // The conservative rule: a reference inside an example still counts, and
    // so does one in a body that is not valid UTF-8.
    let in_example = add(mine, "image/png");
    let in_blob = add(mine, "image/png");
    // Named in a note without the scheme, or with no extension: not embedded.
    let mentioned = add(mine, "image/png");
    let bare = add(mine, "image/png");
    // An id that an embedded id is part of, or is part of an embedded id.
    let longer = format!("{in_note}0");
    let shorter = in_note[1..].to_string();
    for id in [&longer, &shorter] {
        let mut image = Image::new(None, &project.id, "file.bin", "image/png", 1, false);
        image.id = id.clone();
        image.session_id = Some(queued.id.clone());
        store.create_image(&image).unwrap();
    }
    // Not an attachment's mime type, embedded or not: never returned.
    add(mine, "video/mp4");
    let video_in_note = add(mine, "video/quicktime");
    add(mine, "text/plain");
    add(mine, "image");
    // Linked to another session, to none, or still pending.
    let theirs_plain = add(theirs, "image/png");
    let theirs_in_note = add(theirs, "image/png");
    add(theirs, "video/mp4");
    add(None, "image/png");
    let pending = add(Some(PENDING_SESSION_ID), "image/png");
    add(Some(NOTE_MEDIA_SESSION_ID), "video/mp4");

    let notes = [
        Note::new_standalone(
            &branch.id,
            "Owner",
            &format!(
                "{} {} {} {} {}\n\nsee {mentioned}.png and staged-media://{bare}",
                embed(&in_note),
                embed(&in_both),
                embed(&pdf_in_note),
                embed(&video_in_note),
                embed(&theirs_in_note),
            ),
        ),
        Note::new_standalone(
            &branch.id,
            "Example",
            &format!("```md\n{}\n```", embed(&in_example)),
        ),
        Note::new_standalone(&branch.id, "Plain", "nothing embedded"),
    ];
    for note in &notes {
        store.create_note(note).unwrap();
    }
    let blob_note = Note::new_standalone(&branch.id, "Blob", "placeholder");
    store.create_note(&blob_note).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        let mut body = vec![0xff, 0xfe];
        body.extend_from_slice(embed(&in_blob).as_bytes());
        conn.execute(
            "UPDATE notes SET content = ?1 WHERE id = ?2",
            params![body, blob_note.id],
        )
        .unwrap();
    }
    let project_note = ProjectNote::new(
        &project.id,
        "Plan",
        &format!("{} {}", embed(&in_project), embed(&in_both)),
    );
    store.create_project_note(&project_note).unwrap();

    // A chat message attaching a row is what makes it an attachment, and is
    // no reason to leave it out: the lookup asks the notes, never the chat.
    store
        .add_session_message_with_images(
            &queued.id,
            MessageRole::User,
            "three attachments",
            &[plain.clone(), pdf.clone(), in_note.clone()],
        )
        .unwrap();

    Attachments {
        store,
        project: project.id,
        session: queued.id,
        other_session: other.id,
        expected: vec![plain, pdf, upper_image, mentioned, bare, longer, shorter],
        other_expected: vec![theirs_plain],
        pending_expected: vec![pending],
    }
}

/// The lookup must return, for every session, exactly what its one-statement
/// form returned, in the same order.
#[test]
fn test_session_attachments_match_the_per_image_query() {
    let f = attachments();
    for session_id in [
        f.session.as_str(),
        f.other_session.as_str(),
        PENDING_SESSION_ID,
        NOTE_MEDIA_SESSION_ID,
        "no-such-session",
        "",
    ] {
        assert_eq!(
            f.store.get_image_ids_for_session(session_id).unwrap(),
            f.store
                .get_image_ids_for_session_per_image(session_id)
                .unwrap(),
            "session {session_id:?}"
        );
    }

    // What the fixture is meant to exercise, spelled out, so the comparison
    // cannot pass on a store where both forms return nothing.
    let lookup = |session_id: &str| f.store.get_image_ids_for_session(session_id).unwrap();
    assert_eq!(lookup(&f.session), f.expected);
    assert_eq!(lookup(&f.other_session), f.other_expected);
    assert_eq!(lookup(PENDING_SESSION_ID), f.pending_expected);
    assert!(lookup(NOTE_MEDIA_SESSION_ID).is_empty());
    assert!(lookup("no-such-session").is_empty());
}

/// The answer follows the notes: an attachment a note starts to embed drops
/// out, and comes back when the note stops, in both forms alike.
#[test]
fn test_session_attachments_follow_the_notes_like_the_per_image_query() {
    let f = attachments();
    let both = |store: &Store| {
        let ids = store.get_image_ids_for_session(&f.session).unwrap();
        assert_eq!(
            ids,
            store
                .get_image_ids_for_session_per_image(&f.session)
                .unwrap()
        );
        ids
    };
    let embedded = f.expected[0].clone();
    let note = ProjectNote::new(
        &f.project,
        "Later",
        &format!("![later](staged-media://{embedded}.png)"),
    );
    f.store.create_project_note(&note).unwrap();
    assert_eq!(both(&f.store), f.expected[1..]);

    f.store.delete_project_note(&note.id).unwrap();
    assert_eq!(both(&f.store), f.expected);
}

/// One pass over the note bodies for the whole set, none over the chat
/// attachment lists, and no pass at all for a session with no attachment.
#[test]
fn test_session_attachments_take_one_note_pass_however_many_are_attached() {
    let f = attachments();
    take_reference_passes();

    assert_eq!(
        f.store.get_image_ids_for_session(&f.session).unwrap().len(),
        f.expected.len()
    );
    assert_eq!(
        take_reference_passes(),
        ReferencePasses { notes: 1, chat: 0 }
    );

    // Only a video is linked to the note-media marker, and nothing to an
    // unknown session: no attachment, so nothing to ask the notes about.
    for session_id in [NOTE_MEDIA_SESSION_ID, "no-such-session"] {
        assert!(f
            .store
            .get_image_ids_for_session(session_id)
            .unwrap()
            .is_empty());
    }
    assert_eq!(take_reference_passes(), ReferencePasses::default());
}
