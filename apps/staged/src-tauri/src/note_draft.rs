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
//!   seeded with the stored body ([`seed_body`]: verbatim, so a heading
//!   the body opens with and its `staged-media://` references survive)
//!   before the agent starts, and the prompt says to read it and edit only
//!   what changes.
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

/// What stands in for the workstation home in a draft path whose home could
/// not be resolved.
const UNRESOLVED_HOME: &str = "$HOME";

/// The draft path a note prompt names. It is the path the `write_note`
/// handler reads: [`local_draft_path`] locally, [`remote_draft_path`] on a
/// workstation. A workstation whose home could not be resolved gets the
/// `$HOME/…` spelling. That text is a placeholder, not a path: a file tool
/// that does not expand variables would create a `$HOME` directory in the
/// worktree, so the prompt that carries it must say so
/// ([`names_unresolved_home`]). The Staged MCP on Blox names the absolute
/// path in its `initialize` instructions and in every draft error.
pub(crate) fn prompt_draft_path(session_id: &str, location: DraftLocation<'_>) -> String {
    match location {
        DraftLocation::Local => local_draft_path(session_id).to_string_lossy().into_owned(),
        DraftLocation::Remote { home: Some(home) } => remote_draft_path(home, session_id),
        DraftLocation::Remote { home: None } => remote_draft_path(UNRESOLVED_HOME, session_id),
    }
}

/// Whether `draft_path` is the [`prompt_draft_path`] spelling for a
/// workstation whose home could not be resolved. A resolved path is
/// absolute, so it never starts with the placeholder.
pub(crate) fn names_unresolved_home(draft_path: &str) -> bool {
    draft_path
        .strip_prefix(UNRESOLVED_HOME)
        .is_some_and(|rest| rest.starts_with('/'))
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
    /// The first line is not a `# ` heading holding the title
    /// ([`split_title`]).
    MissingTitle,
    /// Nothing below the title line ([`split_title`]).
    TitleOnly,
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
            Self::MissingTitle => format!(
                "the draft file `{path}` does not start with the note's title. Its first \
                 non-blank line must be a level-1 markdown heading: `# ` followed by the title \
                 on one line (not `##`, not a line underlined with `===`, and nothing above \
                 it). Add that line at the top and leave the rest of the note below it"
            ),
            Self::TitleOnly => format!(
                "the draft file `{path}` holds only the title line. Write the note body in \
                 markdown below the `# ` title line"
            ),
            Self::Io(error) => format!("the draft file `{path}` could not be read: {error}"),
        };
        format!("write_note rejected: {what}, then call write_note again.")
    }
}

/// Open the draft for reading in a way that cannot block. `O_NONBLOCK` makes
/// opening a FIFO that has no writer return at once instead of waiting for
/// one (the handle is then rejected as not a regular file); it changes
/// nothing for a regular file.
fn open_draft(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(path)
}

