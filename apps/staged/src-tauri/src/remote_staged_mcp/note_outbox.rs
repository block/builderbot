//! The note outbox: what `write_note` on the Staged MCP on Blox leaves on the
//! workstation, and the laptop's collector for it.
//!
//! A remote note session gets the same `write_note` tool the local ones do,
//! served by the Staged MCP on Blox (see the parent module). Each call writes
//! the note atomically into the outbox, the session's scratch directory under
//! the workstation home:
//!
//! ```text
//! ~/.staged/sessions/<session-id>/draft.md  the agent's draft: the note body it
//!                                           writes and edits; `write_note` reads it
//! ~/.staged/sessions/<session-id>/note.md   the published body (markdown, no title)
//! ~/.staged/sessions/<session-id>/meta.json title, suggested next steps, turn id
//! ~/.staged/sessions/<session-id>/done      marker: a complete note is present
//! ```
//!
//! The draft is the laptop's to seed and clear (`note_draft`): the
//! start-of-run recovery's clear seeds it with the stored body for an
//! amendment, in the same round trip ([`clear_outbox_and_seed`]), and every
//! clear removes it with the outbox, along with `.draft.md.seed`, the temp
//! file a seed is decoded into before it is renamed into place.
//!
//! Only `note.md` beside a `done` marker is ever read as a note. The draft
//! is never collected: whatever state a lost run left it in, nothing is
//! stored without a `write_note` call.
//!
//! After the turn — or on the dead-owner sweep, when the laptop lost the
//! session — the collector pulls the outbox over `ws exec` and turns it into
//! the same [`NoteSubmission`] the local tool produces, so the post-completion
//! hooks (title resolution, media ingestion) do not know which transport
//! served the call.

use serde::Deserialize;

use super::{session_dir_expr, validate_session_id, RemoteExec};
use crate::note_tool::{
    NoteSubmission, NoteSubmissionSlot, NoteSubmissionSource, SuggestedNextSteps, WriteNoteParams,
};

/// Empty the session's outbox and remove its directory once it is empty.
/// Runs after a note has been collected *and* ingested, after a run read the
/// outbox once its last turn had ended, or after a Stop the user asked for
/// (see `session_runner`'s `remote_outbox_clear_at_run_end`) — never blind:
/// not at session start, where a `done` marker may be a finished note the
/// laptop lost before it could read it (see [`recover_uncollected_outbox`]),
/// and not at the end of a run that did not read the outbox, which uses
/// [`clear_unpublished_outbox`] instead. The `rmdir` is
/// best-effort: a directory `write_note` is still writing into, or one
/// holding another tool's files, is left alone.
/// Always exits 0.
pub(crate) fn clear_outbox_script(session_id: &str) -> String {
    let dir = session_dir_expr(session_id);
    format!(
        "d={dir}; {remove}; rmdir \"$d\" 2>/dev/null; exit 0",
        remove = remove_session_files_fragment()
    )
}

/// The temp file a seed is decoded into, beside the draft, before it is
/// renamed to `draft.md`.
fn seed_temp_file_name() -> String {
    format!(".{}.seed", crate::note_draft::REMOTE_DRAFT_FILE_NAME)
}

/// The `rm -f` every clear script starts with: the outbox's three files, the
/// draft, and the seed's temp file (a chunked seed that failed part-way
/// leaves one, and the session directory could never be removed around it).
/// Expects `$d` to be the session directory.
fn remove_session_files_fragment() -> String {
    format!(
        "rm -f \"$d/note.md\" \"$d/meta.json\" \"$d/done\" \"$d/{draft}\" \"$d/{seed}\"",
        draft = crate::note_draft::REMOTE_DRAFT_FILE_NAME,
        seed = seed_temp_file_name()
    )
}

pub(crate) fn clear_outbox(exec: RemoteExec, session_id: &str) -> Result<(), String> {
    validate_session_id(session_id)?;
    exec(&["sh", "-c", &clear_outbox_script(session_id)])
        .map(|_| ())
        .map_err(|e| format!("failed to clear the remote note outbox: {e}"))
}

/// What [`clear_unpublished_outbox_script`] prints when it found a `done`
/// marker and removed nothing.
const PUBLISHED_NOTE_KEPT_MARKER: &str = "staged-published-note-kept";

/// [`clear_outbox_script`] for a caller that has not read the outbox since
/// the agent last could have written to it: the files are removed only when
/// no `done` marker is there. A marker means a published note nobody has
/// collected, and then nothing is touched — not the note, and not the draft
/// it was published from — so the next reader (the start-of-run recovery of
/// a Resume) finds it whole. The test and the removal are one command on
/// the workstation, so there is no window between a laptop-side read and
/// the clear. Always exits 0.
pub(crate) fn clear_unpublished_outbox_script(session_id: &str) -> String {
    let dir = session_dir_expr(session_id);
    format!(
        "d={dir}; if [ -f \"$d/done\" ]; then echo {PUBLISHED_NOTE_KEPT_MARKER}; exit 0; fi; \
         {remove}; rmdir \"$d\" 2>/dev/null; exit 0",
        remove = remove_session_files_fragment()
    )
}

/// What [`clear_unpublished_outbox`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnreadOutboxClear {
    /// No published note was there; the partial files and the draft are gone.
    Cleared,
    /// A `done` marker was there: a published note no run has collected.
    /// Everything was left in place.
    PublishedNoteKept,
}

/// Empty the session's outbox unless it holds a published note (see
/// [`clear_unpublished_outbox_script`]).
pub(crate) fn clear_unpublished_outbox(
    exec: RemoteExec,
    session_id: &str,
) -> Result<UnreadOutboxClear, String> {
    validate_session_id(session_id)?;
    let output = exec(&["sh", "-c", &clear_unpublished_outbox_script(session_id)])
        .map_err(|e| format!("failed to clear the remote note outbox: {e}"))?;
    Ok(
        if output
            .lines()
            .any(|line| line.trim() == PUBLISHED_NOTE_KEPT_MARKER)
        {
            UnreadOutboxClear::PublishedNoteKept
        } else {
            UnreadOutboxClear::Cleared
        },
    )
}

/// Largest base64 payload put on one command line, the same bound
/// `session_commands::write_bytes_to_remote_with` uses to stay under ARG_MAX.
const SEED_CHUNK_SIZE: usize = 500_000;

/// The line the seeding clear prints when the outbox was emptied but the
/// seed could not be written. It exits 0 either way, so a failed command
/// means the clear itself may not have run.
const SEED_FAILED_MARKER: &str = "staged-draft-seed-failed";

/// How `draft.md` came out of the run-start clear. Kept apart from whether
/// the outbox was settled: by the time a seed fails, the collect, the ingest
/// and the clear's `rm -f` have all happened, so the run has observed the
/// outbox and its end-of-run clear policy must not change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftSeed {
    /// A fresh note: no draft was written and any stale one is gone.
    Absent,
    /// An amendment: `draft.md` holds the stored body.
    Seeded,
    /// An amendment whose seed could not be written. The outbox is cleared
    /// and there is no `draft.md`; `write_note` reports the missing draft
    /// by path and the agent writes the file.
    Failed(String),
}

/// The run-start form of [`clear_outbox_script`]: one round trip that
/// empties the outbox and puts `draft.md` in its run-start state.
///
/// The outbox files and any stale draft go, and the session directory is
/// left in place (created if need be, mode 700) so the agent's first write
/// of `draft.md` needs no `mkdir` — not every agent's file tool creates
/// parent directories. With `seed_b64` (an amendment's stored body, base64)
/// the seed is decoded into a temp file and renamed into place; a seed that
/// fails removes its temp file and prints [`SEED_FAILED_MARKER`]. Without
/// one (a fresh note) no file is created and the `mkdir` is best-effort.
/// Always exits 0, like the plain clear, so the caller can tell a failed
/// seed (the marker) from a command that never ran (an error).
pub(crate) fn clear_outbox_and_seed_script(session_id: &str, seed_b64: Option<&str>) -> String {
    let dir = session_dir_expr(session_id);
    let draft = crate::note_draft::REMOTE_DRAFT_FILE_NAME;
    let remove = remove_session_files_fragment();
    let Some(seed) = seed_b64 else {
        return format!(
            "d={dir}; {remove}; \
             mkdir -p \"$d\" 2>/dev/null && chmod 700 \"$d\" 2>/dev/null; exit 0"
        );
    };
    let temp = seed_temp_file_name();
    format!(
        "d={dir}; {remove}; \
         mkdir -p \"$d\" && chmod 700 \"$d\" && \
         echo '{seed}' | base64 -d > \"$d/{temp}\" && \
         mv -f \"$d/{temp}\" \"$d/{draft}\" || \
         {{ rm -f \"$d/{temp}\"; echo {SEED_FAILED_MARKER}; }}; exit 0"
    )
}

