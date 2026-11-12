//! The per-session note draft file: the file that *is* the note.
//!
//! A note session gets exactly one draft file, named in its prompt. The
//! agent writes the note body into it with its own file tools, edits it in
//! place when amending or repairing, and `write_note(title,
//! suggested_next_steps)` publishes the file's current contents as the note
//! body (see `note_tool`). Carrying the body in the filesystem rather than
//! in the call's arguments is what makes every refinement an edit of the
//! same note instead of another copy of it.
//!
//! Locally the draft lives in the temp directory
//! ([`local_draft_path`]); on a Blox workstation it is `draft.md` in the
//! session's scratch directory under the Staged MCP on Blox
//! ([`remote_draft_path`]), read by the script's own `write_note`.
//!
//! Lifecycle, both transports:
//!
//! - **Fresh note** (stored note empty): the file is *not* pre-created — an
//!   empty file would trip Claude Code's read-before-write gate — and any
//!   stale draft from an earlier run is removed at run start.
//! - **Amendment or Pikchr repair** (stored note non-empty): the file is
//!   seeded with the stored body ([`seed_body`]: H1 stripped,
//!   `staged-media://` references verbatim) before the agent starts, and the
//!   prompt says to read it and edit only what changes.
//! - **Run end**: the local draft is removed ([`remove_local_draft`]); the
//!   remote one is cleared with the outbox and kept in the states that keep
//!   the outbox (`remote_staged_mcp::note_outbox`).
//!
//! No call, no note: a draft that changed without a `write_note` call is
//! never stored.

use std::path::{Path, PathBuf};

/// Largest draft `write_note` publishes, on either transport. The inline
/// body had no cap; p99 of stored notes is 52 kB, the largest 145 kB.
pub(crate) const DRAFT_MAX_BYTES: u64 = 1024 * 1024;

/// The draft's file name in the Blox session scratch directory.
pub(crate) const REMOTE_DRAFT_FILE_NAME: &str = "draft.md";

/// Where a local or project note session's draft lives:
/// `<temp dir>/staged-note-draft-<session-id>.md`. The temp directory is the
/// one place every local provider writes without a prompt or an extra
/// sandbox root, and the root the branch-history note copies already use.
pub(crate) fn local_draft_path(session_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("staged-note-draft-{session_id}.md"))
}

/// Where a remote note session's draft lives, spelled out with the
/// workstation's real home directory (Goose does not expand `~`):
/// `<home>/.staged/sessions/<session-id>/draft.md`.
pub(crate) fn remote_draft_path(home: &str, session_id: &str) -> String {
    format!(
        "{}/.staged/sessions/{session_id}/{REMOTE_DRAFT_FILE_NAME}",
        home.trim_end_matches('/')
    )
}

/// Shell fragment that prints the workstation's home directory with no
/// trailing newline. A remote note prompt needs the real home to spell the
/// draft path out, and gets it by appending this to a `ws exec` it already
/// runs (the Pikchr grammar upload) rather than paying a round trip for it.
pub(crate) const REMOTE_HOME_EPILOGUE: &str = "printf '%s' \"$HOME\"";

/// The home directory out of the output of a command that ended with
/// [`REMOTE_HOME_EPILOGUE`]: its last line, when that is an absolute path.
pub(crate) fn parse_remote_home(stdout: &str) -> Option<String> {
    let home = stdout.lines().last()?.trim();
    (home.starts_with('/') && home.len() > 1).then(|| home.trim_end_matches('/').to_string())
}

/// Which machine a note session's draft lives on, for [`prompt_draft_path`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum DraftLocation<'a> {
    /// A local or project session: the temp directory.
    Local,
    /// A session on a Blox workstation whose home directory is `home` when
    /// the prompt builder resolved it.
    Remote { home: Option<&'a str> },
}

/// The draft path a note prompt names. It is the path the `write_note`
/// handler reads: [`local_draft_path`] locally, [`remote_draft_path`] on a
/// workstation. A workstation whose home could not be resolved gets the
/// `$HOME/…` spelling — right for any agent that goes through a shell, and
/// the Staged MCP on Blox names the absolute path in its `initialize`
/// instructions and in every draft error.
pub(crate) fn prompt_draft_path(session_id: &str, location: DraftLocation<'_>) -> String {
    match location {
        DraftLocation::Local => local_draft_path(session_id).to_string_lossy().into_owned(),
        DraftLocation::Remote { home: Some(home) } => remote_draft_path(home, session_id),
        DraftLocation::Remote { home: None } => remote_draft_path("$HOME", session_id),
    }
}

