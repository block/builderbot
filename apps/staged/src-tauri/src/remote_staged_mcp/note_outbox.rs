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
//! ~/.staged/sessions/<session-id>/done      marker: a complete note is present;
//!                                           holds that call's generation token
//! ```
//!
//! The title in `meta.json` is the call's `title` argument, or, for a call
//! that passed none, the text of the `# Title` heading the draft opens with;
//! `note.md` then holds the draft without that line. The script does that
//! split on the workstation, because the laptop never reads the draft. How
//! the title is read back depends on the script's version, see
//! [`TITLE_AS_WRITTEN_SERVER_VERSION`].
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
    NoteSubmission, NoteSubmissionSlot, NoteSubmissionSource, SuggestedNextSteps,
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

/// What [`clear_unpublished_outbox_script`] prints when the `done` marker it
/// found belonged to the rejected note it was given, which it removed.
const REJECTED_NOTE_CLEARED_MARKER: &str = "staged-rejected-note-cleared";

/// The POSIX `cksum` of `bytes`: the CRC and the byte count, the two numbers
/// `cksum < file` prints. Every POSIX system has the command, so a workstation
/// script can tell whether a file still holds bytes the laptop has read
/// without the laptop sending them back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileCksum {
    crc: u32,
    len: u64,
}

impl FileCksum {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        // CRC-32/CKSUM: polynomial 0x04C11DB7, most significant bit first, no
        // reflection, the length appended low byte first, complemented.
        fn feed(crc: u32, byte: u8) -> u32 {
            let mut crc = crc ^ (u32::from(byte) << 24);
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
            }
            crc
        }
        let mut crc = bytes.iter().fold(0, |crc, byte| feed(crc, *byte));
        let mut len = bytes.len() as u64;
        while len != 0 {
            crc = feed(crc, (len & 0xff) as u8);
            len >>= 8;
        }
        Self {
            crc: !crc,
            len: bytes.len() as u64,
        }
    }
}

/// As `cksum < file` prints it.
impl std::fmt::Display for FileCksum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.crc, self.len)
    }
}

/// Which published note an outbox held when it was collected: the checksums
/// of `meta.json` (title, next steps, turn id, `writtenAt`) and of `note.md`
/// as they sat on the workstation. A later command there can test whether
/// the outbox still holds that same note
/// ([`clear_unpublished_outbox_script`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PublishedNoteStamp {
    meta: FileCksum,
    body: FileCksum,
}

impl PublishedNoteStamp {
    /// The stamp of a published note with these `meta.json` and `note.md`
    /// bytes.
    pub(crate) fn of(meta: &[u8], body: &[u8]) -> Self {
        Self {
            meta: FileCksum::of(meta),
            body: FileCksum::of(body),
        }
    }

    /// The stamp of the note in a collect script's output; `None` when it
    /// does not hold both files.
    fn of_collect_output(output: &str) -> Option<Self> {
        use base64::Engine;
        let mut lines = output.lines().map(str::trim).skip(1);
        let mut decode = || {
            let line = lines.next().filter(|line| !line.is_empty())?;
            base64::engine::general_purpose::STANDARD.decode(line).ok()
        };
        let meta = decode()?;
        let body = decode()?;
        Some(Self::of(&meta, &body))
    }
}

/// [`clear_outbox_script`] for a caller that has not read the outbox since
/// the agent last could have written to it: the files are removed only when
/// no `done` marker is there. A marker means a published note nobody has
/// collected, and then nothing is touched — not the note, and not the draft
/// it was published from — so the next reader (the start-of-run recovery of
/// a Resume) finds it whole. The test and the removal are one command on
/// the workstation, so there is no window between a laptop-side read and
/// the clear. Always exits 0.
///
/// `rejected` is the one published note the caller *has* read: a note its
/// run collected and then turned down (a Pikchr block that does not parse).
/// A marker that still belongs to that note is no reason to keep anything:
/// the note is known bad and the run that read it has already spent its
/// repair on it. So it is removed with the rest. The script recognises it by
/// the `cksum` of `meta.json` and of `note.md`; a note published since has a
/// different `writtenAt` or body and stays. A workstation without `cksum`
/// matches nothing and keeps the note, which is the rule without `rejected`.
///
/// This removal used to be what kept such a note out of the store, when the
/// detached sweep stored what it found unchecked. Every reader now runs the
/// Pikchr check (the sweep parks the row, and the next run's recovery asks
/// for a repair, see [`recover_uncollected_outbox`]), so a kept note would
/// be turned down again, not stored. The removal is left as it was; taking
/// it out is a separate cleanup.
pub(crate) fn clear_unpublished_outbox_script(
    session_id: &str,
    rejected: Option<PublishedNoteStamp>,
) -> String {
    let dir = session_dir_expr(session_id);
    let remove = remove_session_files_fragment();
    let keep = format!("echo {PUBLISHED_NOTE_KEPT_MARKER}; exit 0");
    let published = match rejected {
        None => keep,
        Some(stamp) => format!(
            "if [ \"$(cksum < \"$d/meta.json\" 2>/dev/null)\" = \"{meta}\" ] && \
             [ \"$(cksum < \"$d/note.md\" 2>/dev/null)\" = \"{body}\" ]; \
             then echo {REJECTED_NOTE_CLEARED_MARKER}; else {keep}; fi",
            meta = stamp.meta,
            body = stamp.body,
        ),
    };
    format!(
        "d={dir}; if [ -f \"$d/done\" ]; then {published}; fi; \
         {remove}; rmdir \"$d\" 2>/dev/null; exit 0"
    )
}

/// What [`clear_unpublished_outbox`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnreadOutboxClear {
    /// No published note was there; the partial files and the draft are gone.
    Cleared,
    /// The published note there was the one the caller had rejected; it is
    /// gone with the rest.
    RejectedNoteCleared,
    /// A `done` marker was there: a published note no run has collected.
    /// Everything was left in place.
    PublishedNoteKept,
}

/// Empty the session's outbox unless it holds a published note other than
/// `rejected` (see [`clear_unpublished_outbox_script`]).
pub(crate) fn clear_unpublished_outbox(
    exec: RemoteExec,
    session_id: &str,
    rejected: Option<PublishedNoteStamp>,
) -> Result<UnreadOutboxClear, String> {
    validate_session_id(session_id)?;
    let script = clear_unpublished_outbox_script(session_id, rejected);
    let output = exec(&["sh", "-c", &script])
        .map_err(|e| format!("failed to clear the remote note outbox: {e}"))?;
    let printed = |marker: &str| output.lines().any(|line| line.trim() == marker);
    Ok(if printed(PUBLISHED_NOTE_KEPT_MARKER) {
        UnreadOutboxClear::PublishedNoteKept
    } else if printed(REJECTED_NOTE_CLEARED_MARKER) {
        UnreadOutboxClear::RejectedNoteCleared
    } else {
        UnreadOutboxClear::Cleared
    })
}

/// Largest base64 payload put on one command line. The workstation runs each
/// command as `sh -c <script>`, and Linux refuses any single argument of
/// 131072 bytes or more ("Argument list too long"), whatever ARG_MAX is. This
/// bound leaves the rest for the script's fixed text. macOS limits only the
/// total, so a chunk that is too large fails on a workstation and on Linux
/// CI, never on the laptop.
const SEED_CHUNK_SIZE: usize = 100_000;

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
    /// The recovery turned down a published note
    /// ([`OutboxRecovery::Rejected`]): `draft.md` is left as the earlier run
    /// wrote it and nothing is seeded, so the agent repairs that file. When
    /// the earlier run left no draft, the rejected body was put there
    /// ([`reject_published_note_script`]).
    Kept,
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

/// The run-start clear for a published note the recovery turned down
/// ([`OutboxRecovery::Rejected`]): remove the publication and keep the draft
/// it was published from.
///
/// Only `done`, `meta.json` and `note.md` go, the marker first, as
/// `write_note` itself starts a rewrite: the run that is starting must not
/// collect the old note as its own, and no later reader may find it.
/// `draft.md` is not touched and nothing is seeded, so the agent repairs
/// the text it wrote instead of writing the note again. If the earlier run
/// left no draft at all, the rejected body becomes the draft, so the file
/// the repair prompt names always exists. Always exits 0.
pub(crate) fn reject_published_note_script(session_id: &str) -> String {
    let dir = session_dir_expr(session_id);
    let draft = crate::note_draft::REMOTE_DRAFT_FILE_NAME;
    format!(
        "d={dir}; rm -f \"$d/done\"; \
         if [ ! -e \"$d/{draft}\" ]; then mv -f \"$d/note.md\" \"$d/{draft}\" 2>/dev/null; fi; \
         rm -f \"$d/meta.json\" \"$d/note.md\"; exit 0"
    )
}

/// Run [`reject_published_note_script`]. `Err` means the command could not
/// be confirmed, so the publication may still be there.
fn reject_published_note(exec: RemoteExec, session_id: &str) -> Result<(), String> {
    validate_session_id(session_id)?;
    exec(&["sh", "-c", &reject_published_note_script(session_id)])
        .map(|_| ())
        .map_err(|e| format!("failed to remove the rejected note from the remote note outbox: {e}"))
}