/// Clear the outbox at run start and, for an amendment, seed the draft with
/// `seed` (the stored note body). One round trip, as [`clear_outbox`] is,
/// for a fresh note and for a seed that fits one command line; a larger
/// seed is appended in [`SEED_CHUNK_SIZE`] base64 chunks after the clear.
///
/// `Err` means the clear itself failed or could not be confirmed (the
/// workstation was not reached), so the outbox is in an unknown state. A
/// seed that failed after the clear is `Ok(DraftSeed::Failed)`.
pub(crate) fn clear_outbox_and_seed(
    exec: RemoteExec,
    session_id: &str,
    seed: Option<&[u8]>,
) -> Result<DraftSeed, String> {
    use base64::Engine;
    validate_session_id(session_id)?;
    let Some(seed) = seed else {
        return exec(&["sh", "-c", &clear_outbox_and_seed_script(session_id, None)])
            .map(|_| DraftSeed::Absent)
            .map_err(|e| format!("failed to clear the remote note outbox: {e}"));
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(seed);
    if encoded.len() <= SEED_CHUNK_SIZE {
        let output = exec(&[
            "sh",
            "-c",
            &clear_outbox_and_seed_script(session_id, Some(&encoded)),
        ])
        .map_err(|e| format!("failed to clear the remote note outbox and seed the draft: {e}"))?;
        return Ok(
            if output.lines().any(|line| line.trim() == SEED_FAILED_MARKER) {
                DraftSeed::Failed(
                    "the workstation could not write the seed into the session directory"
                        .to_string(),
                )
            } else {
                DraftSeed::Seeded
            },
        );
    }
    clear_outbox(exec, session_id)?;
    let dir = session_dir_expr(session_id);
    let draft = crate::note_draft::REMOTE_DRAFT_FILE_NAME;
    let temp = seed_temp_file_name();
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(SEED_CHUNK_SIZE)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
        .collect();
    let last = chunks.len() - 1;
    for (i, chunk) in chunks.into_iter().enumerate() {
        let redirect = if i == 0 { ">" } else { ">>" };
        let publish = if i == last {
            format!(" && mv -f \"$d/{temp}\" \"$d/{draft}\"")
        } else {
            String::new()
        };
        let script = format!(
            "d={dir}; mkdir -p \"$d\" && chmod 700 \"$d\" && \
             echo '{chunk}' | base64 -d {redirect} \"$d/{temp}\"{publish}"
        );
        // The outbox is already cleared: from here on a failure costs the
        // seed only. The temp file a failed chunk leaves goes with the next
        // clear.
        if let Err(e) = exec(&["sh", "-c", &script]) {
            return Ok(DraftSeed::Failed(format!(
                "failed to seed the remote note draft (chunk {i}): {e}"
            )));
        }
    }
    Ok(DraftSeed::Seeded)
}

/// Print the outbox as four lines: `done`/`pending`, then `meta.json` and
/// `note.md` each as one base64 line (empty when the file is missing), then
/// the `done`/`pending` state again. Base64 keeps arbitrary markdown —
/// including lines that look like our framing — out of the line protocol.
///
/// The marker is tested twice because the three reads are separate steps: a
/// rewrite that starts after the first test (`write_note` unlinks `done`, then
/// renames a new body and a new meta into place, then recreates `done`) can
/// hand this script the new body with the old title. The tool's own
/// ordering guarantees that `done` is absent for the whole of such a rewrite,
/// so a marker that is present both before and after the reads brackets a
/// title and body from the same call; [`parse_collect_output`] treats any
/// other combination as pending. Always exits 0.
pub(crate) fn collect_script(session_id: &str) -> String {
    let dir = session_dir_expr(session_id);
    format!(
        "d={dir}; \
         if [ -f \"$d/done\" ]; then echo done; else echo pending; fi; \
         if [ -f \"$d/meta.json\" ]; then base64 < \"$d/meta.json\" | tr -d '\\n'; fi; echo; \
         if [ -f \"$d/note.md\" ]; then base64 < \"$d/note.md\" | tr -d '\\n'; fi; echo; \
         if [ -f \"$d/done\" ]; then echo done; else echo pending; fi; \
         exit 0"
    )
}

/// `meta.json` as `write_note` writes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutboxMeta {
    pub title: String,
    #[serde(default)]
    pub suggested_next_steps: Option<SuggestedNextSteps>,
    /// The turn id the Staged MCP was started with; lets the collector tell this
    /// run's note from an earlier one.
    #[serde(default)]
    pub turn: Option<String>,
    #[serde(default)]
    pub written_at: Option<i64>,
    /// The script's `SERVER_VERSION`, under the key it has always been
    /// written with.
    #[serde(default, rename = "helperVersion")]
    pub server_version: Option<u32>,
}

/// The raw outbox contents as collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutboxSnapshot {
    pub done: bool,
    pub meta: Option<OutboxMeta>,
    pub body: Option<String>,
}

pub(crate) fn parse_collect_output(output: &str) -> Result<OutboxSnapshot, String> {
    use base64::Engine;
    let mut lines = output.lines().map(str::trim);
    let done_before = match lines.next() {
        Some("done") => true,
        Some("pending") => false,
        other => {
            return Err(format!(
                "unexpected outbox status line {other:?} in {output:?}"
            ))
        }
    };
    let meta_line = lines.next();
    let body_line = lines.next();
    // The marker re-read after both files (see `collect_script`). An older
    // script that printed only three lines is read as before; a marker that
    // changed state while the files were being read means the files may
    // straddle two calls, so the outbox counts as pending for this read.
    let done = match lines.next() {
        None => done_before,
        Some("done") => done_before,
        Some("pending") => false,
        other => {
            return Err(format!(
                "unexpected trailing outbox status line {other:?} in {output:?}"
            ))
        }
    };
    let decode = |line: Option<&str>, what: &str| -> Result<Option<Vec<u8>>, String> {
        match line.filter(|l| !l.is_empty()) {
            None => Ok(None),
            Some(encoded) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map(Some)
                .map_err(|e| format!("outbox {what} is not valid base64: {e}")),
        }
    };
    let meta = decode(meta_line, "meta.json")?
        .map(|bytes| {
            serde_json::from_slice::<OutboxMeta>(&bytes)
                .map_err(|e| format!("outbox meta.json does not parse: {e}"))
        })
        .transpose()?;
    let body = decode(body_line, "note.md")?
        .map(|bytes| {
            String::from_utf8(bytes).map_err(|e| format!("outbox note.md is not UTF-8: {e}"))
        })
        .transpose()?;
    Ok(OutboxSnapshot { done, meta, body })
}

/// What the collector found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutboxState {
    /// No `done` marker: the agent never completed a `write_note` call (or the
    /// outbox was cleared and nothing has been written since).
    Pending,
    /// A complete note from a different turn than the one asked about.
    StaleTurn { found: Option<String> },
    /// A complete note for this turn.
    Delivered(NoteSubmission),
}

/// Interpret a snapshot for `expected_turn` (`None` accepts any turn, which is
/// what the dead-owner sweep wants: it has no run of its own).
pub(crate) fn outbox_state(
    snapshot: OutboxSnapshot,
    expected_turn: Option<&str>,
) -> Result<OutboxState, String> {
    if !snapshot.done {
        return Ok(OutboxState::Pending);
    }
    let (Some(meta), Some(body)) = (snapshot.meta, snapshot.body) else {
        return Err("outbox is marked done but meta.json or note.md is missing".to_string());
    };
    if let Some(expected) = expected_turn {
        if meta.turn.as_deref() != Some(expected) {
            return Ok(OutboxState::StaleTurn { found: meta.turn });
        }
    }
    let submission = NoteSubmission::from_params(
        WriteNoteParams {
            title: meta.title,
            suggested_next_steps: meta.suggested_next_steps,
        },
        &body,
        NoteSubmissionSource::Outbox,
    )
    .map_err(|e| format!("outbox note is invalid: {e}"))?;
    Ok(OutboxState::Delivered(submission))
}

/// Why a collection failed: the two kinds call for different handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CollectError {
    /// The workstation could not be reached (or the session id is unusable).
    /// Nothing is known about the outbox; leave it for the next attempt.
    Transport(String),
    /// The workstation answered, but what it holds is not a usable note: a
    /// `done` marker next to a missing file, undecodable framing, or a body
    /// the tool itself would have rejected. Retrying reads the same bytes.
    Content(String),
}

impl CollectError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Transport(message) | Self::Content(message) => message,
        }
    }
}

impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// Pull the session's outbox from the workstation and interpret it, telling
/// a workstation that could not be reached from one that holds garbage.
pub(crate) fn collect_classified(
    exec: RemoteExec,
    session_id: &str,
    expected_turn: Option<&str>,
) -> Result<OutboxState, CollectError> {
    collect_with_written_at(exec, session_id, expected_turn).map(|(state, _)| state)
}

/// [`collect_classified`], also returning the `writtenAt` stamp of a complete
/// note (`write_note` writes whole seconds), for callers that need to tell one
/// `done` from the next ([`CollectedNote`]).
fn collect_with_written_at(
    exec: RemoteExec,
    session_id: &str,
    expected_turn: Option<&str>,
) -> Result<(OutboxState, Option<i64>), CollectError> {
    validate_session_id(session_id).map_err(CollectError::Transport)?;
    let output = exec(&["sh", "-c", &collect_script(session_id)]).map_err(|e| {
        CollectError::Transport(format!("failed to read the remote note outbox: {e}"))
    })?;
    let snapshot = parse_collect_output(&output).map_err(CollectError::Content)?;
    let written_at = snapshot.meta.as_ref().and_then(|meta| meta.written_at);
    let state = outbox_state(snapshot, expected_turn).map_err(CollectError::Content)?;
    Ok((state, written_at))
}

/// [`collect_classified`] for callers that treat every failure alike.
pub(crate) fn collect(
    exec: RemoteExec,
    session_id: &str,
    expected_turn: Option<&str>,
) -> Result<OutboxState, String> {
    collect_classified(exec, session_id, expected_turn).map_err(|e| e.message().to_string())
}