/// Why a draft could not be published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftError {
    /// No file at the path.
    Missing,
    /// Something other than a regular file (a directory, a FIFO).
    NotRegular,
    /// Larger than [`DRAFT_MAX_BYTES`].
    Oversized(u64),
    /// Not valid UTF-8.
    NotUtf8,
    /// Nothing but whitespace.
    Empty,
    /// Another I/O failure.
    Io(String),
}

impl DraftError {
    /// The `write_note` error text for this failure: it names the absolute
    /// path so the agent fixes the file within the turn.
    pub(crate) fn describe(&self, path: &str) -> String {
        let what = match self {
            Self::Missing => format!(
                "the draft file `{path}` does not exist. This file is your note: write the full \
                 note body (markdown, without the title) into it"
            ),
            Self::NotRegular => format!(
                "`{path}` is not a regular file. The draft must be a plain file holding the \
                 note body"
            ),
            Self::Oversized(size) => format!(
                "the draft file `{path}` is {size} bytes, over the {DRAFT_MAX_BYTES} byte \
                 limit. Shorten it"
            ),
            Self::NotUtf8 => {
                format!("the draft file `{path}` is not valid UTF-8. Rewrite it as UTF-8 text")
            }
            Self::Empty => format!(
                "the draft file `{path}` is empty. Write the full note body (markdown, \
                 without the title) into it"
            ),
            Self::Io(error) => format!("the draft file `{path}` could not be read: {error}"),
        };
        format!("write_note rejected: {what}, then call write_note again.")
    }
}