/// Print the outbox as four lines: the marker line, then `meta.json` and
/// `note.md` each as one base64 line (empty when the file is missing), then
/// the marker line again. Base64 keeps arbitrary markdown — including lines
/// that look like our framing — out of the line protocol.
///
/// A marker line is `pending` when there is no `done` file, and otherwise
/// `done` followed by the generation token `write_note` wrote into it (see
/// [`MarkerLine`]). The marker is read with one `cat`, so its presence and
/// its content come from the same open, and the token is cut down to
/// letters and digits so that nothing in the file can add a line.
///
/// The marker is read twice because the three reads are separate steps: a
/// rewrite that starts after the first read (`write_note` unlinks `done`,
/// then renames a new body and a new meta into place, then recreates `done`)
/// can hand this script the new body with the old title. The tool's own
/// ordering guarantees that `done` is absent for the whole of such a
/// rewrite, which catches a rewrite still in flight at the second read. A
/// rewrite that *finished* between the reads has put a marker back, so
/// presence alone does not tell it from the first call's: the token does,
/// because every call writes a new one. Only the same token before and
/// after the reads brackets a title and body from the same call;
/// [`parse_collect_output`] treats any other combination as pending. Always
/// exits 0.
pub(crate) fn collect_script(session_id: &str) -> String {
    let dir = session_dir_expr(session_id);
    format!(
        "d={dir}; \
         m() {{ if t=$(cat \"$d/done\" 2>/dev/null); \
         then echo \"done $(printf %s \"$t\" | tr -cd 0-9A-Za-z)\"; else echo pending; fi; }}; \
         m; \
         if [ -f \"$d/meta.json\" ]; then base64 < \"$d/meta.json\" | tr -d '\\n'; fi; echo; \
         if [ -f \"$d/note.md\" ]; then base64 < \"$d/note.md\" | tr -d '\\n'; fi; echo; \
         m; \
         exit 0"
    )
}

/// Whether `script` is a [`collect_script`], for test doubles of the remote
/// runner that answer it with a scripted outbox. It is the only script that
/// reads the marker's content; the clears only test that the file exists.
#[cfg(test)]
pub(crate) fn is_collect_script(script: &str) -> bool {
    script.contains("cat \"$d/done\"")
}

/// One marker line of the collect script's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerLine<'a> {
    /// No `done` file.
    Pending,
    /// A `done` file holding this generation token. `write_note` writes a
    /// new random token on every call; a script from before version 6 wrote
    /// an empty file, which reads as the empty token.
    Done(&'a str),
}

impl<'a> MarkerLine<'a> {
    /// `pending`, or `done` alone or followed by a token; anything else is
    /// not a marker line.
    fn parse(line: &'a str) -> Option<Self> {
        if line == "pending" {
            return Some(Self::Pending);
        }
        let token = line.strip_prefix("done")?;
        if token.is_empty() {
            return Some(Self::Done(""));
        }
        token
            .starts_with(char::is_whitespace)
            .then(|| Self::Done(token.trim()))
    }
}