/// The note a run's per-turn collector last pulled from the outbox: the
/// `writtenAt` stamp `write_note` recorded and the submission itself. Enough
/// to recognise the same `done` on a later read.
///
/// One run shares one turn id across all its prompt turns (the Staged MCP on
/// Blox is attached once), and the outbox is cleared at the end of the run,
/// not between turns. So during a repair or follow-up turn the previous turn's
/// finished note is still on the workstation, marked `done` for this run's
/// turn id, and a probe that could not tell it from a fresh one would end
/// the live turn on a note already collected and rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollectedNote {
    pub written_at: Option<i64>,
    pub submission: NoteSubmission,
}

/// Shared memory of the last [`CollectedNote`] within one run.
pub(crate) type CollectedNoteMemory = std::sync::Arc<std::sync::Mutex<Option<CollectedNote>>>;

pub(crate) fn new_collected_note_memory() -> CollectedNoteMemory {
    std::sync::Arc::new(std::sync::Mutex::new(None))
}

/// Collect the outbox right after a turn settled and, when it holds this
/// run's note, record it in the run's [`NoteSubmissionSlot`] — the same slot
/// the local tool fills. From there the post-turn Pikchr validation, the
/// "did the turn deliver a note" check behind the `write_note` follow-up and
/// the post-completion hooks all read it, so one `ws exec` round trip serves
/// all three and remote notes get the checks local ones do. Anything other
/// than this turn's completed note leaves the slot untouched. The note is
/// also remembered in `memory` so a later probe can recognise it
/// ([`probe_turn_into_slot`]).
pub(crate) fn collect_turn_into_slot(
    exec: RemoteExec,
    session_id: &str,
    turn_id: &str,
    slot: &NoteSubmissionSlot,
    memory: &CollectedNoteMemory,
) -> Result<OutboxState, String> {
    let (state, written_at) = collect_with_written_at(exec, session_id, Some(turn_id))
        .map_err(|e| e.message().to_string())?;
    if let OutboxState::Delivered(submission) = &state {
        crate::note_tool::store_submission(slot, submission.clone());
        *memory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CollectedNote {
            written_at,
            submission: submission.clone(),
        });
    }
    Ok(state)
}

/// [`collect_turn_into_slot`] for a probe made *during* a turn (the inactivity
/// watchdog): a `done` identical to the note this run already collected — same
/// `writtenAt`, same title, body and next steps — is the previous turn's, not
/// a sign that the agent has finished this one, and is reported `Pending`
/// with the slot left alone. A note that differs in any of those is new and
/// is collected as usual.
pub(crate) fn probe_turn_into_slot(
    exec: RemoteExec,
    session_id: &str,
    turn_id: &str,
    slot: &NoteSubmissionSlot,
    memory: &CollectedNoteMemory,
) -> Result<OutboxState, String> {
    let (state, written_at) = collect_with_written_at(exec, session_id, Some(turn_id))
        .map_err(|e| e.message().to_string())?;
    let OutboxState::Delivered(submission) = state else {
        return Ok(state);
    };
    let found = CollectedNote {
        written_at,
        submission,
    };
    let mut remembered = memory
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if remembered.as_ref() == Some(&found) {
        log::info!(
            "Session {session_id}: the workstation outbox still holds the note \"{}\" this \
             run already collected; not a new delivery",
            found.submission.title
        );
        return Ok(OutboxState::Pending);
    }
    crate::note_tool::store_submission(slot, found.submission.clone());
    *remembered = Some(found.clone());
    Ok(OutboxState::Delivered(found.submission))
}

/// What [`recover_uncollected_outbox`] did at session start: what it found
/// in the outbox, and — separately — how the draft came out of the clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunStartRecovery {
    pub outbox: OutboxRecovery,
    pub draft: DraftSeed,
}

/// What [`recover_uncollected_outbox`] found at session start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboxRecovery {
    /// Nothing complete was waiting; whatever partial files were there are gone.
    Empty,
    /// A finished note from an earlier run was handed to `ingest`, then cleared.
    Ingested,
    /// The outbox held something marked `done` that is not a usable note (a
    /// file missing beside the marker, or a body the tool would reject). It
    /// was logged and cleared so the session is not stuck on it forever.
    Discarded,
}