/// Read and validate the draft: a regular file, at most [`DRAFT_MAX_BYTES`],
/// UTF-8, non-empty after trimming. Returns the trimmed body.
pub(crate) fn read_draft(path: &Path) -> Result<String, DraftError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(DraftError::Missing),
        Err(e) => return Err(DraftError::Io(e.to_string())),
    };
    if !metadata.is_file() {
        return Err(DraftError::NotRegular);
    }
    if metadata.len() > DRAFT_MAX_BYTES {
        return Err(DraftError::Oversized(metadata.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| DraftError::Io(e.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|_| DraftError::NotUtf8)?;
    let body = text.trim();
    if body.is_empty() {
        return Err(DraftError::Empty);
    }
    Ok(body.to_string())
}

/// The body to seed an amendment's draft with: the stored note content with
/// a leading H1 removed (the store keeps the body without the title, but an
/// older note may carry one), `staged-media://` references untouched. `None`
/// when the stored note is empty — a fresh note gets no file.
pub(crate) fn seed_body(stored_content: &str) -> Option<String> {
    let (_, body) = crate::session_runner::extract_note_title(stored_content);
    let body = body.trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// Put the local draft in its run-start state: seeded with `seed` for an
/// amendment, absent for a fresh note (a stale draft from an earlier run is
/// removed). The seed is written through a temp file and rename so the agent
/// never sees a half-written draft.
pub(crate) fn prepare_local_draft(path: &Path, seed: Option<&str>) -> Result<(), String> {
    match seed {
        Some(body) => {
            let tmp = path.with_extension(format!("seed.{}", std::process::id()));
            std::fs::write(&tmp, body).map_err(|e| {
                format!("failed to write the note draft seed {}: {e}", tmp.display())
            })?;
            std::fs::rename(&tmp, path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("failed to place the note draft {}: {e}", path.display())
            })
        }
        None => remove_local_draft(path),
    }
}

/// Remove the local draft, treating a missing file as already removed.
pub(crate) fn remove_local_draft(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "failed to remove the note draft {}: {e}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_per_session_and_spell_out_the_remote_home() {
        let local = local_draft_path("abc-123");
        assert!(local.starts_with(std::env::temp_dir()));
        assert!(local.ends_with("staged-note-draft-abc-123.md"));
        assert_eq!(
            remote_draft_path("/home/bloxer/", "abc-123"),
            "/home/bloxer/.staged/sessions/abc-123/draft.md"
        );
    }

    #[test]
    fn prompt_paths_match_the_handlers_and_fall_back_to_the_shell_home() {
        assert_eq!(
            prompt_draft_path("abc-123", DraftLocation::Local),
            local_draft_path("abc-123").to_string_lossy()
        );
        assert_eq!(
            prompt_draft_path(
                "abc-123",
                DraftLocation::Remote {
                    home: Some("/home/bloxer")
                }
            ),
            "/home/bloxer/.staged/sessions/abc-123/draft.md"
        );
        assert_eq!(
            prompt_draft_path("abc-123", DraftLocation::Remote { home: None }),
            "$HOME/.staged/sessions/abc-123/draft.md"
        );
    }

    #[test]
    fn remote_home_is_the_last_line_when_it_is_an_absolute_path() {
        assert_eq!(
            parse_remote_home("/home/bloxer").as_deref(),
            Some("/home/bloxer")
        );
        assert_eq!(
            parse_remote_home("noise from a login shell\n/home/bloxer/\n").as_deref(),
            Some("/home/bloxer")
        );
        assert_eq!(parse_remote_home(""), None);
        assert_eq!(parse_remote_home("/"), None);
        assert_eq!(parse_remote_home("not a path"), None);
    }

    #[test]
    fn the_home_epilogue_prints_home_without_a_newline() {
        let output = std::process::Command::new("sh")
            .args(["-c", REMOTE_HOME_EPILOGUE])
            .env("HOME", "/home/bloxer")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "/home/bloxer");
    }

    #[test]
    fn read_draft_trims_and_rejects_each_bad_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft.md");

        assert_eq!(read_draft(&path), Err(DraftError::Missing));
        assert_eq!(read_draft(dir.path()), Err(DraftError::NotRegular));

        std::fs::write(&path, "  \n\n").unwrap();
        assert_eq!(read_draft(&path), Err(DraftError::Empty));

        std::fs::write(&path, [0xff, 0xfe, b'x']).unwrap();
        assert_eq!(read_draft(&path), Err(DraftError::NotUtf8));

        let file = std::fs::File::create(&path).unwrap();
        file.set_len(DRAFT_MAX_BYTES + 1).unwrap();
        drop(file);
        assert_eq!(
            read_draft(&path),
            Err(DraftError::Oversized(DRAFT_MAX_BYTES + 1))
        );

        std::fs::write(&path, "\n# Not a title\nBody.\n\n").unwrap();
        assert_eq!(read_draft(&path).unwrap(), "# Not a title\nBody.");
    }

    #[test]
    fn error_text_names_the_path_and_asks_for_another_call() {
        let text = DraftError::Missing.describe("/tmp/draft.md");
        assert!(text.starts_with("write_note rejected: "), "{text}");
        assert!(text.contains("`/tmp/draft.md`"), "{text}");
        assert!(text.ends_with("call write_note again."), "{text}");
        assert!(DraftError::Oversized(2_000_000)
            .describe("/x")
            .contains("2000000 bytes"),);
    }

    #[test]
    fn seed_body_strips_the_h1_and_keeps_media_references() {
        assert_eq!(seed_body(""), None);
        assert_eq!(seed_body("  \n"), None);
        assert_eq!(
            seed_body("# Title\n\nBody ![Shot](staged-media://abc.png)\n").as_deref(),
            Some("Body ![Shot](staged-media://abc.png)")
        );
        assert_eq!(
            seed_body("No heading\n## Section").as_deref(),
            Some("No heading\n## Section")
        );
    }

    #[test]
    fn prepare_seeds_or_removes_and_remove_tolerates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("staged-note-draft-s1.md");

        std::fs::write(&path, "stale").unwrap();
        prepare_local_draft(&path, None).unwrap();
        assert!(!path.exists(), "a fresh note gets no file");

        prepare_local_draft(&path, Some("Seeded body")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Seeded body");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temp file left beside the draft"
        );

        remove_local_draft(&path).unwrap();
        assert!(!path.exists());
        remove_local_draft(&path).unwrap();
    }
}