/// Read and validate the draft: a regular file, at most [`DRAFT_MAX_BYTES`],
/// UTF-8, non-empty once a leading byte order mark and the surrounding
/// whitespace are removed. Returns that body.
///
/// The file is opened once and everything is asked of the handle, so the
/// file that is checked is the file that is read: a path swapped for a FIFO
/// or another file after the check changes nothing, and the read stops one
/// byte past the cap however much the file grows meanwhile. The open follows
/// symbolic links, so a draft that is a link to a regular file publishes
/// that file's contents; the agent already has the access to read it.
///
/// `staged-mcp.cjs` (`readDraft`) applies the same steps in the same order;
/// `write_note_reads_the_same_drafts_on_both_transports` in
/// `remote_staged_mcp::note_outbox` runs both over the same files.
pub(crate) fn read_draft(path: &Path) -> Result<String, DraftError> {
    use std::io::Read;

    let file = match open_draft(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(DraftError::Missing),
        // Where a directory cannot be opened for reading at all (Windows).
        Err(_) if path.is_dir() => return Err(DraftError::NotRegular),
        Err(e) => return Err(DraftError::Io(e.to_string())),
    };
    let metadata = file.metadata().map_err(|e| DraftError::Io(e.to_string()))?;
    if !metadata.is_file() {
        return Err(DraftError::NotRegular);
    }
    if metadata.len() > DRAFT_MAX_BYTES {
        return Err(DraftError::Oversized(metadata.len()));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(DRAFT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| DraftError::Io(e.to_string()))?;
    let read = bytes.len() as u64;
    if read > DRAFT_MAX_BYTES {
        // The file grew past the cap after the size check.
        let size = file.metadata().map_or(read, |m| m.len().max(read));
        return Err(DraftError::Oversized(size));
    }
    let text = String::from_utf8(bytes).map_err(|_| DraftError::NotUtf8)?;
    // One leading U+FEFF is an encoding mark some editors write, not note
    // text; `trim` does not count it as whitespace.
    let body = text.strip_prefix('\u{feff}').unwrap_or(&text).trim();
    if body.is_empty() {
        return Err(DraftError::Empty);
    }
    Ok(body.to_string())
}

/// Split a draft that opens with its title into `(title, body)`. `draft` is
/// what [`read_draft`] returns: the byte order mark and the surrounding
/// whitespace are already gone, so its first line is the file's first
/// non-blank line.
///
/// 1. The first line is the text up to the first `\n`.
/// 2. It must be one `#` followed by a space or a tab. The title is the rest
///    of the line, trimmed, and must not be empty
///    ([`DraftError::MissingTitle`] otherwise).
/// 3. The body is everything after that line, trimmed, and must not be empty
///    ([`DraftError::TitleOnly`] otherwise).
///
/// Only that one line is consumed. A `##` heading is a section and never a
/// title, the title text is kept as written (inline markdown and a closing
/// `#` included), and a later `# Heading` is body text. Putting
/// `# {title}`, a blank line and a body together and splitting the result
/// therefore gives the same title and body back, whatever the body opens
/// with.
///
/// The local `write_note` handler calls this for a call that passes no
/// `title` (`NoteSubmission::from_draft`). A call that passes one still
/// publishes the whole draft as the body.
pub(crate) fn split_title(draft: &str) -> Result<(String, String), DraftError> {
    let (first_line, rest) = draft.split_once('\n').unwrap_or((draft, ""));
    let title = first_line
        .strip_prefix('#')
        .filter(|after_marker| after_marker.starts_with([' ', '\t']))
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or(DraftError::MissingTitle)?;
    let body = rest.trim();
    if body.is_empty() {
        return Err(DraftError::TitleOnly);
    }
    Ok((title.to_string(), body.to_string()))
}

/// The body to seed an amendment's draft with: the stored note body exactly
/// as stored, `staged-media://` references and any heading it opens with
/// included. `None` when the stored note is blank — a fresh note gets no
/// file.
///
/// Verbatim is what keeps the seed and the publish in agreement. The store
/// holds the body without the title, and `write_note` treats an H1 at the
/// top of the draft as body text (the title is a call argument), so a body
/// that opens with `# Section` must come back from a seed, an untouched
/// republish and the store with that heading still in place.
pub(crate) fn seed_body(stored_content: &str) -> Option<String> {
    (!stored_content.trim().is_empty()).then(|| stored_content.to_string())
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
        let unresolved = prompt_draft_path("abc-123", DraftLocation::Remote { home: None });
        assert_eq!(unresolved, "$HOME/.staged/sessions/abc-123/draft.md");

        // Only that spelling is flagged as a placeholder for the prompt.
        assert!(names_unresolved_home(&unresolved));
        assert!(!names_unresolved_home(
            "/home/bloxer/.staged/sessions/abc-123/draft.md"
        ));
        assert!(!names_unresolved_home(
            &local_draft_path("abc-123").to_string_lossy()
        ));
        assert!(!names_unresolved_home("$HOMEWORK/draft.md"));
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
    fn read_draft_drops_one_leading_byte_order_mark_before_trimming() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft.md");

        // Nothing but the mark, with or without whitespace: an empty draft,
        // not a three-byte invisible note.
        std::fs::write(&path, "\u{feff}").unwrap();
        assert_eq!(read_draft(&path), Err(DraftError::Empty));
        std::fs::write(&path, "\u{feff} \n\t\n").unwrap();
        assert_eq!(read_draft(&path), Err(DraftError::Empty));

        // The mark goes, then the whitespace around the body.
        std::fs::write(&path, "\u{feff}\n# Section\nBody.\n").unwrap();
        assert_eq!(read_draft(&path).unwrap(), "# Section\nBody.");

        // Only one, and only at the very start: anything else is text.
        std::fs::write(&path, "\u{feff}\u{feff}Body\u{feff}").unwrap();
        assert_eq!(read_draft(&path).unwrap(), "\u{feff}Body\u{feff}");
        std::fs::write(&path, " \u{feff}Body").unwrap();
        assert_eq!(read_draft(&path).unwrap(), "\u{feff}Body");
    }

    #[cfg(unix)]
    #[test]
    fn read_draft_follows_a_symlink_and_never_waits_on_a_fifo() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft.md");

        // A link to a regular file publishes that file; a dangling link is
        // a missing draft.
        let target = dir.path().join("elsewhere.md");
        std::fs::write(&target, "Linked body\n").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert_eq!(read_draft(&path).unwrap(), "Linked body");
        std::fs::remove_file(&target).unwrap();
        assert_eq!(read_draft(&path), Err(DraftError::Missing));
        std::fs::remove_file(&path).unwrap();

        // A FIFO with no writer: a blocking open would hang the handler
        // here for good. The test finishing is the assertion.
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert_eq!(read_draft(&path), Err(DraftError::NotRegular));
        std::fs::remove_file(&path).unwrap();

        // A file the handler may not read is an I/O error naming the cause.
        // Root reads it regardless, so the check only runs where the mode
        // is enforced.
        std::fs::write(&path, "Body").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_err() {
            match read_draft(&path) {
                Err(DraftError::Io(error)) => {
                    let text = DraftError::Io(error).describe("/tmp/draft.md");
                    assert!(
                        text.starts_with(
                            "write_note rejected: the draft file `/tmp/draft.md` could not be \
                             read: "
                        ),
                        "{text}"
                    );
                }
                other => panic!("{other:?}"),
            }
        }
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
    fn title_error_texts_say_what_the_first_line_must_be() {
        assert_eq!(
            DraftError::MissingTitle.describe("/tmp/draft.md"),
            "write_note rejected: the draft file `/tmp/draft.md` does not start with the \
             note's title. Its first non-blank line must be a level-1 markdown heading: `# ` \
             followed by the title on one line (not `##`, not a line underlined with `===`, \
             and nothing above it). Add that line at the top and leave the rest of the note \
             below it, then call write_note again."
        );
        assert_eq!(
            DraftError::TitleOnly.describe("/tmp/draft.md"),
            "write_note rejected: the draft file `/tmp/draft.md` holds only the title line. \
             Write the note body in markdown below the `# ` title line, then call write_note \
             again."
        );
    }

    fn split_ok(draft: &str) -> (String, String) {
        split_title(draft).unwrap_or_else(|error| panic!("{draft:?}: {error:?}"))
    }

    fn parts(title: &str, body: &str) -> (String, String) {
        (title.to_string(), body.to_string())
    }

    #[test]
    fn split_title_takes_the_first_line_as_the_title_and_the_rest_as_the_body() {
        assert_eq!(
            split_ok("# Title\n\nBody.\n\n## Section\nMore."),
            parts("Title", "Body.\n\n## Section\nMore.")
        );
        // The body may follow on the very next line, and a tab separates the
        // marker from the title as well as a space does.
        assert_eq!(split_ok("# Title\nBody."), parts("Title", "Body."));
        assert_eq!(split_ok("#\tTitle\nBody."), parts("Title", "Body."));
        // The title is trimmed; the blank lines under it are not body.
        assert_eq!(
            split_ok("#   Spaced out \t\n\n\n\nBody."),
            parts("Spaced out", "Body.")
        );
    }

    #[test]
    fn split_title_accepts_blank_lines_or_a_byte_order_mark_above_the_heading() {
        // `read_draft` removes both before the split sees the text.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("draft.md");
        for draft in [
            "\n\n  \n# Title\n\nBody.\n",
            "\u{feff}# Title\n\nBody.\n",
            "\u{feff}\n\n# Title\n\nBody.\n",
        ] {
            std::fs::write(&path, draft).unwrap();
            assert_eq!(
                split_ok(&read_draft(&path).unwrap()),
                parts("Title", "Body."),
                "{draft:?}"
            );
        }
    }

    #[test]
    fn split_title_rejects_a_first_line_that_is_not_a_level_one_heading() {
        for draft in [
            // A deeper heading is a section, never the title.
            "## Title\n\nBody.",
            "### Title\n\nBody.",
            "###### Title\n\nBody.",
            // A marker with no title text.
            "#\n\nBody.",
            "#",
            "#   \n\nBody.",
            "# \t \n\nBody.",
            "#   ",
            // No space after the marker: not a heading, and a hashtag
            // reference must not become a title.
            "#Title\n\nBody.",
            "#note:6c48d5ac-cecc-4cfe-a9ac-f7f7313fa06e\n\nBody.",
            // A setext heading.
            "Title\n===\n\nBody.",
            "Title\n=====\n\nBody.",
            // Front matter.
            "---\ntitle: Title\n---\n\nBody.",
            // Plain text, with the heading only further down.
            "Body first.\n\n# Title\n\nMore.",
            "Body only.",
            // An escaped marker is text.
            "\\# Title\n\nBody.",
            "",
        ] {
            assert_eq!(
                split_title(draft),
                Err(DraftError::MissingTitle),
                "{draft:?}"
            );
        }
    }

    #[test]
    fn split_title_keeps_the_title_text_as_written() {
        // Inline markdown is title text: titles are stored as markdown source.
        assert_eq!(
            split_ok("# Use `foo` for [bar](x)\n\nBody."),
            parts("Use `foo` for [bar](x)", "Body.")
        );
        // A closing sequence is not stripped.
        assert_eq!(split_ok("# Title #\n\nBody."), parts("Title #", "Body."));
        assert_eq!(split_ok("# Title ##\n\nBody."), parts("Title ##", "Body."));
        // Only the one marker is consumed.
        assert_eq!(split_ok("# # Title\n\nBody."), parts("# Title", "Body."));
    }

    #[test]
    fn split_title_rejects_a_draft_that_is_only_the_title_line() {
        for draft in ["# Title", "# Title\n", "# Title\n\n  \n\t\n", "# Title\r\n"] {
            assert_eq!(split_title(draft), Err(DraftError::TitleOnly), "{draft:?}");
        }
    }

    #[test]
    fn split_title_leaves_a_later_level_one_heading_in_the_body() {
        assert_eq!(
            split_ok("# Title\n\nIntro.\n\n# Second heading\n\nMore.\n\n# Third\nEnd."),
            parts(
                "Title",
                "Intro.\n\n# Second heading\n\nMore.\n\n# Third\nEnd."
            )
        );
        // Directly under the title too: the body opens with its own H1.
        assert_eq!(
            split_ok("# Title\n# Section\nBody."),
            parts("Title", "# Section\nBody.")
        );
    }

    #[test]
    fn split_title_reads_crlf_line_endings() {
        // The carriage return that ends the title line is trimmed off the
        // title; the body keeps its own line endings.
        assert_eq!(
            split_ok("# Title\r\n\r\nFirst line.\r\nSecond line."),
            parts("Title", "First line.\r\nSecond line.")
        );
        assert_eq!(split_ok("# Title\r\nBody."), parts("Title", "Body."));
        // The same rejections as with bare line feeds.
        assert_eq!(split_title("#\r\n\r\nBody."), Err(DraftError::MissingTitle));
        assert_eq!(
            split_title("## Title\r\n\r\nBody."),
            Err(DraftError::MissingTitle)
        );
        assert_eq!(
            split_title("Title\r\n===\r\n\r\nBody."),
            Err(DraftError::MissingTitle)
        );
        assert_eq!(split_title("# Title\r\n\r\n"), Err(DraftError::TitleOnly));
    }

    #[test]
    fn a_title_heading_placed_above_a_body_splits_back_into_the_same_title_and_body() {
        // The shape a draft seeded with its title will have: the heading, a
        // blank line, the stored body. Written to the draft file, read back
        // as an untouched publish would read it and split, it gives the
        // stored title and body again. Only the first line is consumed, so
        // a body that opens with its own `# Section` keeps it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("staged-note-draft-s1.md");
        for (title, body) in [
            (
                "Plan the title move",
                "## Summary\n\nFirst paragraph.\n\n## Details\nMore.",
            ),
            (
                "Plan the title move",
                "# Section\n\nFirst paragraph.\n\n## Details\nMore.",
            ),
            (
                "Plan the title move",
                "Plain opening line.\n\n# Late heading\nMore.",
            ),
            (
                "Use `foo` for [bar](x) (2)",
                "Body ![Shot](staged-media://abc.png)",
            ),
            (
                "Title #",
                "#note:6c48d5ac-cecc-4cfe-a9ac-f7f7313fa06e is the plan.",
            ),
        ] {
            let draft = format!("# {title}\n\n{body}");
            prepare_local_draft(&path, Some(&draft)).unwrap();
            assert_eq!(
                split_ok(&read_draft(&path).unwrap()),
                parts(title, body),
                "{draft:?}"
            );
        }
    }

    #[test]
    fn seed_body_is_the_stored_body_verbatim() {
        assert_eq!(seed_body(""), None);
        assert_eq!(seed_body("  \n"), None);
        // A heading the body opens with is body text, exactly as it is when
        // `write_note` publishes the draft.
        assert_eq!(
            seed_body("# Section\n\nBody ![Shot](staged-media://abc.png)\n").as_deref(),
            Some("# Section\n\nBody ![Shot](staged-media://abc.png)\n")
        );
        assert_eq!(
            seed_body("No heading\n## Section").as_deref(),
            Some("No heading\n## Section")
        );
    }

    #[test]
    fn a_seeded_draft_republishes_the_stored_body_heading_included() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("staged-note-draft-s1.md");
        let stored = "# Section\n\nFirst paragraph.\n\n## Details\nMore.";

        prepare_local_draft(&path, seed_body(stored).as_deref()).unwrap();
        assert_eq!(read_draft(&path).unwrap(), stored);
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