/// The first `SERVER_VERSION` of the script whose `meta.json` holds the title
/// exactly as it is to be stored.
///
/// Up to version 7 the title is always the call's `title` argument, and the
/// collector normalizes it the way the local tool normalizes that argument
/// ([`NoteSubmission::from_parts`]: one leading `#` is stripped). From
/// version 8 the title may be the draft's heading text, which the local
/// handler stores as written ([`NoteSubmission::from_draft`]); stripping a
/// `#` from it here would title the draft `# #12 follow-up` as
/// `12 follow-up` for a remote session and `#12 follow-up` for a local one.
/// So a version 8 script settles the title itself, for a `title` argument
/// too, and the collector takes it as written
/// ([`NoteSubmission::from_parts_as_written`]).
///
/// An outbox with an earlier version, or none, is collected as it always
/// was: a turn that outlives its connection keeps the script it started
/// with, so such outboxes are still written after an update.
pub(crate) const TITLE_AS_WRITTEN_SERVER_VERSION: u32 = 8;

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
    let first_line = lines.next();
    let Some(before) = first_line.and_then(MarkerLine::parse) else {
        return Err(format!(
            "unexpected outbox status line {first_line:?} in {output:?}"
        ));
    };
    let meta_line = lines.next();
    let body_line = lines.next();
    // The marker re-read after both files (see `collect_script`). Output
    // with only three lines (no re-read) is read by its first line alone. A
    // marker that was missing at either read, or that carries a different
    // token after the files than before them, means the files may straddle
    // two calls, so the outbox counts as pending for this read. Two empty
    // tokens are the same marker as far as anyone can tell: that is a note
    // published by a script that wrote no token, collected as it always was.
    let done = match lines.next() {
        None => matches!(before, MarkerLine::Done(_)),
        last_line @ Some(line) => match MarkerLine::parse(line) {
            Some(after) => matches!(before, MarkerLine::Done(_)) && before == after,
            None => {
                return Err(format!(
                    "unexpected trailing outbox status line {last_line:?} in {output:?}"
                ))
            }
        },
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
    let title_as_written = meta
        .server_version
        .is_some_and(|version| version >= TITLE_AS_WRITTEN_SERVER_VERSION);
    let from_outbox = if title_as_written {
        NoteSubmission::from_parts_as_written
    } else {
        NoteSubmission::from_parts
    };
    let submission = from_outbox(
        &meta.title,
        &body,
        meta.suggested_next_steps,
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
    collect_with_written_at(exec, session_id, expected_turn).map(|(state, _, _)| state)
}

/// [`collect_classified`], also returning the `writtenAt` stamp of a complete
/// note (`write_note` writes whole seconds) and the checksums of its files,
/// for callers that need to tell one `done` from the next ([`CollectedNote`]).
fn collect_with_written_at(
    exec: RemoteExec,
    session_id: &str,
    expected_turn: Option<&str>,
) -> Result<(OutboxState, Option<i64>, Option<PublishedNoteStamp>), CollectError> {
    validate_session_id(session_id).map_err(CollectError::Transport)?;
    let output = exec(&["sh", "-c", &collect_script(session_id)]).map_err(|e| {
        CollectError::Transport(format!("failed to read the remote note outbox: {e}"))
    })?;
    let snapshot = parse_collect_output(&output).map_err(CollectError::Content)?;
    let written_at = snapshot.meta.as_ref().and_then(|meta| meta.written_at);
    let state = outbox_state(snapshot, expected_turn).map_err(CollectError::Content)?;
    let stamp = PublishedNoteStamp::of_collect_output(&output);
    Ok((state, written_at, stamp))
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
    /// The note's files as they sat on the workstation, for the end-of-run
    /// clear of a run that rejected this note
    /// ([`clear_unpublished_outbox_script`]).
    pub stamp: Option<PublishedNoteStamp>,
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
    let (state, written_at, stamp) = collect_with_written_at(exec, session_id, Some(turn_id))
        .map_err(|e| e.message().to_string())?;
    if let OutboxState::Delivered(submission) = &state {
        crate::note_tool::store_submission(slot, submission.clone());
        *memory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(CollectedNote {
            written_at,
            submission: submission.clone(),
            stamp,
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
    let (state, written_at, stamp) = collect_with_written_at(exec, session_id, Some(turn_id))
        .map_err(|e| e.message().to_string())?;
    let OutboxState::Delivered(submission) = state else {
        return Ok(state);
    };
    let found = CollectedNote {
        written_at,
        submission,
        stamp,
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

/// A published note [`recover_uncollected_outbox`] found and did not store,
/// because a Pikchr block in it does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RejectedNote {
    /// The title it was published under.
    pub title: String,
    /// The first block that does not parse. Its line number counts the
    /// lines of the published body, which is the draft from its first
    /// non-blank line.
    pub error: crate::pikchr_validation::PikchrValidationError,
}

/// What [`recover_uncollected_outbox`] found at session start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutboxRecovery {
    /// Nothing complete was waiting; whatever partial files were there are gone.
    Empty,
    /// A finished note from an earlier run was handed to `ingest`, then cleared.
    Ingested,
    /// The outbox held something marked `done` that the collector cannot
    /// turn into a note (a file missing beside the marker, a body the tool
    /// would reject). It was logged and cleared so the session is not stuck
    /// on it forever.
    Discarded,
    /// A finished note from an earlier run has a Pikchr block that does not
    /// parse. It was not stored. Its publication was removed and its draft
    /// kept ([`DraftSeed::Kept`]), and the run that is starting asks the
    /// agent to repair the draft and publish it again.
    Rejected(RejectedNote),
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
/// The waiting note is checked before it is handed over: a note with a
/// Pikchr block that does not parse is not stored. It is reported as
/// [`OutboxRecovery::Rejected`] with the parse error, and the run that is
/// starting is the repair: its first prompt carries the correction
/// (`session_runner`'s `first_turn_prompt`). So that the agent has
/// something to repair, only the publication is removed
/// ([`reject_published_note_script`]): `draft.md` stays as the earlier run
/// wrote it, `seed` is not asked, and the result carries
/// [`DraftSeed::Kept`]. This is where a row the detached sweep parked on a
/// broken diagram ends up when the user resumes it, and it also covers a
/// note an earlier run rejected, had republished still broken, and then
/// never read again: that note carries a new `writtenAt`, so the earlier
/// run's guarded clear kept it as one nobody had read
/// ([`clear_unpublished_outbox_script`]).
///
/// Two failures leave the outbox alone for the next attempt and are errors
/// here: a workstation that could not be reached (at the read, the clear,
/// or the removal of a rejected publication), and a waiting note that
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
            match crate::pikchr_validation::validate_pikchr_blocks(&submission.body)
                .into_iter()
                .next()
            {
                Some(pikchr_error) => {
                    log::warn!(
                        "Session {session_id}: the workstation outbox holds an uncollected \
                         note \"{}\" from an earlier run whose Pikchr block at line {} does \
                         not parse ({}); not storing it, keeping its draft for this run to \
                         repair",
                        submission.title,
                        pikchr_error.line_number,
                        pikchr_error.message
                    );
                    reject_published_note(exec, session_id)?;
                    return Ok(RunStartRecovery {
                        outbox: OutboxRecovery::Rejected(RejectedNote {
                            title: submission.title,
                            error: pikchr_error,
                        }),
                        draft: DraftSeed::Kept,
                    });
                }
                None => {
                    log::warn!(
                        "Session {session_id}: the workstation outbox holds an uncollected \
                         note \"{}\" from an earlier run; ingesting it before this run starts",
                        submission.title
                    );
                    let title = submission.title.clone();
                    if !ingest(submission) {
                        return Err(format!(
                            "the uncollected note \"{title}\" could not be stored; leaving it \
                             in the outbox"
                        ));
                    }
                    OutboxRecovery::Ingested
                }
            }
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
    use crate::note_tool::WriteNoteParams;
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
    }

    #[test]
    fn a_marker_whose_token_changed_during_the_reads_makes_the_snapshot_pending() {
        let meta = br#"{"title":"T","turn":"t1"}"#;
        let bracketed = |before: &str, after: &str| {
            format!("{before}\n{}\n{}\n{after}\n", b64(meta), b64(b"B"))
        };
        let done = |before: &str, after: &str| {
            parse_collect_output(&bracketed(before, after))
                .unwrap()
                .done
        };

        assert!(
            done("done 0a1b", "done 0a1b"),
            "one call's marker brackets the files"
        );
        // A whole `write_note` call fitted between the two marker reads: the
        // marker is present both times, but it is not the same marker, so
        // the title and the body may come from different calls.
        let straddled = parse_collect_output(&bracketed("done 0a1b", "done 9f8e")).unwrap();
        assert!(!straddled.done, "two calls' markers: not one note");
        assert!(
            straddled.meta.is_some() && straddled.body.is_some(),
            "the files themselves are still reported"
        );
        assert_eq!(
            outbox_state(straddled, Some("t1")).unwrap(),
            OutboxState::Pending
        );

        // A script from before the token wrote an empty marker. Two empty
        // tokens are read as they always were, so a note it published is
        // still collected.
        assert!(done("done", "done"));
        assert!(done("done ", "done "), "what `echo \"done $t\"` prints");
        // One empty and one not is two different markers, in either order.
        assert!(!done("done", "done 0a1b"));
        assert!(!done("done 0a1b", "done"));
        // The token does not rescue a marker that was absent at either read.
        assert!(!done("pending", "done 0a1b"));
        assert!(!done("done 0a1b", "pending"));

        // Three lines (no marker re-read) still parse, token or not.
        let three_lines = format!("done 0a1b\n{}\n{}\n", b64(meta), b64(b"B"));
        assert!(parse_collect_output(&three_lines).unwrap().done);

        // Only `done`, alone or followed by a token, is a marker line.
        assert!(parse_collect_output(&bracketed("done0a1b", "done0a1b")).is_err());
        assert!(parse_collect_output(&bracketed("done 0a1b", "done0a1b")).is_err());
        assert!(parse_collect_output(&bracketed("pending 0a1b", "pending")).is_err());
    }

    /// The four lines the collect script prints against `home`, under `sh`.
    fn collect_lines(home: &std::path::Path) -> Vec<String> {
        let output = local_exec(home)(&["sh", "-c", &collect_script("sess-1")]).unwrap();
        let lines: Vec<String> = output.lines().map(str::to_string).collect();
        assert_eq!(lines.len(), 4, "{output:?}");
        lines
    }

    #[test]
    fn the_collect_script_prints_the_marker_token_on_both_marker_lines() {
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join(".staged/sessions/sess-1");
        let marker_lines = || {
            let lines = collect_lines(home.path());
            (lines[0].clone(), lines[3].clone())
        };
        let both = |line: &str| (line.to_string(), line.to_string());

        assert_eq!(marker_lines(), both("pending"), "no session directory");
        std::fs::create_dir_all(&outbox).unwrap();
        assert_eq!(marker_lines(), both("pending"), "no marker");

        let token = "00112233445566778899aabbccddeeff";
        std::fs::write(outbox.join("done"), token).unwrap();
        assert_eq!(marker_lines(), both(&format!("done {token}")));

        // The empty marker of a script from before the token.
        std::fs::write(outbox.join("done"), "").unwrap();
        let (before, after) = marker_lines();
        assert_eq!((before.trim(), after.trim()), ("done", "done"));

        // Whatever else ends up in the marker cannot break the line framing:
        // the token is cut down to letters and digits on one line.
        std::fs::write(outbox.join("done"), "ab cd\nef\n$(echo x) `y`\n").unwrap();
        assert_eq!(marker_lines(), both("done abcdefechoxy"));

        // Not a readable file: no marker.
        std::fs::remove_file(outbox.join("done")).unwrap();
        std::fs::create_dir(outbox.join("done")).unwrap();
        assert_eq!(marker_lines(), both("pending"));
    }

    #[test]
    fn a_whole_rewrite_between_the_file_reads_is_never_collected_as_one_note() {
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join(".staged/sessions/sess-1");
        let second = home.path().join("second-publication");
        let publish = |dir: &std::path::Path, title: &str, body: &str, token: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("note.md"), body).unwrap();
            std::fs::write(
                dir.join("meta.json"),
                format!(
                    r#"{{"title":"{title}","suggestedNextSteps":null,"turn":"turn-1","writtenAt":1,"helperVersion":6}}"#
                ),
            )
            .unwrap();
            std::fs::write(dir.join("done"), token).unwrap();
        };
        publish(
            &outbox,
            "First",
            "first body",
            "11111111111111111111111111111111",
        );
        publish(
            &second,
            "Second",
            "second body",
            "22222222222222222222222222222222",
        );

        // The race, made deterministic: `base64` is wrapped in a shell
        // function that, once the collect script has read `meta.json`, does
        // what a second `write_note` call does, in its order: unlink the
        // marker, replace `note.md`, replace `meta.json`, write the marker.
        // The script then reads the new body and finds a marker again.
        let rewrite_after_the_first_read = format!(
            "base64() {{ command base64 \"$@\"; s='{second}'; \
             if [ -f \"$s/done\" ]; then rm -f \"$d/done\"; \
             mv \"$s/note.md\" \"$d/note.md\"; mv \"$s/meta.json\" \"$d/meta.json\"; \
             mv \"$s/done\" \"$d/done\"; fi; }}; ",
            second = second.display()
        );
        let run = local_exec(home.path());
        let exec = |args: &[&str]| -> Result<String, String> {
            assert_eq!(args[..2], ["sh", "-c"]);
            run(&[
                "sh",
                "-c",
                &format!("{rewrite_after_the_first_read}{}", args[2]),
            ])
        };

        assert_eq!(
            collect(&exec, "sess-1", Some("turn-1")).unwrap(),
            OutboxState::Pending,
            "the first call's title with the second call's body is not a note"
        );
        assert!(
            !second.join("done").exists(),
            "the rewrite ran during that collection"
        );
        match collect(&exec, "sess-1", Some("turn-1")).unwrap() {
            OutboxState::Delivered(submission) => {
                assert_eq!(submission.title, "Second");
                assert_eq!(submission.body, "second body");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_write_note_call_writes_a_fresh_generation_token_into_the_marker() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping write_note marker token test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join(".staged/sessions/sess-1");
        let call = |id: u32, title: &str| serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"write_note","arguments":{"title":title}}});
        let token = || std::fs::read_to_string(outbox.join("done")).unwrap();
        let is_token = |token: &str| {
            token.len() == 32
                && token
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };

        write_draft(home.path(), "body");
        let first = run_script(home.path(), &node, &[call(1, "First")]);
        assert_eq!(first[0]["result"]["isError"], false);
        let first_token = token();
        assert!(is_token(&first_token), "{first_token:?}");

        // The same title and the same draft, republished: the files are
        // byte for byte what they were (bar the timestamp), and the marker
        // still tells the two calls apart. Two calls in one server process
        // differ as well.
        let again = run_script(home.path(), &node, &[call(2, "First"), call(3, "First")]);
        assert_eq!(again[0]["result"]["isError"], false);
        assert_eq!(again[1]["result"]["isError"], false);
        let last_token = token();
        assert!(is_token(&last_token), "{last_token:?}");
        assert_ne!(first_token, last_token);

        // The collect script reports that token on both marker lines, and
        // the note is collected.
        let lines = collect_lines(home.path());
        assert_eq!(lines[0], format!("done {last_token}"));
        assert_eq!(lines[3], lines[0]);
        assert!(matches!(
            collect(&local_exec(home.path()), "sess-1", Some("turn-1")).unwrap(),
            OutboxState::Delivered(submission) if submission.title == "First"
        ));
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

    /// The collector applies the tool's own title and body rules to what the
    /// outbox holds, and names the outbox in each rejection.
    #[test]
    fn outbox_state_normalizes_and_validates_the_outbox_title_and_body() {
        let snapshot = |title: &str, body: &str| {
            let mut snapshot = done_snapshot(Some("t1"));
            let meta = snapshot.meta.as_mut().unwrap();
            meta.title = title.to_string();
            meta.suggested_next_steps = Some(SuggestedNextSteps {
                suggested_next_commit_step: Some("  Commit ".into()),
                suggested_next_note_step: Some("  ".into()),
            });
            snapshot.body = Some(body.to_string());
            snapshot
        };

        assert_eq!(
            outbox_state(snapshot("  # Title  ", "\nBody\n"), Some("t1")).unwrap(),
            OutboxState::Delivered(NoteSubmission {
                title: "Title".into(),
                body: "Body".into(),
                suggested_next_steps: Some(SuggestedNextSteps {
                    suggested_next_commit_step: Some("Commit".into()),
                    suggested_next_note_step: None,
                }),
                source: NoteSubmissionSource::Outbox,
            })
        );

        let rejected =
            |title: &str, body: &str| outbox_state(snapshot(title, body), Some("t1")).unwrap_err();
        assert_eq!(
            rejected("  ", "Body"),
            "outbox note is invalid: `title` must not be empty"
        );
        assert_eq!(
            rejected("a\nb", "Body"),
            "outbox note is invalid: `title` must be a single line"
        );
        assert_eq!(
            rejected("Title", " \n"),
            "outbox note is invalid: the note body must not be empty"
        );
    }

    /// From version 8 the script settles the title, so the collector stores
    /// it as written: a title taken from the draft heading keeps a `#` of
    /// its own, as it does for a local session. An outbox from an earlier
    /// script, or one that names no version, is normalized as it always was.
    #[test]
    fn outbox_state_keeps_a_version_8_title_as_written_and_normalizes_an_earlier_one() {
        assert_eq!(TITLE_AS_WRITTEN_SERVER_VERSION, 8);
        let snapshot = |version: Option<u32>, title: &str, body: &str| {
            let mut snapshot = done_snapshot(Some("t1"));
            let meta = snapshot.meta.as_mut().unwrap();
            meta.title = title.to_string();
            meta.server_version = version;
            snapshot.body = Some(body.to_string());
            snapshot
        };
        let title_of = |version: Option<u32>, title: &str| match outbox_state(
            snapshot(version, title, "Body"),
            Some("t1"),
        )
        .unwrap()
        {
            OutboxState::Delivered(submission) => submission.title,
            other => panic!("{other:?}"),
        };

        // (title in meta.json, stored before version 8, stored from it on)
        for (written, before, from_8) in [
            ("Plan", "Plan", "Plan"),
            ("  Padded  ", "Padded", "Padded"),
            ("#12 follow-up", "12 follow-up", "#12 follow-up"),
            ("# Plan", "Plan", "# Plan"),
            (
                "Ends with a hash #",
                "Ends with a hash #",
                "Ends with a hash #",
            ),
        ] {
            for version in [None, Some(1), Some(6), Some(7)] {
                assert_eq!(
                    title_of(version, written),
                    before,
                    "{version:?} {written:?}"
                );
            }
            for version in [Some(8), Some(9)] {
                assert_eq!(
                    title_of(version, written),
                    from_8,
                    "{version:?} {written:?}"
                );
            }
        }

        // The body and the steps are read the same way at every version.
        for version in [Some(7), Some(8)] {
            let mut with_steps = snapshot(version, "Title", "\nBody\n");
            with_steps.meta.as_mut().unwrap().suggested_next_steps = Some(SuggestedNextSteps {
                suggested_next_commit_step: Some("  Commit ".into()),
                suggested_next_note_step: Some("  ".into()),
            });
            assert_eq!(
                outbox_state(with_steps, Some("t1")).unwrap(),
                OutboxState::Delivered(NoteSubmission {
                    title: "Title".into(),
                    body: "Body".into(),
                    suggested_next_steps: Some(SuggestedNextSteps {
                        suggested_next_commit_step: Some("Commit".into()),
                        suggested_next_note_step: None,
                    }),
                    source: NoteSubmissionSource::Outbox,
                }),
                "{version:?}"
            );
        }

        // A version 8 outbox is still checked, in the same words.
        let rejected = |title: &str, body: &str| {
            outbox_state(snapshot(Some(8), title, body), Some("t1")).unwrap_err()
        };
        assert_eq!(
            rejected("  ", "Body"),
            "outbox note is invalid: `title` must not be empty"
        );
        assert_eq!(
            rejected("a\nb", "Body"),
            "outbox note is invalid: `title` must be a single line"
        );
        assert_eq!(
            rejected("Title", " \n"),
            "outbox note is invalid: the note body must not be empty"
        );
        // A lone `#` is a title as written and was an empty one before.
        assert_eq!(title_of(Some(8), "#"), "#");
        assert_eq!(
            outbox_state(snapshot(Some(7), "#", "Body"), Some("t1")).unwrap_err(),
            "outbox note is invalid: `title` must not be empty"
        );
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
            if args.len() == 3 && is_collect_script(args[2]) {
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
        assert!(log.borrow().iter().all(|cmd| is_collect_script(&cmd[2])));
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
        assert!(is_collect_script(&commands[0][2]));
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

    const BROKEN_PIKCHR_BODY: &str = "Body\n```pikchr\nboxx \"bad\"\n```";

    /// What the collect script prints for a published note titled "Plan"
    /// whose body is [`BROKEN_PIKCHR_BODY`].
    fn broken_pikchr_output() -> String {
        let meta = r#"{"title":"Plan","turn":"earlier-run","writtenAt":7}"#;
        format!(
            "done\n{}\n{}\ndone\n",
            b64(meta.as_bytes()),
            b64(BROKEN_PIKCHR_BODY.as_bytes())
        )
    }

    /// A waiting note is checked before it is stored. One with a diagram
    /// that does not parse is not handed to the hooks, and it is not thrown
    /// away either: the recovery reports it with the parse error, removes
    /// only its publication, and neither removes nor seeds the draft, so the
    /// run that is starting can ask the agent to repair that file. Before
    /// the check the note went to the hooks as it was; between the check
    /// and this outcome it was cleared with its draft and the agent was told
    /// nothing.
    #[test]
    fn session_start_rejects_a_note_with_a_broken_pikchr_block_and_keeps_its_draft() {
        let log = RefCell::new(Vec::new());
        let exec = collecting_exec(&broken_pikchr_output(), &log);
        let mut ingested = 0;
        let mut seed_asked = 0;
        let recovery = recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |_| {
                ingested += 1;
                true
            },
            &mut || {
                seed_asked += 1;
                Some(b"the stored body".to_vec())
            },
        )
        .unwrap();
        let OutboxRecovery::Rejected(rejected) = &recovery.outbox else {
            panic!("expected the note to be rejected: {recovery:?}");
        };
        assert_eq!(rejected.title, "Plan");
        assert_eq!(rejected.error.line_number, 2, "{rejected:?}");
        assert!(!rejected.error.message.is_empty(), "{rejected:?}");
        assert_eq!(recovery.draft, DraftSeed::Kept);
        assert_eq!(ingested, 0, "a note that fails the check is not stored");
        assert_eq!(seed_asked, 0, "the kept draft is not overwritten by a seed");
        let commands = log.borrow();
        assert_eq!(commands.len(), 2, "collect, then the removal: {commands:?}");
        let removal = &commands[1][2];
        assert_eq!(removal, &reject_published_note_script("sess-1"));
        // The publication goes, marker first. The draft is never removed.
        let done = removal.find("rm -f \"$d/done\"").expect(removal);
        let rest = removal
            .find("rm -f \"$d/meta.json\" \"$d/note.md\"")
            .expect(removal);
        assert!(done < rest, "{removal}");
        // The draft is named twice, as the test for a missing draft and as
        // where the rejected body goes then; it is never removed, nothing
        // is seeded, and the directory stays.
        assert_eq!(removal.matches("draft.md").count(), 2, "{removal}");
        assert!(removal.contains("if [ ! -e \"$d/draft.md\" ]"), "{removal}");
        for absent in ["base64", "rmdir", ".seed"] {
            assert!(!removal.contains(absent), "{absent}: {removal}");
        }
    }

    /// The removal of a rejected publication is a round trip like the clear:
    /// when it cannot be confirmed the recovery is an error, so the run is
    /// unsettled and its end-of-run clear keeps whatever is there.
    #[test]
    fn session_start_reports_a_rejected_note_it_could_not_remove_as_unsettled() {
        let log: RefCell<Vec<Vec<String>>> = RefCell::new(Vec::new());
        let output = broken_pikchr_output();
        let exec = |args: &[&str]| -> Result<String, String> {
            log.borrow_mut()
                .push(args.iter().map(|a| a.to_string()).collect());
            if is_collect_script(args[2]) {
                Ok(output.clone())
            } else {
                Err("workstation suspended".to_string())
            }
        };
        let error = recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |_| panic!("a rejected note is not stored"),
            &mut || panic!("a rejected note's draft is not seeded"),
        )
        .unwrap_err();
        assert!(
            error.contains("rejected note") && error.contains("workstation suspended"),
            "{error}"
        );
        assert_eq!(log.borrow().len(), 2);
    }

    /// The same recovery against a real shell, in the three states the
    /// draft can be in beside a rejected publication.
    #[test]
    fn a_rejected_note_loses_its_publication_and_keeps_its_draft_against_a_local_shell() {
        fn publish_broken(outbox: &std::path::Path, draft: Option<&str>) {
            std::fs::create_dir_all(outbox).unwrap();
            for (file, content) in [
                ("note.md", BROKEN_PIKCHR_BODY),
                (
                    "meta.json",
                    r#"{"title":"Plan","turn":"earlier-run","writtenAt":7}"#,
                ),
                ("done", "0123456789abcdef0123456789abcdef"),
            ] {
                std::fs::write(outbox.join(file), content).unwrap();
            }
            if let Some(draft) = draft {
                std::fs::write(outbox.join("draft.md"), draft).unwrap();
            }
        }
        fn recover(home: &std::path::Path) -> RunStartRecovery {
            recover_uncollected_outbox(
                &local_exec(home),
                "sess-1",
                &mut |_| panic!("a rejected note is not stored"),
                &mut || panic!("a rejected note's draft is not seeded"),
            )
            .unwrap()
        }
        fn assert_rejected(home: &std::path::Path, recovery: &RunStartRecovery, draft: &str) {
            let outbox = home.join(".staged/sessions/sess-1");
            assert!(
                matches!(
                    &recovery.outbox,
                    OutboxRecovery::Rejected(RejectedNote { title, error })
                        if title == "Plan" && error.line_number == 2
                ),
                "{recovery:?}"
            );
            assert_eq!(recovery.draft, DraftSeed::Kept);
            for file in ["done", "meta.json", "note.md"] {
                assert!(!outbox.join(file).exists(), "{file} is removed");
            }
            assert_eq!(
                std::fs::read_to_string(outbox.join("draft.md")).unwrap(),
                draft
            );
            // The run that is starting cannot collect the old note as its
            // own, and a second recovery finds nothing to reject.
            assert_eq!(
                collect(&local_exec(home), "sess-1", None).unwrap(),
                OutboxState::Pending
            );
        }

        // The draft the note was published from, with an indented first
        // line and a trailing newline `write_note` trimmed off.
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join(".staged/sessions/sess-1");
        let published_from = format!("\n{BROKEN_PIKCHR_BODY}\n");
        publish_broken(&outbox, Some(&published_from));
        assert_rejected(home.path(), &recover(home.path()), &published_from);

        // A draft the lost run went on editing after it published (a repair
        // turn that fixed the block and lost its connection before the
        // call): the later text is what the agent finds.
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join(".staged/sessions/sess-1");
        let edited_since = "Body\n```pikchr\nbox \"good\"\n```\n";
        publish_broken(&outbox, Some(edited_since));
        std::fs::write(outbox.join("unrelated"), "another tool's file").unwrap();
        assert_rejected(home.path(), &recover(home.path()), edited_since);
        assert!(outbox.join("unrelated").exists());

        // No draft at all: the rejected body becomes the draft, so the file
        // the repair prompt names exists.
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join(".staged/sessions/sess-1");
        publish_broken(&outbox, None);
        assert_rejected(home.path(), &recover(home.path()), BROKEN_PIKCHR_BODY);

        // The script alone on a workstation with no session directory: it
        // exits 0 and creates nothing.
        let home = tempfile::tempdir().unwrap();
        local_exec(home.path())(&["sh", "-c", &reject_published_note_script("sess-1")]).unwrap();
        assert!(!home.path().join(".staged").exists());
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

    /// Linux's `MAX_ARG_STRLEN`: one `execve` argument, its NUL included,
    /// must be shorter than this. The workstations are Linux, the laptop
    /// that runs these tests usually is not, so the bound is checked on the
    /// commands themselves.
    const LINUX_MAX_ARG_BYTES: usize = 131_072;

    #[test]
    fn every_seeding_command_fits_one_linux_argument() {
        // A real session id is a UUID; the longest single-command seed is
        // the one whose base64 is exactly the chunk size.
        let session_id = "0a734c48-c0fc-409e-bacb-ed8afb83a442";
        let largest_single = vec![b'x'; SEED_CHUNK_SIZE / 4 * 3];
        let chunked = vec![b'x'; SEED_CHUNK_SIZE * 3];
        for (seed, expected_commands) in [(&largest_single, 1), (&chunked, 5)] {
            let log = RefCell::new(Vec::new());
            let exec = collecting_exec("pending\n\n\n", &log);
            assert_eq!(
                clear_outbox_and_seed(&exec, session_id, Some(seed)).unwrap(),
                DraftSeed::Seeded
            );
            let commands = log.borrow();
            assert_eq!(commands.len(), expected_commands);
            for command in commands.iter() {
                for arg in command {
                    assert!(
                        arg.len() < LINUX_MAX_ARG_BYTES,
                        "a {} byte argument does not fit one Linux argument",
                        arg.len()
                    );
                }
            }
        }
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

    /// The three files a `write_note` call leaves, as bytes; `None` for a
    /// file that is not there.
    fn outbox_files(home: &std::path::Path) -> [Option<Vec<u8>>; 3] {
        let outbox = home.join(".staged/sessions/sess-1");
        ["done", "meta.json", "note.md"].map(|name| std::fs::read(outbox.join(name)).ok())
    }

    /// One `write_note` call on the script with `arguments`: whether it was
    /// rejected, and the result text.
    fn call_write_note(
        home: &std::path::Path,
        node: &str,
        arguments: serde_json::Value,
    ) -> (bool, String) {
        let responses = run_script(
            home,
            node,
            &[
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":arguments}}),
            ],
        );
        let result = &responses[0]["result"];
        (
            result["isError"] == true,
            result["content"][0]["text"]
                .as_str()
                .unwrap_or_else(|| panic!("a text result: {responses:?}"))
                .to_string(),
        )
    }

    /// A call without `title` takes the title from the draft's first line:
    /// `meta.json` carries the parsed title, `note.md` the draft without the
    /// heading line, and the collector hands both on unchanged.
    #[test]
    fn write_note_without_a_title_publishes_the_draft_heading_as_the_title() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping the draft heading test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");

        let draft =
            "\n\n# Design the outbox\n\nFirst body ![Shot](staged-media://abc.png)\n\n## Details\n";
        let body = "First body ![Shot](staged-media://abc.png)\n\n## Details";
        write_draft(home.path(), draft);
        let (is_error, text) = call_write_note(
            home.path(),
            &node,
            serde_json::json!({"suggested_next_steps":{"suggestedNextCommitStep":" Fix it ","suggestedNextNoteStep":""}}),
        );
        assert!(!is_error, "{text}");
        // The result echoes the parsed title, and the count is the body
        // without the heading line.
        assert_eq!(
            text,
            crate::note_tool::saved_message("Design the outbox", body.len())
        );

        assert!(outbox.join("done").exists());
        assert_eq!(
            std::fs::read_to_string(outbox.join("note.md")).unwrap(),
            body,
            "note.md has no heading"
        );
        assert_eq!(
            std::fs::read_to_string(outbox.join("draft.md")).unwrap(),
            draft,
            "the script only reads the draft"
        );
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(outbox.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["title"], "Design the outbox");
        assert_eq!(meta["turn"], "turn-1");
        assert_eq!(
            meta["suggestedNextSteps"],
            serde_json::json!({"suggestedNextCommitStep":"Fix it","suggestedNextNoteStep":null})
        );
        // No key was added for this: the version is what tells the laptop
        // the title is to be stored as written.
        let mut keys: Vec<&str> = meta
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "helperVersion",
                "suggestedNextSteps",
                "title",
                "turn",
                "writtenAt"
            ]
        );
        assert_eq!(
            meta["helperVersion"],
            u64::from(TITLE_AS_WRITTEN_SERVER_VERSION),
            "the script's version is the one the collector's rule starts at"
        );

        assert_eq!(
            collect(&exec, "sess-1", Some("turn-1")).unwrap(),
            OutboxState::Delivered(NoteSubmission {
                title: "Design the outbox".into(),
                body: body.into(),
                suggested_next_steps: Some(SuggestedNextSteps {
                    suggested_next_commit_step: Some("Fix it".into()),
                    suggested_next_note_step: None,
                }),
                source: NoteSubmissionSource::Outbox,
            })
        );

        // (draft, title, body)
        let cases = [
            // Only the first line is consumed: a body that opens with its
            // own H1 keeps it.
            ("# Title\n\n# Section\n\nText", "Title", "# Section\n\nText"),
            // The title text is kept as written, a `#` of its own included,
            // on the workstation and through the collector.
            ("# #12 follow-up\nBody", "#12 follow-up", "Body"),
            ("# # Spaced\nBody", "# Spaced", "Body"),
            (
                "# Use `foo` for [bar](x) #\nBody",
                "Use `foo` for [bar](x) #",
                "Body",
            ),
            ("#\t Tabbed \t\nBody", "Tabbed", "Body"),
            ("\u{feff}\n\n# Marked\r\n\r\nBody\r\n", "Marked", "Body"),
        ];
        for (draft, title, body) in cases {
            write_draft(home.path(), draft);
            // `title` may be left out or null.
            for arguments in [serde_json::json!({}), serde_json::json!({"title": null})] {
                let (is_error, text) = call_write_note(home.path(), &node, arguments.clone());
                assert!(!is_error, "{draft:?} {arguments}: {text}");
                assert_eq!(
                    text,
                    crate::note_tool::saved_message(title, body.len()),
                    "{draft:?}"
                );
                let meta: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(outbox.join("meta.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(meta["title"], title, "{draft:?}");
                assert_eq!(
                    meta["suggestedNextSteps"],
                    serde_json::Value::Null,
                    "{draft:?}"
                );
                assert_eq!(
                    std::fs::read_to_string(outbox.join("note.md")).unwrap(),
                    body,
                    "{draft:?}"
                );
                assert_eq!(
                    collect(&exec, "sess-1", Some("turn-1")).unwrap(),
                    OutboxState::Delivered(NoteSubmission {
                        title: title.into(),
                        body: body.into(),
                        suggested_next_steps: None,
                        source: NoteSubmissionSource::Outbox,
                    }),
                    "{draft:?}"
                );
            }
        }
    }

    /// With no `title`, a draft that does not open with a `# Title` line, or
    /// has nothing under it, is rejected in the laptop's words and the
    /// outbox keeps what it held: nothing, or the note of an earlier call.
    #[test]
    fn write_note_without_a_title_rejects_a_draft_with_no_heading_and_leaves_the_outbox_untouched()
    {
        use crate::note_draft::DraftError;

        let Some(node) = node() else {
            eprintln!("node not available; skipping the missing heading test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");
        let shown = outbox.join("draft.md").to_string_lossy().into_owned();

        let cases = [
            ("Plain text\n\nBody", DraftError::MissingTitle),
            ("## Section\n\nBody", DraftError::MissingTitle),
            ("### Deeper\n\nBody", DraftError::MissingTitle),
            ("#\nBody", DraftError::MissingTitle),
            ("#   \nBody", DraftError::MissingTitle),
            ("#Title\n\nBody", DraftError::MissingTitle),
            ("#note:6c48d5ac\n\nBody", DraftError::MissingTitle),
            ("Title\n=====\n\nBody", DraftError::MissingTitle),
            ("---\ntitle: T\n---\n\nBody", DraftError::MissingTitle),
            ("Intro line\n# Title\n\nBody", DraftError::MissingTitle),
            ("# Title", DraftError::TitleOnly),
            ("\n# Title\n\n  \n", DraftError::TitleOnly),
        ];

        // An empty outbox stays empty: only the draft is in the directory.
        for (draft, error) in &cases {
            write_draft(home.path(), draft);
            let (is_error, text) = call_write_note(home.path(), &node, serde_json::json!({}));
            assert!(is_error, "{draft:?}: {text}");
            assert_eq!(text, error.describe(&shown), "{draft:?}");
            assert_eq!(outbox_files(home.path()), [None, None, None], "{draft:?}");
            assert_eq!(
                collect(&exec, "sess-1", Some("turn-1")).unwrap(),
                OutboxState::Pending,
                "{draft:?}"
            );
        }

        // The texts, word for word.
        write_draft(home.path(), "## Section\n\nBody");
        assert_eq!(
            call_write_note(home.path(), &node, serde_json::json!({"title": null})).1,
            format!(
                "write_note rejected: the draft file `{shown}` does not start with the note's \
title. Its first non-blank line must be a level-1 markdown heading: `# ` followed by the title \
on one line (not `##`, not a line underlined with `===`, and nothing above it). Add that line \
at the top and leave the rest of the note below it, then call write_note again."
            )
        );
        write_draft(home.path(), "# Title\n");
        assert_eq!(
            call_write_note(home.path(), &node, serde_json::json!({})).1,
            format!(
                "write_note rejected: the draft file `{shown}` holds only the title line. \
Write the note body in markdown below the `# ` title line, then call write_note again."
            )
        );

        // An outbox filled by an earlier call keeps that note, marker token
        // included.
        write_draft(home.path(), "# Kept\n\nKept body");
        let (is_error, text) = call_write_note(home.path(), &node, serde_json::json!({}));
        assert!(!is_error, "{text}");
        let kept = outbox_files(home.path());
        assert!(kept.iter().all(Option::is_some), "{kept:?}");
        for (draft, _) in &cases {
            write_draft(home.path(), draft);
            let (is_error, text) = call_write_note(home.path(), &node, serde_json::json!({}));
            assert!(is_error, "{draft:?}: {text}");
            assert_eq!(
                outbox_files(home.path()),
                kept,
                "{draft:?} must not replace the earlier note"
            );
        }
        assert!(matches!(
            collect(&exec, "sess-1", Some("turn-1")).unwrap(),
            OutboxState::Delivered(submission)
                if submission.title == "Kept" && submission.body == "Kept body"
        ));

        // Bad suggested next steps are refused before the draft is read,
        // and an unreadable draft before its heading is looked for.
        let (is_error, text) = call_write_note(
            home.path(),
            &node,
            serde_json::json!({"suggested_next_steps": "soon"}),
        );
        assert!(is_error);
        assert_eq!(
            text,
            "write_note rejected: `suggested_next_steps` must be an object or null."
        );
        std::fs::remove_file(outbox.join("draft.md")).unwrap();
        let (_, without) = call_write_note(home.path(), &node, serde_json::json!({}));
        let (_, with) = call_write_note(home.path(), &node, serde_json::json!({"title": "Title"}));
        assert_eq!(without, DraftError::Missing.describe(&shown));
        assert_eq!(without, with);
        write_draft(home.path(), "  \n\t\n");
        let (_, without) = call_write_note(home.path(), &node, serde_json::json!({}));
        assert_eq!(without, DraftError::Empty.describe(&shown));
        assert_eq!(outbox_files(home.path()), kept);
    }

    /// A call that passes `title` is handled as it was before the script
    /// could read a heading: the whole draft is the body, the result text
    /// names the title as the script normalized it, and the note is stored
    /// under the title a version 7 outbox of the same call was collected
    /// with. The laptop no longer strips a `#` from a version 8 title, so
    /// the script has to write the stored title itself.
    #[test]
    fn a_call_that_passes_a_title_stores_what_it_stored_before_version_8() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping the passed title test");
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");

        // What a version 7 script left for the same call: the title of its
        // result text in `meta.json`, the same body in `note.md`.
        let collected_from_version_7 = |title: &str, body: &str| {
            let mut snapshot = done_snapshot(Some("turn-1"));
            let meta = snapshot.meta.as_mut().unwrap();
            meta.title = title.to_string();
            meta.suggested_next_steps = None;
            meta.server_version = Some(7);
            snapshot.body = Some(body.to_string());
            outbox_state(snapshot, Some("turn-1"))
        };

        // (draft, `title` argument, title in the result text, stored title)
        let cases = [
            // A heading at the top of the draft stays in the body, whether
            // it differs from the argument or repeats it.
            (
                "# Draft heading\n\nBody",
                "Passed title",
                "Passed title",
                "Passed title",
            ),
            ("# Same\n\nBody", "Same", "Same", "Same"),
            ("# Only a heading", "Passed", "Passed", "Passed"),
            // Drafts the no-title path rejects are accepted with a title.
            ("Plain body", "Passed", "Passed", "Passed"),
            ("## Section\n\nText", "Passed", "Passed", "Passed"),
            // The argument is trimmed and loses one leading `#`.
            ("Body", "  # Design  ", "Design", "Design"),
            (
                "# #12 follow-up\n\nBody",
                "#12 follow-up",
                "12 follow-up",
                "12 follow-up",
            ),
            // The laptop took one more `#` off a title that still had one.
            ("Body", "## Plan", "# Plan", "Plan"),
            ("Body", "# # Plan", "# Plan", "Plan"),
        ];
        for (draft, argument, shown_title, stored_title) in cases {
            write_draft(home.path(), draft);
            let body = draft.trim();
            let (is_error, text) =
                call_write_note(home.path(), &node, serde_json::json!({"title": argument}));
            assert!(!is_error, "{argument:?}: {text}");
            assert_eq!(
                text,
                crate::note_tool::saved_message(shown_title, body.len()),
                "{argument:?}"
            );
            assert_eq!(
                std::fs::read_to_string(outbox.join("note.md")).unwrap(),
                body,
                "{argument:?}"
            );
            let collected = collect(&exec, "sess-1", Some("turn-1"));
            assert_eq!(
                collected,
                Ok(OutboxState::Delivered(NoteSubmission {
                    title: stored_title.into(),
                    body: body.into(),
                    suggested_next_steps: None,
                    source: NoteSubmissionSource::Outbox,
                })),
                "{argument:?}"
            );
            assert_eq!(
                collected,
                collected_from_version_7(shown_title, body),
                "{argument:?}"
            );
        }

        // A title that is only `#` marks was saved on the workstation and
        // refused by the collector. It still is, in the same words.
        write_draft(home.path(), "Body");
        let (is_error, text) =
            call_write_note(home.path(), &node, serde_json::json!({"title": "##"}));
        assert!(!is_error, "{text}");
        assert_eq!(text, crate::note_tool::saved_message("#", "Body".len()));
        let collected = collect(&exec, "sess-1", Some("turn-1"));
        assert_eq!(
            collected,
            Err("outbox note is invalid: `title` must not be empty".to_string())
        );
        assert_eq!(collected, collected_from_version_7("#", "Body"));

        // The rejections of a bad `title` read as before, and a heading in
        // the draft does not stand in for it.
        write_draft(home.path(), "# Draft heading\n\nBody");
        let before = outbox_files(home.path());
        for (argument, text) in [
            (
                serde_json::json!(""),
                "write_note rejected: `title` must not be empty. Call it again with a non-empty \
one-line title.",
            ),
            (
                serde_json::json!("# "),
                "write_note rejected: `title` must not be empty. Call it again with a non-empty \
one-line title.",
            ),
            (
                serde_json::json!("Two\nlines"),
                "write_note rejected: `title` must be a single line. Call it again with a \
one-line title.",
            ),
            (
                serde_json::json!(12),
                "write_note rejected: `title` must be a string. Call it again with a non-empty \
one-line title.",
            ),
        ] {
            let (is_error, result) =
                call_write_note(home.path(), &node, serde_json::json!({"title": argument}));
            assert!(is_error, "{argument}");
            assert_eq!(result, text, "{argument}");
            assert_eq!(outbox_files(home.path()), before, "{argument}");
        }
    }

    /// A turn that outlives its connection keeps the script it started
    /// with, so the laptop still meets outboxes a version 7 script wrote:
    /// the `title` argument in `meta.json`, the whole draft in `note.md`.
    /// They collect as they did, the title normalized by the laptop and a
    /// heading at the top of the body left where it is.
    #[test]
    fn an_outbox_written_by_a_version_7_script_still_collects() {
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");
        std::fs::create_dir_all(&outbox).unwrap();

        let write_outbox = |title: &str, body: &str, version: Option<u32>| {
            let mut meta = serde_json::json!({
                "title": title,
                "suggestedNextSteps": {"suggestedNextCommitStep": "Ship", "suggestedNextNoteStep": null},
                "turn": "turn-1",
                "writtenAt": 1,
            });
            if let Some(version) = version {
                meta["helperVersion"] = version.into();
            }
            std::fs::write(outbox.join("note.md"), body).unwrap();
            std::fs::write(outbox.join("meta.json"), meta.to_string()).unwrap();
            std::fs::write(outbox.join("done"), "0123456789abcdef0123456789abcdef").unwrap();
        };
        let collected = |title: &str, body: &str| {
            Ok(OutboxState::Delivered(NoteSubmission {
                title: title.into(),
                body: body.into(),
                suggested_next_steps: Some(SuggestedNextSteps {
                    suggested_next_commit_step: Some("Ship".into()),
                    suggested_next_note_step: None,
                }),
                source: NoteSubmissionSource::Outbox,
            }))
        };

        // (title in meta.json, note.md, collected title)
        let cases = [
            ("Plan", "Body", "Plan"),
            // The body is the whole draft: its heading is not a title.
            ("Plan", "# Draft heading\n\nBody", "Plan"),
            // The laptop strips one `#` from an earlier script's title.
            ("# Plan", "Body", "Plan"),
            ("#12 follow-up", "## Section\n\nText", "12 follow-up"),
        ];
        for (title, body, stored) in cases {
            // Versions before 8, and the first outboxes, which named none.
            for version in [Some(7), Some(3), None] {
                write_outbox(title, body, version);
                assert_eq!(
                    collect(&exec, "sess-1", Some("turn-1")),
                    collected(stored, body),
                    "{title:?} at {version:?}"
                );
                // The sweep, which has no turn of its own, reads it alike.
                assert_eq!(
                    collect(&exec, "sess-1", None),
                    collected(stored, body),
                    "{title:?} at {version:?}"
                );
            }
        }

        // The same files stamped by a version 8 script keep the title.
        write_outbox("#12 follow-up", "Body", Some(8));
        assert_eq!(
            collect(&exec, "sess-1", Some("turn-1")),
            collected("#12 follow-up", "Body")
        );

        // The run-start recovery ingests a version 7 note the same way.
        write_outbox("# Plan", "# Draft heading\n\nBody", Some(7));
        let mut ingested = Vec::new();
        let recovery = recover_uncollected_outbox(
            &exec,
            "sess-1",
            &mut |submission| {
                ingested.push((submission.title, submission.body));
                true
            },
            &mut || None,
        )
        .unwrap();
        assert_eq!(recovery.outbox, OutboxRecovery::Ingested);
        assert_eq!(
            ingested,
            [("Plan".to_string(), "# Draft heading\n\nBody".to_string())]
        );
    }

    /// The saved result is the instruction that ends a note turn: it asks for
    /// a bare "Done", which the chat pane hides. A local session gets the text
    /// from `note_tool::handle_write_note` and a Blox session from the script,
    /// so the two copies must read the same for the same title and draft, or
    /// the two kinds of session end their turns differently.
    #[test]
    fn saved_result_text_is_identical_for_local_and_remote_sessions() {
        let Some(node) = node() else {
            eprintln!("node not available; skipping saved result text comparison");
            return;
        };
        // Titles and drafts that would expose a difference in quoting,
        // trimming or counting: the count is UTF-8 bytes of the trimmed body.
        let cases = [
            ("Plan", "Body"),
            ("# Plan with a heading mark", "  Body line\n---\nmore  \n"),
            ("Fix \"quoted\" titles", "Body with \"quotes\" and a `tick`"),
            ("Né: résumé → done", "é→ in the body, 日本語 too"),
            ("  Padded title  ", "\u{feff}Body after a byte order mark"),
        ];
        for (title, draft) in cases {
            let home = tempfile::tempdir().unwrap();
            write_draft(home.path(), draft);

            let slot = crate::note_tool::new_submission_slot();
            let local = crate::note_tool::handle_write_note(
                "sess-1",
                &slot,
                &home.path().join(".staged/sessions/sess-1/draft.md"),
                serde_json::from_value(serde_json::json!({ "title": title })).unwrap(),
            );
            assert_ne!(local.is_error, Some(true), "{title:?}: {local:?}");
            let local = serde_json::to_value(&local.content).unwrap();
            let local = local[0]["text"].as_str().expect("a text result");

            let responses = run_script(
                home.path(),
                &node,
                &[
                    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write_note","arguments":{"title":title}}}),
                ],
            );
            assert_eq!(responses[0]["result"]["isError"], false, "{title:?}");
            let remote = responses[0]["result"]["content"][0]["text"]
                .as_str()
                .expect("a text result");

            assert_eq!(remote, local, "title {title:?}, draft {draft:?}");
            // And both are the shared wording, not two copies that drifted
            // the same way: the instruction after the "Saved note" sentence.
            assert!(
                local.ends_with(
                    " bytes from the draft file). If the user asked you a question this turn \
and you have not answered it in chat yet, answer it now, briefly. Otherwise send a simple \
\"Done\" message with no other text to end your turn, rather than summarising the work done."
                ),
                "{local}"
            );
        }
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
        assert!(
            lines[0].starts_with("done ") && lines[0].len() > "done ".len(),
            "the marker line carries the call's token: {output:?}"
        );
        assert_eq!(lines[3], lines[0]);
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
            if is_collect_script(args[2]) {
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
            clear_unpublished_outbox_script("sess-1", None),
            clear_unpublished_outbox_script("sess-1", Some(PublishedNoteStamp::of(b"{}", b"Body"))),
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
            clear_unpublished_outbox(&exec, "sess-1", None).unwrap(),
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
            clear_unpublished_outbox(&exec, "sess-1", None).unwrap(),
            UnreadOutboxClear::Cleared
        );
        assert!(!outbox.exists(), "{outbox:?}");

        // Nothing there at all.
        assert_eq!(
            clear_unpublished_outbox(&exec, "sess-1", None).unwrap(),
            UnreadOutboxClear::Cleared
        );

        // The marker is matched as a whole line of the script's output.
        let kept = |_: &[&str]| Ok(format!("  {PUBLISHED_NOTE_KEPT_MARKER}\n"));
        assert_eq!(
            clear_unpublished_outbox(&kept, "sess-1", None).unwrap(),
            UnreadOutboxClear::PublishedNoteKept
        );
        let unreachable = |_: &[&str]| Err::<String, String>("no route".to_string());
        assert!(clear_unpublished_outbox(&unreachable, "sess-1", None)
            .unwrap_err()
            .contains("no route"));
        assert!(
            clear_unpublished_outbox(&exec, "../sess-1", None).is_err(),
            "a session id that is not a plain name never reaches the shell"
        );
    }

    /// `FileCksum` is what `cksum` prints: the published check values, and
    /// whatever this machine's `cksum` says about the same bytes.
    #[test]
    fn file_cksum_matches_the_posix_cksum_command() {
        assert_eq!(FileCksum::of(b"").to_string(), "4294967295 0");
        assert_eq!(FileCksum::of(b"123456789").to_string(), "930766865 9");

        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let samples: [&[u8]; 4] = [
            b"Body",
            "Unicode \u{2192} caf\u{e9}\n".as_bytes(),
            &[0, 255, 10, 13, 0],
            &[b'x'; 70_000],
        ];
        for bytes in samples {
            std::fs::write(home.path().join("sample"), bytes).unwrap();
            let printed = exec(&["sh", "-c", "cksum < \"$HOME/sample\""]).unwrap();
            assert_eq!(
                printed.split_whitespace().collect::<Vec<_>>().join(" "),
                FileCksum::of(bytes).to_string(),
                "{} bytes",
                bytes.len()
            );
        }
    }

    /// Review of ea2ebbb3, warning 1. A run reads a note, rejects it (a
    /// broken Pikchr block) and asks for a repair; the repair turn ends with
    /// no read and without republishing. The guarded clear used to find the
    /// rejected note's `done` and keep it, and the next Resume's recovery
    /// ingested it unchecked. Against a real shell: the clear given the
    /// rejected note's stamp removes that note, and still keeps a note
    /// published after it.
    #[test]
    fn the_unread_clear_removes_the_note_its_run_rejected_and_keeps_a_newer_one() {
        let home = tempfile::tempdir().unwrap();
        let exec = local_exec(home.path());
        let outbox = home.path().join(".staged/sessions/sess-1");
        let publish = |written_at: u64, body: &str| {
            std::fs::create_dir_all(&outbox).unwrap();
            for (file, content) in [
                ("note.md", body.to_string()),
                (
                    "meta.json",
                    format!(r#"{{"title":"Plan","turn":"turn-1","writtenAt":{written_at}}}"#),
                ),
                ("done", String::new()),
                ("draft.md", body.to_string()),
            ] {
                std::fs::write(outbox.join(file), content).unwrap();
            }
        };
        let broken = "Body\n```pikchr\nboxx \"bad\"\n```";

        // The run's read of the note it then rejects.
        publish(1, broken);
        let slot = crate::note_tool::new_submission_slot();
        let memory = new_collected_note_memory();
        assert!(matches!(
            collect_turn_into_slot(&exec, "sess-1", "turn-1", &slot, &memory).unwrap(),
            OutboxState::Delivered(_)
        ));
        let rejected = memory.lock().unwrap().as_ref().unwrap().stamp;
        assert!(rejected.is_some(), "a collected note carries its stamp");

        // Without the stamp the marker alone decides, which is the defect:
        // the rejected note stays for the next run's recovery.
        assert_eq!(
            clear_unpublished_outbox(&exec, "sess-1", None).unwrap(),
            UnreadOutboxClear::PublishedNoteKept
        );
        assert!(matches!(
            collect(&exec, "sess-1", None).unwrap(),
            OutboxState::Delivered(_)
        ));

        // With it, the rejected note goes, draft and directory included.
        assert_eq!(
            clear_unpublished_outbox(&exec, "sess-1", rejected).unwrap(),
            UnreadOutboxClear::RejectedNoteCleared
        );
        assert!(!outbox.exists(), "{outbox:?}");
        assert_eq!(
            clear_unpublished_outbox(&exec, "sess-1", rejected).unwrap(),
            UnreadOutboxClear::Cleared,
            "nothing published at all"
        );

        // A note the repair turn published before it failed is one nobody
        // has read: a later `writtenAt`, or the same second and title with
        // another body of the same length.
        for (written_at, body) in [
            (2, broken.to_string()),
            (1, broken.replace("boxx", "boxy")),
            (2, "Body\n```pikchr\nbox \"good\"\n```".to_string()),
        ] {
            publish(written_at, &body);
            assert_eq!(
                clear_unpublished_outbox(&exec, "sess-1", rejected).unwrap(),
                UnreadOutboxClear::PublishedNoteKept,
                "writtenAt {written_at}: {body}"
            );
            assert!(
                matches!(
                    collect(&exec, "sess-1", None).unwrap(),
                    OutboxState::Delivered(submission) if submission.body == body
                ),
                "the next run's recovery still finds it"
            );
            assert_eq!(
                std::fs::read_to_string(outbox.join("draft.md")).unwrap(),
                body
            );
        }

        // A rewrite in flight (`write_note` unlinks `done` first) is not a
        // published note, with or without a stamp.
        std::fs::remove_file(outbox.join("done")).unwrap();
        assert_eq!(
            clear_unpublished_outbox(&exec, "sess-1", rejected).unwrap(),
            UnreadOutboxClear::Cleared
        );
        assert!(!outbox.exists(), "{outbox:?}");

        // The markers are matched as whole lines of the script's output.
        let cleared = |_: &[&str]| Ok(format!(" {REJECTED_NOTE_CLEARED_MARKER}\n"));
        assert_eq!(
            clear_unpublished_outbox(&cleared, "sess-1", rejected).unwrap(),
            UnreadOutboxClear::RejectedNoteCleared
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

    /// What `write_note` answered: the result text, and the title and body
    /// of the note it handed on when it accepted the draft.
    #[derive(Debug, PartialEq, Eq)]
    struct DraftOutcome {
        text: String,
        title: Option<String>,
        body: Option<String>,
    }

    /// Remove everything in the session directory, whatever kind of file.
    fn empty_session_dir(outbox: &std::path::Path) {
        for entry in std::fs::read_dir(outbox).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                std::fs::remove_dir_all(&path).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
        }
    }

    /// One `write_note` call with `arguments` on each transport, over the
    /// draft at `~/.staged/sessions/sess-1/draft.md`: the laptop's handler
    /// first, then the script. Returns `(laptop, script)`.
    ///
    /// The laptop's note is what its handler put in the slot. The script's
    /// is what the laptop's collector makes of the outbox the script wrote,
    /// so the comparison covers the whole remote path to the slot, not only
    /// the files. The session directory must hold no earlier outbox.
    fn write_note_on_both_transports(
        home: &std::path::Path,
        node: &str,
        arguments: &serde_json::Value,
        what: &str,
    ) -> (DraftOutcome, DraftOutcome) {
        let outbox = home.join(".staged/sessions/sess-1");

        let slot = crate::note_tool::new_submission_slot();
        let result = crate::note_tool::handle_write_note(
            "sess-1",
            &slot,
            &outbox.join("draft.md"),
            serde_json::from_value(arguments.clone()).unwrap(),
        );
        let content = serde_json::to_value(&result.content).unwrap();
        let submission = crate::note_tool::current_submission(&slot);
        let laptop = DraftOutcome {
            text: content[0]["text"].as_str().unwrap().to_string(),
            title: submission.as_ref().map(|s| s.title.clone()),
            body: submission.map(|s| s.body),
        };
        assert_eq!(
            result.is_error == Some(true),
            laptop.body.is_none(),
            "{what}"
        );

        let (is_error, text) = call_write_note(home, node, arguments.clone());
        let collected = match collect(&local_exec(home), "sess-1", Some("turn-1")) {
            Ok(OutboxState::Delivered(submission)) => Some(submission),
            Ok(OutboxState::Pending) => None,
            other => panic!("{what}: {other:?}"),
        };
        assert_eq!(is_error, collected.is_none(), "{what}: {text}");
        assert_eq!(
            outbox.join("done").exists(),
            !is_error,
            "{what}: a rejected draft leaves the outbox untouched"
        );
        // The collector hands on the body file as the script wrote it.
        assert_eq!(
            std::fs::read_to_string(outbox.join("note.md")).ok(),
            collected.as_ref().map(|s| s.body.clone()),
            "{what}"
        );
        let script = DraftOutcome {
            text,
            title: collected.as_ref().map(|s| s.title.clone()),
            body: collected.map(|s| s.body),
        };
        (laptop, script)
    }

    /// `write_note` has two implementations of the draft read: the laptop's
    /// (`note_draft::read_draft` behind `note_tool::handle_write_note`) and
    /// the script's `readDraft`. They must accept and reject the same files,
    /// in the same words, and publish the same body.
    ///
    /// The same holds for the split of a draft into its title and body,
    /// which a call without `title` asks for (`note_draft::split_title` and
    /// the script's `splitDraftTitle`): the same result and error texts,
    /// and the same title and body in the slot. The script's side of each
    /// comparison is the note as the laptop collects it from the outbox.
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
            (
                // With a `title`, a heading is body text on both.
                "a heading under a passed title",
                DraftFixture::Bytes("# Heading\n\nBody\n".into()),
                Ok("# Heading\n\nBody"),
            ),
        ];

        for (what, fixture, expected) in cases {
            // A clean session directory holding only the fixture.
            empty_session_dir(&outbox);
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

            // The laptop's handler, then the script, on the same file.
            let (laptop, script) = write_note_on_both_transports(
                home.path(),
                &node,
                &serde_json::json!({"title": "Title"}),
                what,
            );

            match expected {
                Ok(body) => {
                    assert_eq!(laptop.title.as_deref(), Some("Title"), "{what}");
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

        // A call without `title`: the draft's first line is the title.
        // (what, the draft or None for no file, the title and body both
        // transports hand on or the rejection)
        use crate::note_draft::DraftError;
        type Split = Result<(&'static str, &'static str), DraftError>;
        let cases: Vec<(&str, Option<&str>, Split)> = vec![
            (
                "a heading and a body",
                Some("# Design the outbox\n\nFirst body\n\n## Details\n"),
                Ok(("Design the outbox", "First body\n\n## Details")),
            ),
            (
                "blank lines and a byte order mark above the heading",
                Some("\u{feff}\n\n# Marked\n\nBody\n"),
                Ok(("Marked", "Body")),
            ),
            (
                "CRLF line endings",
                Some("# Marked\r\n\r\nBody\r\nmore\r\n"),
                Ok(("Marked", "Body\r\nmore")),
            ),
            (
                "only the first line is consumed",
                Some("# Title\n\n# Section\n\nText"),
                Ok(("Title", "# Section\n\nText")),
            ),
            (
                "a body right under the heading",
                Some("# Title\nBody"),
                Ok(("Title", "Body")),
            ),
            (
                // The laptop must not strip this `#` when it collects the
                // outbox: the handler does not strip it either.
                "a title that starts with its own hash",
                Some("# #12 follow-up\nBody"),
                Ok(("#12 follow-up", "Body")),
            ),
            (
                "a title that starts with a hash and a space",
                Some("# # Spaced\nBody"),
                Ok(("# Spaced", "Body")),
            ),
            (
                "a title that is one hash",
                Some("# #\nBody"),
                Ok(("#", "Body")),
            ),
            (
                "inline markdown and a closing hash are kept",
                Some("# Use `foo` for [bar](x) #\nBody"),
                Ok(("Use `foo` for [bar](x) #", "Body")),
            ),
            (
                "a tab after the hash",
                Some("#\t Tabbed \t\nBody"),
                Ok(("Tabbed", "Body")),
            ),
            (
                "quotes and non-ASCII text, counted in bytes",
                Some("# Fix \"quoted\" titles: résumé → 日本語\n\né→ in the body"),
                Ok(("Fix \"quoted\" titles: résumé → 日本語", "é→ in the body")),
            ),
            (
                // U+3000 and U+0085 are whitespace to Rust; JavaScript's own
                // `trim` would keep the second.
                "unicode whitespace around the title is trimmed alike",
                Some("# \u{3000}Wide\u{85}\n\u{85}Body\u{2028}"),
                Ok(("Wide", "Body")),
            ),
            (
                // U+FEFF is not whitespace to Rust; JavaScript's own `trim`
                // would strip it.
                "a mark inside the title line is text",
                Some("# \u{feff}Marked\u{feff}\nBody"),
                Ok(("\u{feff}Marked\u{feff}", "Body")),
            ),
            ("no file", None, Err(DraftError::Missing)),
            ("whitespace only", Some("  \n\t\n"), Err(DraftError::Empty)),
            (
                "plain text",
                Some("Plain text\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a level-2 heading",
                Some("## Section\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a deeper heading",
                Some("### Deeper\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a bare hash",
                Some("#\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a hash and spaces",
                Some("#   \nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a hash and unicode whitespace only",
                Some("# \u{3000}\u{85}\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "no space after the hash",
                Some("#Title\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a no-break space after the hash",
                Some("#\u{a0}Title\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a note reference",
                Some("#note:6c48d5ac\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a setext heading",
                Some("Title\n=====\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "front matter",
                Some("---\ntitle: T\n---\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a line above the heading",
                Some("Intro line\n# Title\n\nBody"),
                Err(DraftError::MissingTitle),
            ),
            (
                "a heading alone",
                Some("# Title"),
                Err(DraftError::TitleOnly),
            ),
            (
                "a heading over blank lines",
                Some("\n# Title\r\n\n  \n"),
                Err(DraftError::TitleOnly),
            ),
        ];
        for (what, text, expected) in cases {
            // `title` left out, and `title` null.
            for arguments in [serde_json::json!({}), serde_json::json!({"title": null})] {
                empty_session_dir(&outbox);
                if let Some(text) = text {
                    std::fs::write(&draft, text).unwrap();
                }
                let (laptop, script) =
                    write_note_on_both_transports(home.path(), &node, &arguments, what);
                // Result or error text, title and body: all three alike.
                assert_eq!(script, laptop, "{what} with {arguments}");
                match &expected {
                    Ok((title, body)) => {
                        assert_eq!(laptop.title.as_deref(), Some(*title), "{what}");
                        assert_eq!(laptop.body.as_deref(), Some(*body), "{what}");
                        assert_eq!(
                            laptop.text,
                            crate::note_tool::saved_message(title, body.len()),
                            "{what}"
                        );
                    }
                    Err(error) => {
                        assert_eq!(laptop.text, error.describe(&shown), "{what}");
                        assert_eq!((&laptop.title, &laptop.body), (&None, &None), "{what}");
                    }
                }
            }
        }

        // A draft exactly at the cap is published whole by both.
        empty_session_dir(&outbox);
        std::fs::write(&draft, vec![b'x'; max as usize]).unwrap();
        let slot = crate::note_tool::new_submission_slot();
        crate::note_tool::handle_write_note(
            "sess-1",
            &slot,
            &draft,
            WriteNoteParams {
                title: Some("Title".to_string()),
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