/// Session start: a `done` marker already in the outbox is a note the agent
/// finished that the laptop never ingested — the previous run lost its
/// connection, the dead-owner sweep could not reach the workstation and the
/// row became `detached`, and now the user is resuming. Hand it to `ingest`
/// (the normal post-completion hooks, bound to the session's note), *then*
/// clear the outbox so the new run's collector cannot mistake it for its own.
/// With no complete note the outbox is just emptied of partial files.
///
/// The clear also puts the draft in its run-start state: `seed` is asked —
/// after any ingest, so it sees the note the ingest stored — for the stored
/// body to seed `draft.md` with (an amendment), or `None` to leave no draft
/// (a fresh note). A seed that fits one command line shares the clear's
/// round trip ([`clear_outbox_and_seed`]).
///
/// Two failures leave the outbox alone for the next attempt and are errors
/// here: a workstation that could not be reached, and a waiting note that
/// `ingest` reports it could not store (it returns `false`) — clearing after
/// either would delete the only copy. Content the collector cannot turn into
/// a note (a `done` marker beside a missing file, a body the tool would have
/// rejected) is cleared and reported as [`OutboxRecovery::Discarded`], since
/// every later resume would otherwise trip over the same bytes and the
/// session would be stuck on the `---` fallback for good.
///
/// After an error the draft is neither removed nor seeded: `draft.md` is
/// whatever the lost run left, possibly text it never published. That is
/// safe because the draft is never collected — only a `write_note` call in
/// the new run publishes it.
///
/// A seed that could not be written is not an error: the outbox was read,
/// any waiting note ingested and the files removed by then, so the result is
/// `Ok` with [`DraftSeed::Failed`] and the caller treats the outbox as
/// settled.
pub(crate) fn recover_uncollected_outbox(
    exec: RemoteExec,
    session_id: &str,
    ingest: &mut dyn FnMut(NoteSubmission) -> bool,
    seed: &mut dyn FnMut() -> Option<Vec<u8>>,
) -> Result<RunStartRecovery, String> {
    let outbox = match collect_classified(exec, session_id, None) {
        Ok(OutboxState::Delivered(submission)) => {
            log::warn!(
                "Session {session_id}: the workstation outbox holds an uncollected note \
                 \"{}\" from an earlier run; ingesting it before this run starts",
                submission.title
            );
            let title = submission.title.clone();
            if !ingest(submission) {
                return Err(format!(
                    "the uncollected note \"{title}\" could not be stored; leaving it in the \
                     outbox"
                ));
            }
            OutboxRecovery::Ingested
        }
        // `collect(.., None)` accepts any turn, so the stale arm cannot fire.
        Ok(OutboxState::Pending | OutboxState::StaleTurn { .. }) => OutboxRecovery::Empty,
        Err(CollectError::Content(message)) => {
            log::warn!(
                "Session {session_id}: the workstation outbox holds an unusable note \
                 ({message}); discarding it so this session can continue"
            );
            OutboxRecovery::Discarded
        }
        Err(CollectError::Transport(message)) => return Err(message),
    };
    let seed = seed();
    let draft = clear_outbox_and_seed(exec, session_id, seed.as_deref())?;
    Ok(RunStartRecovery { outbox, draft })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{local_exec, node, run_script};
    use super::*;
    use std::cell::RefCell;

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn collect_output_parses_status_and_base64_files() {
        let meta = r#"{"title":"Plan","suggestedNextSteps":{"suggestedNextCommitStep":"Fix it","suggestedNextNoteStep":null},"turn":"t1","writtenAt":1,"helperVersion":1}"#;
        let output = format!(
            "done\n{}\n{}\n",
            b64(meta.as_bytes()),
            b64("body\n---\nmore".as_bytes())
        );
        let snapshot = parse_collect_output(&output).unwrap();
        assert!(snapshot.done);
        let parsed_meta = snapshot.meta.unwrap();
        assert_eq!(parsed_meta.title, "Plan");
        assert_eq!(parsed_meta.turn.as_deref(), Some("t1"));
        assert_eq!(
            parsed_meta
                .suggested_next_steps
                .unwrap()
                .suggested_next_commit_step
                .as_deref(),
            Some("Fix it")
        );
        assert_eq!(snapshot.body.as_deref(), Some("body\n---\nmore"));

        let pending = parse_collect_output("pending\n\n\n").unwrap();
        assert_eq!(
            pending,
            OutboxSnapshot {
                done: false,
                meta: None,
                body: None
            }
        );
        assert!(parse_collect_output("").is_err());
        assert!(parse_collect_output("done\nnot base64!\n\n").is_err());
    }

    #[test]
    fn a_marker_that_changed_during_the_reads_makes_the_snapshot_pending() {
        let meta = br#"{"title":"T","turn":"t1"}"#;
        let four_lines = |after: &str| format!("done\n{}\n{}\n{after}\n", b64(meta), b64(b"B"));
        assert!(parse_collect_output(&four_lines("done")).unwrap().done);
        let straddled = parse_collect_output(&four_lines("pending")).unwrap();
        assert!(
            !straddled.done,
            "a rewrite started mid-read: not a complete note"
        );
        assert!(
            straddled.meta.is_some(),
            "the files themselves are still reported"
        );
        assert_eq!(
            outbox_state(straddled, Some("t1")).unwrap(),
            OutboxState::Pending
        );
        // The marker appearing only after the reads is the mirror image: the
        // first test saw no note, so nothing complete was read.
        let late = format!("pending\n{}\n{}\ndone\n", b64(meta), b64(b"B"));
        assert!(!parse_collect_output(&late).unwrap().done);
        assert!(parse_collect_output("done\n\n\nmaybe\n").is_err());
        assert_eq!(
            collect_script("sess-1")
                .matches("echo done; else echo pending")
                .count(),
            2,
            "the script tests the marker before and after the reads"
        );
    }

    #[test]
    fn collect_tells_an_unreachable_workstation_from_unusable_content() {
        let unreachable = |_: &[&str]| -> Result<String, String> { Err("suspended".into()) };
        assert!(matches!(
            collect_classified(&unreachable, "sess-1", None),
            Err(CollectError::Transport(message)) if message.contains("suspended")
        ));
        let marker_without_files =
            |_: &[&str]| -> Result<String, String> { Ok("done\n\n\n".into()) };
        assert!(matches!(
            collect_classified(&marker_without_files, "sess-1", None),
            Err(CollectError::Content(message)) if message.contains("missing")
        ));
        let empty_title = format!(
            "done\n{}\n{}\n",
            b64(br#"{"title":"  ","turn":"t1"}"#),
            b64(b"Body")
        );
        let rejected = move |_: &[&str]| -> Result<String, String> { Ok(empty_title.clone()) };
        assert!(matches!(
            collect_classified(&rejected, "sess-1", None),
            Err(CollectError::Content(_))
        ));
        assert!(matches!(
            collect_classified(&unreachable, "../etc", None),
            Err(CollectError::Transport(_))
        ));
    }

    fn done_snapshot(turn: Option<&str>) -> OutboxSnapshot {
        OutboxSnapshot {
            done: true,
            meta: Some(OutboxMeta {
                title: "Title".into(),
                suggested_next_steps: Some(SuggestedNextSteps {
                    suggested_next_commit_step: Some("Commit".into()),
                    suggested_next_note_step: None,
                }),
                turn: turn.map(str::to_string),
                written_at: None,
                server_version: Some(1),
            }),
            body: Some("Body".into()),
        }
    }

    #[test]
    fn outbox_state_distinguishes_pending_stale_and_delivered() {
        let pending = OutboxSnapshot {
            done: false,
            meta: None,
            body: None,
        };
        assert_eq!(
            outbox_state(pending, Some("t1")).unwrap(),
            OutboxState::Pending
        );

        assert_eq!(
            outbox_state(done_snapshot(Some("t0")), Some("t1")).unwrap(),
            OutboxState::StaleTurn {
                found: Some("t0".into())
            }
        );

        match outbox_state(done_snapshot(Some("t1")), Some("t1")).unwrap() {
            OutboxState::Delivered(submission) => {
                assert_eq!(submission.title, "Title");
                assert_eq!(submission.body, "Body");
                assert_eq!(submission.source, NoteSubmissionSource::Outbox);
                assert_eq!(submission.as_note_content(), "# Title\nBody");
                assert_eq!(
                    submission
                        .suggested_next_steps
                        .unwrap()
                        .suggested_next_commit_step
                        .as_deref(),
                    Some("Commit")
                );
            }
            other => panic!("{other:?}"),
        }

        // The sweep has no turn of its own and takes whatever is complete.
        assert!(matches!(
            outbox_state(done_snapshot(Some("t0")), None).unwrap(),
            OutboxState::Delivered(_)
        ));

        let incomplete = OutboxSnapshot {
            done: true,
            meta: None,
            body: Some("Body".into()),
        };
        assert!(outbox_state(incomplete, None).is_err());
    }

    /// Answers the collect script with `collect_output` and records every
    /// command; anything else succeeds with empty output.
    fn collecting_exec<'a>(
        collect_output: &str,
        log: &'a RefCell<Vec<Vec<String>>>,
    ) -> impl Fn(&[&str]) -> Result<String, String> + 'a {
        let collect_output = collect_output.to_string();
        move |args: &[&str]| {
            log.borrow_mut()
                .push(args.iter().map(|a| a.to_string()).collect());
            if args.len() == 3 && args[2].contains("echo done; else echo pending") {
                Ok(collect_output.clone())
            } else {
                Ok(String::new())
            }
        }
    }

    fn done_output(turn: &str) -> String {
        let meta = format!(
            r#"{{"title":"Plan","suggestedNextSteps":null,"turn":"{turn}","writtenAt":1,"helperVersion":2}}"#
        );
        format!("done\n{}\n{}\n", b64(meta.as_bytes()), b64(b"Body"))
    }

    /// The inactivity watchdog probes the outbox mid-turn with the same turn
    /// id every turn of the run carries. During a repair turn the previous
    /// turn's `done` is still there (the outbox is cleared at the end of the
    /// run, not between turns), and the probe must not read it as the agent
    /// having finished the repair.
    #[test]
    fn a_probe_treats_the_note_already_collected_as_pending_until_it_changes() {
        let slot = crate::note_tool::new_submission_slot();
        let memory = new_collected_note_memory();
        let log = RefCell::new(Vec::new());
        let done = |written_at: u64, body: &str| {
            let meta = format!(
                r#"{{"title":"Plan","suggestedNextSteps":null,"turn":"t1","writtenAt":{written_at},"helperVersion":2}}"#
            );
            format!("done\n{}\n{}\n", b64(meta.as_bytes()), b64(body.as_bytes()))
        };

        // Nothing collected yet: a probe that finds `done` is a delivery.
        let exec = collecting_exec(&done(1, "First"), &log);
        assert!(matches!(
            probe_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Delivered(_)
        ));
        assert_eq!(
            crate::note_tool::current_submission(&slot).unwrap().body,
            "First"
        );

        // The per-turn collector pulled the same note after the turn (say the
        // body failed Pikchr and a repair turn is now running); the probe
        // sees the identical `done` and reports nothing new.
        let exec = collecting_exec(&done(1, "First"), &log);
        assert!(matches!(
            collect_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Delivered(_)
        ));
        let exec = collecting_exec(&done(1, "First"), &log);
        assert_eq!(
            probe_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Pending
        );
        assert_eq!(
            crate::note_tool::current_submission(&slot).unwrap().body,
            "First",
            "the slot is left alone"
        );

        // A rewrite — new stamp or new content — is a delivery again.
        let exec = collecting_exec(&done(2, "First"), &log);
        assert!(matches!(
            probe_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Delivered(_)
        ));
        let exec = collecting_exec(&done(2, "Second"), &log);
        assert!(matches!(
            probe_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Delivered(_)
        ));
        assert_eq!(
            crate::note_tool::current_submission(&slot).unwrap().body,
            "Second"
        );
        // And the probe remembers what it delivered, like the collector.
        let exec = collecting_exec(&done(2, "Second"), &log);
        assert_eq!(
            probe_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Pending
        );

        // Other states pass through untouched.
        let exec = collecting_exec("pending\n\n\n", &log);
        assert_eq!(
            probe_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Pending
        );
    }

    #[test]
    fn collecting_a_turn_fills_the_slot_only_for_this_turns_note() {
        let slot = crate::note_tool::new_submission_slot();
        let memory = new_collected_note_memory();

        let log = RefCell::new(Vec::new());
        let exec = collecting_exec("pending\n\n\n", &log);
        assert_eq!(
            collect_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Pending
        );
        assert!(!crate::note_tool::has_submission(&slot));

        let exec = collecting_exec(&done_output("t0"), &log);
        assert_eq!(
            collect_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::StaleTurn {
                found: Some("t0".into())
            }
        );
        assert!(
            !crate::note_tool::has_submission(&slot),
            "another run's note must not pass for this turn's"
        );

        let exec = collecting_exec(&done_output("t1"), &log);
        assert!(matches!(
            collect_turn_into_slot(&exec, "sess-1", "t1", &slot, &memory).unwrap(),
            OutboxState::Delivered(_)
        ));
        let submission = crate::note_tool::current_submission(&slot).unwrap();
        assert_eq!(submission.title, "Plan");
        assert_eq!(submission.body, "Body");
        assert_eq!(submission.source, NoteSubmissionSource::Outbox);
        // One round trip per collection, nothing else.
        assert_eq!(log.borrow().len(), 3, "{:?}", log.borrow());
        assert!(log
            .borrow()
            .iter()
            .all(|cmd| cmd[2].contains("echo done; else echo pending")));
    }

    #[test]
    fn session_start_ingests_an_uncollected_note_before_clearing() {
        let log = RefCell::new(Vec::new());
        let exec = collecting_exec(&done_output("earlier-run"), &log);
        let mut ingested = Vec::new();
        let recovery = recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |submission| {
                ingested.push(submission);
                true
            },
            &mut || None,
        )
        .unwrap();
        assert_eq!(recovery.outbox, OutboxRecovery::Ingested);
        assert_eq!(recovery.draft, DraftSeed::Absent);
        assert_eq!(ingested.len(), 1);
        assert_eq!(ingested[0].title, "Plan");
        assert_eq!(ingested[0].source, NoteSubmissionSource::Outbox);
        let commands = log.borrow();
        assert_eq!(commands.len(), 2, "collect, then clear: {commands:?}");
        assert!(commands[0][2].contains("echo done; else echo pending"));
        assert_eq!(commands[1][2], clear_outbox_and_seed_script("sess-1", None));
    }

    #[test]
    fn session_start_keeps_a_note_it_could_not_store() {
        let log = RefCell::new(Vec::new());
        let exec = collecting_exec(&done_output("earlier-run"), &log);
        let error =
            recover_uncollected_outbox(&exec, "sess-1", &mut |_| false, &mut || None).unwrap_err();
        assert!(
            error.contains("Plan") && error.contains("could not be stored"),
            "{error}"
        );
        let commands = log.borrow();
        assert_eq!(commands.len(), 1, "collect only, no clear: {commands:?}");
    }

    #[test]
    fn session_start_with_nothing_waiting_only_clears() {
        let log = RefCell::new(Vec::new());
        let exec = collecting_exec("pending\n\n\n", &log);
        let mut ingested = 0;
        let recovery = recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |_| {
                ingested += 1;
                true
            },
            &mut || None,
        )
        .unwrap();
        assert_eq!(recovery.outbox, OutboxRecovery::Empty);
        assert_eq!(ingested, 0);
        let commands = log.borrow();
        assert_eq!(commands.len(), 2, "{commands:?}");
        assert_eq!(commands[1][2], clear_outbox_and_seed_script("sess-1", None));
    }

    #[test]
    fn session_start_discards_unusable_outbox_content_and_continues() {
        let log = RefCell::new(Vec::new());
        // `done` beside a missing `note.md`: a v1 script's partial write.
        let exec = collecting_exec("done\n\n\n", &log);
        let mut ingested = 0;
        let recovery = recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |_| {
                ingested += 1;
                true
            },
            &mut || None,
        )
        .unwrap();
        assert_eq!(recovery.outbox, OutboxRecovery::Discarded);
        assert_eq!(ingested, 0);
        let commands = log.borrow();
        assert_eq!(commands.len(), 2, "collect, then clear: {commands:?}");
        assert_eq!(commands[1][2], clear_outbox_and_seed_script("sess-1", None));
    }

    #[test]
    fn session_start_leaves_the_outbox_alone_when_it_cannot_be_read() {
        let log: RefCell<Vec<Vec<String>>> = RefCell::new(Vec::new());
        let exec = |args: &[&str]| -> Result<String, String> {
            log.borrow_mut()
                .push(args.iter().map(|a| a.to_string()).collect());
            Err("workstation suspended".to_string())
        };
        let mut ingested = 0;
        let error = recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |_| {
                ingested += 1;
                true
            },
            &mut || None,
        )
        .unwrap_err();
        assert!(error.contains("workstation suspended"), "{error}");
        assert_eq!(ingested, 0);
        assert_eq!(log.borrow().len(), 1, "no clear after a failed collection");
    }

    #[test]
    fn session_start_seeds_the_draft_in_the_clear_round_trip() {
        // A fresh note: the seed closure says None, and the clear removes a
        // stale draft and leaves the directory for the agent's first write,
        // creating no file. Two commands either way, so a fresh remote note
        // costs no more round trips than before.
        let log = RefCell::new(Vec::new());
        let exec = collecting_exec("pending\n\n\n", &log);
        recover_uncollected_outbox(&exec, "sess-1", &mut |_| true, &mut || None).unwrap();
        {
            let commands = log.borrow();
            assert_eq!(commands.len(), 2, "{commands:?}");
            let clear = &commands[1][2];
            assert_eq!(*clear, clear_outbox_and_seed_script("sess-1", None));
            assert!(clear.contains("rm -f \"$d/note.md\""), "{clear}");
            assert!(clear.contains("\"$d/draft.md\""), "{clear}");
            assert!(clear.contains("mkdir -p \"$d\""), "{clear}");
            assert!(!clear.contains("base64"), "no file is created: {clear}");
        }

        // An amendment: the stored body rides along in the clear command.
        log.borrow_mut().clear();
        let seed = b"Stored body ![Shot](staged-media://abc.png)";
        let mut asked = 0;
        let recovery = recover_uncollected_outbox(&exec, "sess-1", &mut |_| true, &mut || {
            asked += 1;
            Some(seed.to_vec())
        })
        .unwrap();
        assert_eq!(asked, 1);
        assert_eq!(recovery.draft, DraftSeed::Seeded);
        let commands = log.borrow();
        assert_eq!(
            commands.len(),
            2,
            "still collect, then clear+seed: {commands:?}"
        );
        let clear = &commands[1][2];
        assert_eq!(
            *clear,
            clear_outbox_and_seed_script("sess-1", Some(&b64(seed)))
        );
        assert!(clear.contains("rm -f \"$d/note.md\""), "{clear}");
        assert!(clear.contains(&b64(seed)), "{clear}");
        assert!(
            clear.contains("mv -f \"$d/.draft.md.seed\" \"$d/draft.md\""),
            "{clear}"
        );
    }

    #[test]
    fn a_seed_too_large_for_one_command_line_is_appended_after_the_clear() {
        let log = RefCell::new(Vec::new());
        let exec = collecting_exec("pending\n\n\n", &log);
        let seed = vec![b'x'; SEED_CHUNK_SIZE + 10];
        assert_eq!(
            clear_outbox_and_seed(&exec, "sess-1", Some(&seed)).unwrap(),
            DraftSeed::Seeded
        );
        let commands = log.borrow();
        assert_eq!(commands.len(), 3, "clear, then two chunks: {commands:?}");
        assert_eq!(commands[0][2], clear_outbox_script("sess-1"));
        assert!(commands[1][2].contains("base64 -d > \"$d/.draft.md.seed\""));
        assert!(!commands[1][2].contains("mv -f"));
        assert!(commands[2][2].contains("base64 -d >> \"$d/.draft.md.seed\""));
        assert!(commands[2][2].ends_with("mv -f \"$d/.draft.md.seed\" \"$d/draft.md\""));
    }

    /// `write_note` as a `tools/list` response lists it, wherever it sits
    /// among the Staged MCP's tools.
    fn listed_write_note(tools_list: &serde_json::Value) -> serde_json::Value {
        tools_list["result"]["tools"]
            .as_array()
            .expect("a tools/list result")
            .iter()
            .find(|tool| tool["name"] == crate::note_tool::WRITE_NOTE_TOOL_NAME)
            .unwrap_or_else(|| panic!("write_note is not listed: {tools_list}"))
            .clone()
    }

    fn property_names(schema: &serde_json::Value) -> Vec<String> {
        let mut names: Vec<String> = schema["properties"]
            .as_object()
            .expect("object schema")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// The nested next-steps object, whether written inline (`properties`) or
    /// as the first branch of an `anyOf`/`oneOf` nullable union.
    fn nested_steps_schema(schema: &serde_json::Value) -> serde_json::Value {
        let steps = &schema["properties"]["suggested_next_steps"];
        if steps.get("properties").is_some() {
            return steps.clone();
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(branches) = steps[key].as_array() {
                if let Some(obj) = branches.iter().find(|b| b.get("properties").is_some()) {
                    return obj.clone();
                }
                // schemars may emit `$ref` branches; resolve against `$defs`.
                for branch in branches {
                    if let Some(reference) = branch["$ref"].as_str() {
                        let name = reference.rsplit('/').next().unwrap();
                        let def = &schema["$defs"][name];
                        if def.get("properties").is_some() {
                            return def.clone();
                        }
                    }
                }
            }
        }
        if let Some(reference) = steps["$ref"].as_str() {
            let name = reference.rsplit('/').next().unwrap();
            return schema["$defs"][name].clone();
        }
        panic!("cannot find the suggested_next_steps object in {schema}");
    }

    /// Write the session's draft the way the agent would, at
    /// `~/.staged/sessions/sess-1/draft.md`.
    fn write_draft(home: &std::path::Path, body: &str) {
        let dir = home.join(".staged/sessions/sess-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("draft.md"), body).unwrap();
    }

    #[test]
    fn write_note_on_the_staged_mcp_writes_the_outbox() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping write_note subprocess test");
            return;
        };
        let home = tempfile::tempdir().unwrap();

        // A rejected call on its own must leave no trace in the outbox: a bad
        // title, and a good title with no draft to publish.
        let rejected = run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"write_note","arguments":{"title":""}}}),
                serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"No draft yet"}}}),
            ],
        );
        assert_eq!(rejected[0]["result"]["isError"], true);
        assert_eq!(rejected[1]["result"]["isError"], true);
        let text = rejected[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        let draft_path = home.path().join(".staged/sessions/sess-1/draft.md");
        assert!(
            text.contains(&format!("`{}`", draft_path.display()))
                && text.contains("does not exist"),
            "{text}"
        );
        assert!(
            !home.path().join(".staged/sessions/sess-1").exists(),
            "a rejected call writes nothing"
        );

        // The draft is the body: the agent writes it, then publishes it.
        write_draft(home.path(), "  Body line\n---\nmore  ");
        let responses = run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
                serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"write_note","arguments":{"title":""}}}),
                serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"# Plan","suggested_next_steps":{"suggestedNextCommitStep":" Fix it ","suggestedNextNoteStep":""}}}}),
                serde_json::json!({"jsonrpc":"2.0","id":5,"method":"nope"}),
            ],
        );
        assert_eq!(responses.len(), 5, "{responses:?}");

        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "staged");
        assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");
        let instructions = responses[0]["result"]["instructions"].as_str().unwrap();
        assert!(
            instructions.contains(&format!("`{}`", draft_path.display())),
            "initialize names the draft: {instructions}"
        );

        let write_note = listed_write_note(&responses[1]);
        assert_eq!(
            write_note["description"].as_str().unwrap(),
            crate::note_tool::WRITE_NOTE_DESCRIPTION
        );

        assert_eq!(responses[2]["id"], 3);
        assert_eq!(responses[2]["result"]["isError"], true);

        assert_eq!(responses[3]["id"], 4);
        assert_eq!(responses[3]["result"]["isError"], false);
        let text = responses[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert_eq!(
            text,
            crate::note_tool::saved_message("Plan", "Body line\n---\nmore".len())
        );

        assert_eq!(responses[4]["error"]["code"], -32601);

        let outbox = home.path().join(".staged/sessions/sess-1");
        assert!(outbox.join("done").exists());
        assert_eq!(
            std::fs::read_to_string(outbox.join("note.md")).unwrap(),
            "Body line\n---\nmore"
        );
        assert_eq!(
            std::fs::read_to_string(outbox.join("draft.md")).unwrap(),
            "  Body line\n---\nmore  ",
            "the script only reads the draft"
        );
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(outbox.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["title"], "Plan");
        assert_eq!(meta["turn"], "turn-1");
        assert_eq!(
            meta["suggestedNextSteps"]["suggestedNextCommitStep"],
            "Fix it"
        );
        assert_eq!(
            meta["suggestedNextSteps"]["suggestedNextNoteStep"],
            serde_json::Value::Null
        );

        // The collector script against the same outbox, under `sh`.
        let exec = local_exec(home.path());
        match collect(&exec, "sess-1", Some("turn-1")).unwrap() {
            OutboxState::Delivered(submission) => {
                assert_eq!(submission.title, "Plan");
                assert_eq!(submission.body, "Body line\n---\nmore");
                assert_eq!(
                    submission.suggested_next_steps,
                    Some(SuggestedNextSteps {
                        suggested_next_commit_step: Some("Fix it".into()),
                        suggested_next_note_step: None,
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            collect(&exec, "sess-1", Some("turn-2")).unwrap(),
            OutboxState::StaleTurn {
                found: Some("turn-1".into())
            }
        );

        clear_outbox(&exec, "sess-1").unwrap();
        assert_eq!(
            collect(&exec, "sess-1", None).unwrap(),
            OutboxState::Pending
        );
        assert!(
            !outbox.exists(),
            "the clear removes the draft too: {outbox:?}"
        );
        assert_eq!(
            collect(&exec, "never-ran", None).unwrap(),
            OutboxState::Pending
        );
    }

    #[test]
    fn write_note_schema_matches_the_rust_tool_schema() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping write_note schema test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let responses = run_script(
            home.path(),
            &node,
            &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
        );
        let script_schema = &listed_write_note(&responses[0])["inputSchema"];
        let tool_schema = crate::note_tool::write_note_input_schema();

        assert_eq!(property_names(script_schema), property_names(&tool_schema));
        assert_eq!(script_schema["required"], tool_schema["required"]);
        assert_eq!(
            property_names(&nested_steps_schema(script_schema)),
            property_names(&nested_steps_schema(&tool_schema)),
            "script: {script_schema}\ntool: {tool_schema}"
        );
    }

    #[test]
    fn write_note_rewrite_drops_the_done_marker_until_both_files_are_renamed() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping write_note rewrite test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join(".staged/sessions/sess-1");
        let call = |id: u32, title: &str| serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"write_note","arguments":{"title":title}}});

        write_draft(home.path(), "first body");
        let first = run_script(home.path(), &node, &[call(1, "First")]);
        assert_eq!(first[0]["result"]["isError"], false);
        assert!(outbox.join("done").exists());

        // Make the second call's `meta.json` rename fail: a non-empty
        // directory in its place cannot be renamed over. The tool is then
        // stopped between `note.md` and `meta.json` — exactly where a sweep
        // reading the outbox must not find `done`.
        std::fs::remove_file(outbox.join("meta.json")).unwrap();
        std::fs::create_dir(outbox.join("meta.json")).unwrap();
        std::fs::write(outbox.join("meta.json/keep"), "x").unwrap();
        write_draft(home.path(), "second body");
        let second = run_script(home.path(), &node, &[call(2, "Second")]);
        assert_eq!(
            second[0]["result"]["isError"], true,
            "the blocked rename is reported to the agent: {second:?}"
        );
        assert!(
            !outbox.join("done").exists(),
            "`done` was unlinked before the rewrite began"
        );
        assert_eq!(
            std::fs::read_to_string(outbox.join("note.md")).unwrap(),
            "second body",
            "the body rename landed before the failure"
        );
        let exec = local_exec(home.path());
        assert_eq!(
            collect(&exec, "sess-1", None).unwrap(),
            OutboxState::Pending,
            "a half-written rewrite is never collected as a note"
        );

        // Unblock and rewrite again: the marker comes back last, with both
        // files from the same call.
        std::fs::remove_dir_all(outbox.join("meta.json")).unwrap();
        write_draft(home.path(), "third body");
        let third = run_script(home.path(), &node, &[call(3, "Third")]);
        assert_eq!(third[0]["result"]["isError"], false);
        match collect(&exec, "sess-1", Some("turn-1")).unwrap() {
            OutboxState::Delivered(submission) => {
                assert_eq!(submission.title, "Third");
                assert_eq!(submission.body, "third body");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn session_start_recovery_runs_against_a_local_shell() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping local recovery test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        write_draft(home.path(), "found later");
        run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"Lost"}}}),
            ],
        );
        let exec = local_exec(home.path());
        let mut ingested = Vec::new();
        assert_eq!(
            recover_uncollected_outbox(
                &exec,
                "sess-1",
                &mut |s| {
                    ingested.push(s);
                    true
                },
                &mut || None
            )
            .unwrap()
            .outbox,
            OutboxRecovery::Ingested
        );
        assert_eq!(ingested.len(), 1);
        assert_eq!(ingested[0].title, "Lost");
        assert_eq!(ingested[0].body, "found later");
        // The run-start clear empties the directory and leaves it for the
        // agent's first write of the draft; the end-of-run clear removes it.
        let outbox = home.path().join(".staged/sessions/sess-1");
        assert!(outbox.is_dir(), "{outbox:?}");
        assert_eq!(
            std::fs::read_dir(&outbox).unwrap().count(),
            0,
            "outbox files and draft are gone"
        );
        clear_outbox(&exec, "sess-1").unwrap();
        assert!(
            !outbox.exists(),
            "the emptied outbox directory is removed too"
        );
        assert_eq!(
            recover_uncollected_outbox(
                &exec,
                "sess-1",
                &mut |_| panic!("nothing to ingest"),
                &mut || None
            )
            .unwrap()
            .outbox,
            OutboxRecovery::Empty
        );

        // The collect script against a real shell: four lines, marker
        // bracketing the files, nothing left behind after a clear.
        write_draft(home.path(), "second");
        run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"Again"}}}),
            ],
        );
        let output = exec(&["sh", "-c", &collect_script("sess-1")]).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 4, "{output:?}");
        assert_eq!((lines[0], lines[3]), ("done", "done"));
        assert!(matches!(
            collect(&exec, "sess-1", None).unwrap(),
            OutboxState::Delivered(submission) if submission.title == "Again"
        ));
        std::fs::write(outbox.join("extra"), "keep").unwrap();
        clear_outbox(&exec, "sess-1").unwrap();
        assert!(
            outbox.exists(),
            "a directory holding anything else is not removed"
        );
        assert!(
            !outbox.join("draft.md").exists(),
            "the clear removes the draft"
        );
        std::fs::remove_file(outbox.join("extra")).unwrap();
        std::fs::remove_dir(&outbox).unwrap();
    }

    #[test]
    fn the_seed_in_clear_script_runs_against_a_local_shell() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping local seed test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");

        // An amendment's start: a finished note from an earlier run is
        // waiting, the session resumes, and the recovery's clear seeds the
        // draft with the stored body in the same command.
        write_draft(home.path(), "old draft");
        run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"Earlier"}}}),
            ],
        );
        let seed = "Stored body\n\n![Shot](staged-media://abc.png)\n";
        let mut ingested = Vec::new();
        assert_eq!(
            recover_uncollected_outbox(
                &exec,
                "sess-1",
                &mut |s| {
                    ingested.push(s);
                    true
                },
                &mut || Some(seed.as_bytes().to_vec()),
            )
            .unwrap()
            .outbox,
            OutboxRecovery::Ingested
        );
        assert_eq!(ingested[0].body, "old draft");
        assert_eq!(
            std::fs::read_to_string(outbox.join("draft.md")).unwrap(),
            seed
        );
        for gone in ["note.md", "meta.json", "done", ".draft.md.seed"] {
            assert!(!outbox.join(gone).exists(), "{gone} should be gone");
        }
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&outbox).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o700);

        // The agent edits the seeded draft and publishes it.
        write_draft(home.path(), "Stored body, amended");
        let published = run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"Amended"}}}),
            ],
        );
        assert_eq!(published[0]["result"]["isError"], false);
        assert!(matches!(
            collect(&exec, "sess-1", Some("turn-1")).unwrap(),
            OutboxState::Delivered(submission) if submission.body == "Stored body, amended"
        ));

        // A fresh note's start with the same directory: no seed, so the
        // clear leaves no draft and no outbox file, only the directory
        // (mode 700) the agent will write its draft into.
        recover_uncollected_outbox(&exec, "sess-1", &mut |_| true, &mut || None).unwrap();
        assert!(outbox.is_dir(), "{outbox:?}");
        assert_eq!(std::fs::read_dir(&outbox).unwrap().count(), 0);
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&outbox).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }

        // A fresh start on a workstation that has no session directory yet
        // creates it, and the end-of-run clear takes it away again.
        clear_outbox(&exec, "sess-1").unwrap();
        assert!(!outbox.exists(), "{outbox:?}");
        recover_uncollected_outbox(&exec, "sess-1", &mut |_| true, &mut || None).unwrap();
        assert!(outbox.is_dir(), "{outbox:?}");
        clear_outbox(&exec, "sess-1").unwrap();
        assert!(!outbox.exists(), "{outbox:?}");

        // A large seed goes in chunks and arrives whole.
        let large = vec![b'y'; SEED_CHUNK_SIZE + 1];
        assert_eq!(
            clear_outbox_and_seed(&exec, "sess-1", Some(&large)).unwrap(),
            DraftSeed::Seeded
        );
        assert_eq!(std::fs::read(outbox.join("draft.md")).unwrap(), large);
        assert!(!outbox.join(".draft.md.seed").exists());
    }

    /// An exec that answers the collect with `collect_output` and every
    /// other command through `other`, which sees the command's index among
    /// those others.
    fn scripted_exec<'a>(
        collect_output: &'a str,
        log: &'a RefCell<Vec<String>>,
        other: impl Fn(usize) -> Result<String, String> + 'a,
    ) -> impl Fn(&[&str]) -> Result<String, String> + 'a {
        move |args: &[&str]| {
            if args[2].contains("echo done; else echo pending") {
                return Ok(collect_output.to_string());
            }
            let index = log.borrow().len();
            log.borrow_mut().push(args[2].to_string());
            other(index)
        }
    }

    #[test]
    fn a_failed_seed_is_reported_apart_from_the_settled_recovery() {
        // The seeding clear ran — the outbox is empty — but printed the
        // marker: the recovery is settled, and only the draft is missing.
        let log = RefCell::new(Vec::new());
        let done = done_output("earlier-run");
        let exec = scripted_exec(&done, &log, |_| Ok(format!("{SEED_FAILED_MARKER}\n")));
        let mut ingested = 0;
        let recovery = recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |_| {
                ingested += 1;
                true
            },
            &mut || Some(b"Stored body".to_vec()),
        )
        .expect("a failed seed is not a failed recovery");
        assert_eq!(ingested, 1);
        assert_eq!(recovery.outbox, OutboxRecovery::Ingested);
        assert!(
            matches!(recovery.draft, DraftSeed::Failed(_)),
            "{recovery:?}"
        );
        assert_eq!(log.borrow().len(), 1, "one clear, no retry");

        // A chunked seed: once the clear's own round trip has succeeded, a
        // chunk that fails costs the seed, not the recovery.
        let log = RefCell::new(Vec::new());
        let exec = scripted_exec("pending\n\n\n", &log, |index| match index {
            0 | 1 => Ok(String::new()),
            _ => Err("workstation suspended".to_string()),
        });
        let large = vec![b'x'; SEED_CHUNK_SIZE + 10];
        match clear_outbox_and_seed(&exec, "sess-1", Some(&large)).unwrap() {
            DraftSeed::Failed(error) => {
                assert!(error.contains("chunk 1"), "{error}");
                assert!(error.contains("workstation suspended"), "{error}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(log.borrow()[0], clear_outbox_script("sess-1"));

        // A clear that could not be confirmed is still an error: the outbox
        // is in an unknown state, with or without a seed.
        for seed in [
            None,
            Some(b"Stored body".as_slice()),
            Some(large.as_slice()),
        ] {
            let log = RefCell::new(Vec::new());
            let exec = scripted_exec("pending\n\n\n", &log, |_| {
                Err("workstation suspended".to_string())
            });
            let error = recover_uncollected_outbox(&exec, "sess-1", &mut |_| true, &mut || {
                seed.map(<[u8]>::to_vec)
            })
            .unwrap_err();
            assert!(error.contains("workstation suspended"), "{error}");
            assert_eq!(log.borrow().len(), 1, "nothing after the failed clear");
        }
    }

    #[test]
    fn every_clear_removes_the_seed_temp_file() {
        for script in [
            clear_outbox_script("sess-1"),
            clear_unpublished_outbox_script("sess-1"),
            clear_outbox_and_seed_script("sess-1", None),
            clear_outbox_and_seed_script("sess-1", Some("QQ==")),
        ] {
            let remove = script
                .split(';')
                .find(|part| part.trim_start().starts_with("rm -f \"$d/note.md\""))
                .unwrap_or_else(|| panic!("no rm -f in {script}"));
            for file in ["note.md", "meta.json", "done", "draft.md", ".draft.md.seed"] {
                assert!(
                    remove.contains(&format!("\"$d/{file}\"")),
                    "{file}: {script}"
                );
            }
        }

        // Against a real shell: the temp file a failed chunked seed left
        // behind no longer pins the session directory.
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");
        std::fs::create_dir_all(&outbox).unwrap();
        std::fs::write(outbox.join(".draft.md.seed"), "half a seed").unwrap();
        clear_outbox(&exec, "sess-1").unwrap();
        assert!(!outbox.exists(), "{outbox:?}");

        std::fs::create_dir_all(&outbox).unwrap();
        std::fs::write(outbox.join(".draft.md.seed"), "half a seed").unwrap();
        assert_eq!(
            clear_outbox_and_seed(&exec, "sess-1", None).unwrap(),
            DraftSeed::Absent
        );
        assert_eq!(std::fs::read_dir(&outbox).unwrap().count(), 0);
        assert_eq!(
            clear_outbox_and_seed(&exec, "sess-1", Some(b"Stored body")).unwrap(),
            DraftSeed::Seeded
        );
        assert_eq!(
            std::fs::read_dir(&outbox).unwrap().count(),
            1,
            "only draft.md"
        );
    }

    /// The end-of-run clear of a run that never read the outbox after its
    /// last turn, against a real shell: a published note stays whole, with
    /// its draft, and anything else goes.
    #[test]
    fn the_unread_clear_keeps_a_published_note_and_removes_anything_else() {
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");
        let files = [
            ("note.md", "Body"),
            (
                "meta.json",
                r#"{"title":"Kept","turn":"turn-1","writtenAt":1}"#,
            ),
            ("done", ""),
            ("draft.md", "Body"),
        ];

        std::fs::create_dir_all(&outbox).unwrap();
        for (file, content) in files {
            std::fs::write(outbox.join(file), content).unwrap();
        }
        assert_eq!(
            clear_unpublished_outbox(&exec, "sess-1").unwrap(),
            UnreadOutboxClear::PublishedNoteKept
        );
        for (file, content) in files {
            assert_eq!(
                std::fs::read_to_string(outbox.join(file)).unwrap(),
                content,
                "{file} is untouched"
            );
        }
        assert!(
            matches!(
                collect(&exec, "sess-1", None).unwrap(),
                OutboxState::Delivered(submission)
                    if submission.title == "Kept" && submission.body == "Body"
            ),
            "the next run's recovery still finds the note"
        );

        // Not published: a half-written note and the draft go, and the
        // directory with them.
        std::fs::remove_file(outbox.join("done")).unwrap();
        assert_eq!(
            clear_unpublished_outbox(&exec, "sess-1").unwrap(),
            UnreadOutboxClear::Cleared
        );
        assert!(!outbox.exists(), "{outbox:?}");

        // Nothing there at all.
        assert_eq!(
            clear_unpublished_outbox(&exec, "sess-1").unwrap(),
            UnreadOutboxClear::Cleared
        );

        // The marker is matched as a whole line of the script's output.
        let kept = |_: &[&str]| Ok(format!("  {PUBLISHED_NOTE_KEPT_MARKER}\n"));
        assert_eq!(
            clear_unpublished_outbox(&kept, "sess-1").unwrap(),
            UnreadOutboxClear::PublishedNoteKept
        );
        let unreachable = |_: &[&str]| Err::<String, String>("no route".to_string());
        assert!(clear_unpublished_outbox(&unreachable, "sess-1")
            .unwrap_err()
            .contains("no route"));
        assert!(
            clear_unpublished_outbox(&exec, "../sess-1").is_err(),
            "a session id that is not a plain name never reaches the shell"
        );
    }

    #[test]
    fn a_seed_the_workstation_cannot_write_still_clears_and_reports_it() {
        // The session directory's path is taken by a plain file, so the
        // seed has nowhere to go. The command still exits 0 and says so.
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let sessions = home.path().join(".staged/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("sess-1"), "in the way").unwrap();
        assert!(matches!(
            clear_outbox_and_seed(&exec, "sess-1", Some(b"Stored body")).unwrap(),
            DraftSeed::Failed(_)
        ));
    }

    /// The state an unsettled recovery leaves (see
    /// [`recover_uncollected_outbox`]): the draft on the workstation is
    /// whatever the lost run left. Nothing reads it as a note.
    #[test]
    fn a_draft_left_by_a_lost_run_is_never_stored_without_a_write_note_call() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping lost-draft test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");
        let unpublished = "Unpublished text from a lost run";

        // A draft and no call: every reader the laptop has sees no note.
        write_draft(home.path(), unpublished);
        let raw = exec(&["sh", "-c", &collect_script("sess-1")]).unwrap();
        assert!(!raw.contains(&b64(unpublished.as_bytes())), "{raw}");
        assert_eq!(
            collect(&exec, "sess-1", None).unwrap(),
            OutboxState::Pending
        );
        let slot = crate::note_tool::new_submission_slot();
        let memory = new_collected_note_memory();
        assert_eq!(
            collect_turn_into_slot(&exec, "sess-1", "turn-1", &slot, &memory).unwrap(),
            OutboxState::Pending
        );
        assert_eq!(
            probe_turn_into_slot(&exec, "sess-1", "turn-1", &slot, &memory).unwrap(),
            OutboxState::Pending
        );
        assert!(!crate::note_tool::has_submission(&slot));

        // The recovery that cannot reach the workstation: an error, nothing
        // ingested, and the draft exactly as the lost run left it.
        let unreachable = |_: &[&str]| Err::<String, String>("workstation suspended".to_string());
        recover_uncollected_outbox(
            &unreachable,
            "sess-1",
            &mut |_| panic!("nothing was published"),
            &mut || None,
        )
        .unwrap_err();
        assert_eq!(
            std::fs::read_to_string(outbox.join("draft.md")).unwrap(),
            unpublished
        );

        // A published note beside a draft edited afterwards, in a recovery
        // that cannot store it: what is offered — and kept — is the
        // published body, never the later edit.
        write_draft(home.path(), "Published body");
        run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"Published"}}}),
            ],
        );
        write_draft(home.path(), "Edited after the call, never republished");
        let mut offered = Vec::new();
        recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |submission| {
                offered.push(submission.body);
                false
            },
            &mut || None,
        )
        .unwrap_err();
        assert_eq!(offered, ["Published body"]);
        assert!(matches!(
            collect(&exec, "sess-1", None).unwrap(),
            OutboxState::Delivered(submission) if submission.body == "Published body"
        ));
        assert_eq!(
            std::fs::read_to_string(outbox.join("draft.md")).unwrap(),
            "Edited after the call, never republished"
        );

        // The next settled recovery ingests that note and removes the draft
        // before the agent starts.
        let recovery =
            recover_uncollected_outbox(&exec, "sess-1", &mut |_| true, &mut || None).unwrap();
        assert_eq!(recovery.outbox, OutboxRecovery::Ingested);
        assert_eq!(std::fs::read_dir(&outbox).unwrap().count(), 0);
    }

    /// One draft fixture for the two-transport comparison.
    enum DraftFixture {
        /// No file at the path.
        Missing,
        /// A regular file holding these bytes.
        Bytes(Vec<u8>),
        /// A sparse regular file of this many bytes.
        Sparse(u64),
        Directory,
        Fifo,
        /// A symbolic link to a regular file holding this text.
        SymlinkTo(&'static str),
        /// A regular file with mode 000.
        Unreadable,
    }

    /// What `write_note` answered: the result text, and the body it
    /// published when it accepted the draft.
    #[derive(Debug, PartialEq, Eq)]
    struct DraftOutcome {
        text: String,
        body: Option<String>,
    }

    /// `write_note` has two implementations of the draft read: the laptop's
    /// (`note_draft::read_draft` behind `note_tool::handle_write_note`) and
    /// the script's `readDraft`. They must accept and reject the same files,
    /// in the same words, and publish the same body.
    #[cfg(unix)]
    #[test]
    fn write_note_reads_the_same_drafts_on_both_transports() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let Some(node) = node() else {
            eprintln!("node not available; skipping two-transport draft test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join(".staged/sessions/sess-1");
        std::fs::create_dir_all(&outbox).unwrap();
        let draft = outbox.join("draft.md");
        let shown = draft.to_string_lossy().into_owned();
        let elsewhere = home.path().join("elsewhere.md");
        let max = crate::note_draft::DRAFT_MAX_BYTES;

        // Both servers name the draft in their `initialize` instructions.
        let init = run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
            ],
        );
        assert_eq!(
            init[0]["result"]["instructions"].as_str().unwrap(),
            crate::note_tool::draft_instructions(&shown)
        );

        // (what, fixture, the rejection's wording or None for an accepted
        // draft with its published body)
        let cases: Vec<(&str, DraftFixture, Result<&str, &str>)> = vec![
            ("missing", DraftFixture::Missing, Err("does not exist")),
            (
                "whitespace only",
                DraftFixture::Bytes(b"  \n\t\n".to_vec()),
                Err("is empty"),
            ),
            (
                "byte order mark only",
                DraftFixture::Bytes("\u{feff}".into()),
                Err("is empty"),
            ),
            (
                "byte order mark and whitespace",
                DraftFixture::Bytes("\u{feff} \n\u{a0}\n".into()),
                Err("is empty"),
            ),
            (
                "byte order mark before the body",
                DraftFixture::Bytes("\u{feff}\n# Section\nBody.\n".into()),
                Ok("# Section\nBody."),
            ),
            (
                "only the first mark is dropped",
                DraftFixture::Bytes("\u{feff}\u{feff}Body\u{feff}".into()),
                Ok("\u{feff}Body\u{feff}"),
            ),
            (
                "a mark after whitespace is text",
                DraftFixture::Bytes(" \u{feff}Body".into()),
                Ok("\u{feff}Body"),
            ),
            (
                // U+0085 and U+3000 are whitespace to Rust; JavaScript's
                // own `trim` would keep the first.
                "unicode whitespace is trimmed alike",
                DraftFixture::Bytes("\u{85}\u{3000}Body \u{2028}\u{85}".into()),
                Ok("Body"),
            ),
            (
                "not UTF-8",
                DraftFixture::Bytes(vec![0xff, 0xfe, b'x']),
                Err("not valid UTF-8"),
            ),
            (
                "one byte over the cap",
                DraftFixture::Sparse(max + 1),
                Err("byte limit"),
            ),
            (
                "a directory",
                DraftFixture::Directory,
                Err("not a regular file"),
            ),
            ("a FIFO", DraftFixture::Fifo, Err("not a regular file")),
            (
                "a link to a regular file",
                DraftFixture::SymlinkTo("Linked body\n"),
                Ok("Linked body"),
            ),
            (
                "unreadable",
                DraftFixture::Unreadable,
                Err("could not be read: "),
            ),
        ];

        for (what, fixture, expected) in cases {
            // A clean session directory holding only the fixture.
            for entry in std::fs::read_dir(&outbox).unwrap() {
                let path = entry.unwrap().path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                if meta.is_dir() {
                    std::fs::remove_dir_all(&path).unwrap();
                } else {
                    std::fs::remove_file(&path).unwrap();
                }
            }
            match &fixture {
                DraftFixture::Missing => {}
                DraftFixture::Bytes(bytes) => std::fs::write(&draft, bytes).unwrap(),
                DraftFixture::Sparse(len) => std::fs::File::create(&draft)
                    .unwrap()
                    .set_len(*len)
                    .unwrap(),
                DraftFixture::Directory => std::fs::create_dir(&draft).unwrap(),
                DraftFixture::Fifo => {
                    let path = std::ffi::CString::new(draft.as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                }
                DraftFixture::SymlinkTo(text) => {
                    std::fs::write(&elsewhere, text).unwrap();
                    std::os::unix::fs::symlink(&elsewhere, &draft).unwrap();
                }
                DraftFixture::Unreadable => {
                    std::fs::write(&draft, "Body").unwrap();
                    std::fs::set_permissions(&draft, std::fs::Permissions::from_mode(0o000))
                        .unwrap();
                    if std::fs::File::open(&draft).is_ok() {
                        eprintln!("running as root; skipping the unreadable draft");
                        continue;
                    }
                }
            }

            // The laptop's handler.
            let slot = crate::note_tool::new_submission_slot();
            let result = crate::note_tool::handle_write_note(
                "sess-1",
                &slot,
                &draft,
                WriteNoteParams {
                    title: "Title".to_string(),
                    suggested_next_steps: None,
                },
            );
            let content = serde_json::to_value(&result.content).unwrap();
            let laptop = DraftOutcome {
                text: content[0]["text"].as_str().unwrap().to_string(),
                body: crate::note_tool::current_submission(&slot).map(|s| s.body),
            };
            assert_eq!(
                result.is_error == Some(true),
                laptop.body.is_none(),
                "{what}"
            );

            // The script, on the same file.
            let responses = run_script(
                home.path(),
                &node,
                &[
                    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"Title"}}}),
                ],
            );
            let is_error = responses[0]["result"]["isError"] == true;
            let script = DraftOutcome {
                text: responses[0]["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string(),
                body: std::fs::read_to_string(outbox.join("note.md")).ok(),
            };
            assert_eq!(is_error, script.body.is_none(), "{what}: {script:?}");
            assert_eq!(
                outbox.join("done").exists(),
                !is_error,
                "{what}: a rejected draft leaves the outbox untouched"
            );

            match expected {
                Ok(body) => {
                    assert_eq!(laptop.body.as_deref(), Some(body), "{what}");
                    assert_eq!(script, laptop, "{what}");
                }
                // The cause after the colon is the platform's own message
                // and differs between Rust and Node; the wording up to it
                // is shared.
                Err(wording) if matches!(fixture, DraftFixture::Unreadable) => {
                    let prefix = format!("write_note rejected: the draft file `{shown}` {wording}");
                    assert!(laptop.text.starts_with(&prefix), "{what}: {laptop:?}");
                    assert!(script.text.starts_with(&prefix), "{what}: {script:?}");
                    for text in [&laptop.text, &script.text] {
                        assert!(
                            text.ends_with(", then call write_note again."),
                            "{what}: {text}"
                        );
                    }
                }
                Err(wording) => {
                    assert!(laptop.text.contains(wording), "{what}: {laptop:?}");
                    assert!(laptop.text.contains(&shown), "{what}: {laptop:?}");
                    assert_eq!(script, laptop, "{what}");
                }
            }
        }

        // A draft exactly at the cap is published whole by both.
        std::fs::remove_file(&draft).unwrap();
        std::fs::write(&draft, vec![b'x'; max as usize]).unwrap();
        let slot = crate::note_tool::new_submission_slot();
        crate::note_tool::handle_write_note(
            "sess-1",
            &slot,
            &draft,
            WriteNoteParams {
                title: "Title".to_string(),
                suggested_next_steps: None,
            },
        );
        let laptop = crate::note_tool::current_submission(&slot).expect("published");
        run_script(
            home.path(),
            &node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":"Title"}}}),
            ],
        );
        let script = std::fs::read(outbox.join("note.md")).unwrap();
        assert_eq!(script.len() as u64, max);
        assert_eq!(laptop.body.as_bytes(), script.as_slice());
    }
}
