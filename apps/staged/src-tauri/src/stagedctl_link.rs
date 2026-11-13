//! Putting `stagedctl` on the user's PATH from Settings.
//!
//! A release install puts nothing on PATH: the CLI ships inside the bundle,
//! at `Staged.app/Contents/MacOS/stagedctl`, and only agent sessions get a
//! shim for it. The Settings action behind
//! [`commands::link_stagedctl_on_path`] links that file into
//! `/usr/local/bin`, or into `~/.local/bin` when `/usr/local/bin` cannot be
//! written. It never asks for an administrator password.
//!
//! This file holds the decisions and touches no filesystem:
//!
//! - [`link_source`]: which file to link, or why this copy of Staged is not
//!   linked at all ([`Refusal`]).
//! - [`link_candidates`]: where the link may go, in order.
//! - [`decide`]: what to do given what is already at each of those paths.
//! - [`dir_on_path`]: whether the directory that got the link is on PATH.
//!
//! [`fs`] looks at the disk, runs the one step `decide` chose, and is handed
//! every path it touches, so its tests stay inside a temporary directory.
//!
//! What the action will and will not do:
//!
//! - **The link's target is this app's own sidecar.** It is read off the
//!   running executable's path. Nothing from the frontend, the arguments or
//!   the environment names it.
//! - **A link is made, never a copy.** `stagedctl` finds the app to start
//!   from its own location in the bundle, and a link stays in step with app
//!   updates.
//! - **Nothing is overwritten unseen.** A path that already holds something
//!   else is reported as [`LinkOutcome::Conflict`] and left alone. A link
//!   that points elsewhere is replaced only on a second call that repeats
//!   the path and target the user was shown, and only if that is still what
//!   is on disk. A file or directory is never replaced.
//! - **Desktop only.** `web_server::dispatch` refuses the command, and it
//!   stays out of `control_server::ALLOWED_COMMANDS`, so neither a browser
//!   client nor a stagedctl caller can change what the user's shell runs.

// The symlink work is unix-only; elsewhere the command answers that it is
// unsupported and the decisions below have no caller.
#![cfg_attr(not(unix), allow(dead_code))]

pub mod commands;
mod fs;

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The CLI's file name, in the bundle and on PATH.
const CLI_NAME: &str = "stagedctl";

/// The directory tried first. It is on the default macOS PATH
/// (`/etc/paths`), and it is where `install.sh` links the CLI.
const SYSTEM_BIN_DIR: &str = "/usr/local/bin";

/// The fallback, under the user's home, for when [`SYSTEM_BIN_DIR`] cannot
/// be written. A stock Mac leaves `/usr/local` owned by root.
const USER_BIN_DIR: &str = ".local/bin";

/// The path component macOS puts in every App Translocation mount:
/// `/private/var/folders/…/T/AppTranslocation/<uuid>/d/Staged.app`.
const TRANSLOCATION_COMPONENT: &str = "AppTranslocation";

/// Why this copy of Staged is not linked onto PATH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// macOS is running a quarantined app from a randomized, read-only mount
    /// (App Translocation). The mount is gone after a restart, so a link
    /// into it would dangle.
    Translocated,
    /// The executable is not inside a `.app` bundle: a development build
    /// under `target/`, whose sidecar goes when the build directory does.
    NotBundled,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::Translocated => {
                "macOS is running Staged from a temporary, read-only copy, so a link to it \
                 would stop working. Move Staged to your Applications folder, open it from \
                 there, and try again."
            }
            Refusal::NotBundled => {
                "This is a development build of Staged, which has no app bundle to link to. \
                 Open an installed Staged.app and try again."
            }
        })
    }
}

/// The file the link points at: the `stagedctl` beside `app_executable`,
/// which tauri bundles at `<bundle>.app/Contents/MacOS/stagedctl`.
///
/// Purely lexical. The translocation check comes first, because a
/// translocated app is inside a bundle too.
pub(crate) fn link_source(app_executable: &Path) -> Result<PathBuf, Refusal> {
    if app_executable
        .components()
        .any(|component| component.as_os_str() == TRANSLOCATION_COMPONENT)
    {
        return Err(Refusal::Translocated);
    }
    let macos = bundle_executable_dir(app_executable).ok_or(Refusal::NotBundled)?;
    Ok(macos.join(CLI_NAME))
}

/// `<bundle>.app/Contents/MacOS` when `executable` is an absolute path to a
/// file directly inside it. The same shape `stagedctl` reads its own bundle
/// from (`bundle_for_executable` in its `launch` module).
fn bundle_executable_dir(executable: &Path) -> Option<&Path> {
    if !executable.is_absolute() {
        return None;
    }
    executable.file_name()?;
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    (contents.parent()?.extension()? == "app").then_some(macos)
}

/// Where the link may go, in the order tried: `/usr/local/bin/stagedctl`,
/// then `<home>/.local/bin/stagedctl`. With no home directory there is no
/// fallback.
pub(crate) fn link_candidates(home: Option<&Path>) -> Vec<PathBuf> {
    let mut links = vec![Path::new(SYSTEM_BIN_DIR).join(CLI_NAME)];
    if let Some(home) = home {
        links.push(home.join(USER_BIN_DIR).join(CLI_NAME));
    }
    links
}

/// What is at a candidate link path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    /// Nothing.
    Absent,
    /// A symlink that reaches this app's sidecar.
    LinkToSource,
    /// A symlink that points somewhere else, with its target as written. It
    /// may dangle.
    LinkElsewhere { target: PathBuf },
    /// A file, a directory, or anything else that is not a symlink.
    NotALink,
}

/// One place the link may go, as [`fs`] found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub link: PathBuf,
    pub entry: Entry,
    /// False once creating the link in this directory has failed.
    pub usable: bool,
}

/// A link the user was shown and asked to replace. The frontend sends back
/// the `link` and `target` of the [`LinkOutcome::Conflict`] it displayed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownLink {
    pub link: String,
    pub target: String,
}

impl ShownLink {
    /// Whether this is the link at `link`, still pointing at `target`.
    fn shows(&self, link: &Path, target: &Path) -> bool {
        Path::new(&self.link) == link && Path::new(&self.target) == target
    }
}

/// The single step to take, or the reason none is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// A link to the sidecar is already there.
    AlreadyLinked { link: PathBuf },
    /// Nothing is at `link`: create it.
    Create { link: PathBuf },
    /// The user saw that `link` points at `shown_target` and asked for it to
    /// be replaced.
    Replace {
        link: PathBuf,
        shown_target: PathBuf,
    },
    /// Something else is at `link`, and the user has not seen it.
    Conflict { link: PathBuf, entry: Entry },
    /// No candidate directory took a link.
    NowhereUsable,
}

/// Decide what to do, given what is at each candidate path.
///
/// The candidates are read in order and the first one that is not an empty
/// spot in an unusable directory settles it:
///
/// - a link to the sidecar: nothing to do;
/// - a link elsewhere: replace it if `replace` names exactly this link and
///   target, otherwise report it;
/// - anything that is not a link: report it, whatever `replace` says;
/// - nothing there: create the link, or move on to the next candidate when
///   this directory has already refused one.
///
/// A conflict is never skipped in favour of the fallback. A `stagedctl`
/// earlier on PATH would shadow the new link, and the user should see it.
pub(crate) fn decide(candidates: &[Candidate], replace: Option<&ShownLink>) -> Step {
    for candidate in candidates {
        let link = candidate.link.clone();
        match &candidate.entry {
            Entry::LinkToSource => return Step::AlreadyLinked { link },
            Entry::LinkElsewhere { target }
                if replace.is_some_and(|shown| shown.shows(&link, target)) =>
            {
                return Step::Replace {
                    link,
                    shown_target: target.clone(),
                };
            }
            Entry::LinkElsewhere { .. } | Entry::NotALink => {
                return Step::Conflict {
                    link,
                    entry: candidate.entry.clone(),
                };
            }
            Entry::Absent if candidate.usable => return Step::Create { link },
            Entry::Absent => {}
        }
    }
    Step::NowhereUsable
}

/// Whether `dir` is an entry of the PATH value `path_var`. `None` when the
/// user's PATH is not known.
pub(crate) fn dir_on_path(dir: &Path, path_var: Option<&OsStr>) -> Option<bool> {
    let path_var = path_var?;
    Some(std::env::split_paths(path_var).any(|entry| entry == dir))
}

/// What [`fs::install`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Installed {
    /// The link at this path was created, or replaced at the user's request.
    Linked(PathBuf),
    /// The link at this path already reached the sidecar.
    AlreadyLinked(PathBuf),
    /// Something else is at `link`. Nothing was changed.
    Conflict { link: PathBuf, entry: Entry },
}

/// What is in the way of the link, as the user is shown it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExistingEntry {
    /// A link to `target`, which the user may ask to have replaced.
    Link { target: String },
    /// A file or directory. Staged never replaces one.
    Other,
}

/// The command's answer. `link` is the full path of the `stagedctl` on PATH.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LinkOutcome {
    /// The link was created, or replaced at the user's request. `on_path`
    /// says whether its directory is on the user's PATH; `null` when their
    /// PATH could not be read.
    Linked { link: String, on_path: Option<bool> },
    /// The link was already there.
    AlreadyLinked { link: String, on_path: Option<bool> },
    /// Something else is at `link`. Nothing was changed.
    Conflict {
        link: String,
        existing: ExistingEntry,
    },
}

impl LinkOutcome {
    /// `installed` as the frontend reads it. `path_var` is the user's PATH,
    /// when it is known.
    pub(crate) fn new(installed: Installed, path_var: Option<&OsStr>) -> Self {
        let on_path = |link: &Path| link.parent().and_then(|dir| dir_on_path(dir, path_var));
        let shown = |path: &Path| path.to_string_lossy().into_owned();
        match installed {
            Installed::Linked(link) => LinkOutcome::Linked {
                on_path: on_path(&link),
                link: shown(&link),
            },
            Installed::AlreadyLinked(link) => LinkOutcome::AlreadyLinked {
                on_path: on_path(&link),
                link: shown(&link),
            },
            Installed::Conflict { link, entry } => LinkOutcome::Conflict {
                link: shown(&link),
                existing: match entry {
                    Entry::LinkElsewhere { target } => ExistingEntry::Link {
                        target: shown(&target),
                    },
                    _ => ExistingEntry::Other,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const INSTALLED: &str = "/Applications/Staged.app/Contents/MacOS/Staged";

    // -------------------------------------------------------------------
    // The link target
    // -------------------------------------------------------------------

    #[test]
    fn the_link_points_at_the_sidecar_beside_the_app_executable() {
        assert_eq!(
            link_source(Path::new(INSTALLED)),
            Ok(PathBuf::from(
                "/Applications/Staged.app/Contents/MacOS/stagedctl"
            ))
        );
    }

    #[test]
    fn the_link_follows_the_bundle_wherever_it_is_installed() {
        for (executable, sidecar) in [
            (
                "/Users/me/Applications/Staged.app/Contents/MacOS/Staged",
                "/Users/me/Applications/Staged.app/Contents/MacOS/stagedctl",
            ),
            // A renamed bundle, and one with a space in its path.
            (
                "/Applications/Staged Beta.app/Contents/MacOS/Staged",
                "/Applications/Staged Beta.app/Contents/MacOS/stagedctl",
            ),
            (
                "/Volumes/Work Disk/Apps/Staged.app/Contents/MacOS/Staged",
                "/Volumes/Work Disk/Apps/Staged.app/Contents/MacOS/stagedctl",
            ),
        ] {
            assert_eq!(
                link_source(Path::new(executable)),
                Ok(PathBuf::from(sidecar)),
                "{executable}"
            );
        }
    }

    #[test]
    fn a_translocated_app_is_refused() {
        for executable in [
            "/private/var/folders/yc/abc123/T/AppTranslocation/\
             6F1C0C2E-93B4-4E55-9F0B-0A7D4B0D2A11/d/Staged.app/Contents/MacOS/Staged",
            "/var/folders/yc/abc123/T/AppTranslocation/\
             6F1C0C2E-93B4-4E55-9F0B-0A7D4B0D2A11/d/Staged.app/Contents/MacOS/Staged",
        ] {
            assert_eq!(
                link_source(Path::new(executable)),
                Err(Refusal::Translocated),
                "{executable}"
            );
        }
    }

    /// The component is matched whole: a directory that merely has the word
    /// in its name is an ordinary install location.
    #[test]
    fn a_path_that_only_mentions_translocation_is_not_refused() {
        assert_eq!(
            link_source(Path::new(
                "/Users/me/AppTranslocation-notes/Staged.app/Contents/MacOS/Staged"
            )),
            Ok(PathBuf::from(
                "/Users/me/AppTranslocation-notes/Staged.app/Contents/MacOS/stagedctl"
            ))
        );
    }

    /// Translocation is the reason given even where the path would also fail
    /// the bundle check, since moving the app is the remedy.
    #[test]
    fn translocation_is_checked_before_the_bundle_shape() {
        assert_eq!(
            link_source(Path::new(
                "/private/var/folders/yc/abc/T/AppTranslocation/ID/d/Staged"
            )),
            Err(Refusal::Translocated)
        );
    }

    #[test]
    fn an_executable_outside_a_bundle_is_refused() {
        for executable in [
            // Development builds, with and without a target triple.
            "/Users/me/.cache/cargo-target/debug/Staged",
            "/repo/apps/staged/src-tauri/target/release/Staged",
            // Bundle-like paths that are not `<x>.app/Contents/MacOS/<exe>`.
            "/Applications/Staged.app/Contents/Resources/Staged",
            "/Applications/Staged.app/MacOS/Staged",
            "/Applications/Staged/Contents/MacOS/Staged",
            "/Applications/Staged.app/Contents/MacOS/nested/Staged",
            // Not absolute, so it names no place a link could point at.
            "Staged.app/Contents/MacOS/Staged",
            "/",
        ] {
            assert_eq!(
                link_source(Path::new(executable)),
                Err(Refusal::NotBundled),
                "{executable}"
            );
        }
    }

    #[test]
    fn each_refusal_says_what_to_do_next() {
        let translocated = Refusal::Translocated.to_string();
        assert!(translocated.contains("Move Staged to your Applications folder"));
        let not_bundled = Refusal::NotBundled.to_string();
        assert!(not_bundled.contains("development build"));
        assert!(not_bundled.contains("installed Staged.app"));
    }

    // -------------------------------------------------------------------
    // Where the link goes
    // -------------------------------------------------------------------

    #[test]
    fn the_system_directory_comes_before_the_one_under_home() {
        assert_eq!(
            link_candidates(Some(Path::new("/Users/me"))),
            vec![
                PathBuf::from("/usr/local/bin/stagedctl"),
                PathBuf::from("/Users/me/.local/bin/stagedctl"),
            ]
        );
    }

    #[test]
    fn without_a_home_directory_there_is_no_fallback() {
        assert_eq!(
            link_candidates(None),
            vec![PathBuf::from("/usr/local/bin/stagedctl")]
        );
    }

    // -------------------------------------------------------------------
    // The decision
    // -------------------------------------------------------------------

    const SYSTEM: &str = "/usr/local/bin/stagedctl";
    const USER: &str = "/Users/me/.local/bin/stagedctl";
    const OTHER_BUILD: &str = "/Users/me/old/Staged.app/Contents/MacOS/stagedctl";

    fn candidate(link: &str, entry: Entry) -> Candidate {
        Candidate {
            link: PathBuf::from(link),
            entry,
            usable: true,
        }
    }

    fn unusable(link: &str) -> Candidate {
        Candidate {
            link: PathBuf::from(link),
            entry: Entry::Absent,
            usable: false,
        }
    }

    fn elsewhere() -> Entry {
        Entry::LinkElsewhere {
            target: PathBuf::from(OTHER_BUILD),
        }
    }

    fn shown(link: &str, target: &str) -> ShownLink {
        ShownLink {
            link: link.to_string(),
            target: target.to_string(),
        }
    }

    #[test]
    fn an_empty_spot_in_the_first_directory_gets_the_link() {
        let candidates = [
            candidate(SYSTEM, Entry::Absent),
            candidate(USER, Entry::Absent),
        ];
        assert_eq!(
            decide(&candidates, None),
            Step::Create {
                link: PathBuf::from(SYSTEM)
            }
        );
    }

    #[test]
    fn a_directory_that_refused_the_link_falls_back_to_the_next() {
        let candidates = [unusable(SYSTEM), candidate(USER, Entry::Absent)];
        assert_eq!(
            decide(&candidates, None),
            Step::Create {
                link: PathBuf::from(USER)
            }
        );
    }

    #[test]
    fn no_usable_directory_is_reported() {
        assert_eq!(
            decide(&[unusable(SYSTEM), unusable(USER)], None),
            Step::NowhereUsable
        );
        assert_eq!(decide(&[], None), Step::NowhereUsable);
    }

    #[test]
    fn an_existing_link_to_the_sidecar_needs_nothing() {
        let candidates = [
            candidate(SYSTEM, Entry::LinkToSource),
            candidate(USER, Entry::Absent),
        ];
        assert_eq!(
            decide(&candidates, None),
            Step::AlreadyLinked {
                link: PathBuf::from(SYSTEM)
            }
        );

        // Found in the fallback too, when the first directory has none and
        // cannot take one. It does not need to be writable to count.
        let mut fallback = candidate(USER, Entry::LinkToSource);
        fallback.usable = false;
        assert_eq!(
            decide(&[unusable(SYSTEM), fallback], None),
            Step::AlreadyLinked {
                link: PathBuf::from(USER)
            }
        );
    }

    #[test]
    fn a_link_elsewhere_is_reported_not_replaced() {
        let candidates = [
            candidate(SYSTEM, elsewhere()),
            candidate(USER, Entry::Absent),
        ];
        assert_eq!(
            decide(&candidates, None),
            Step::Conflict {
                link: PathBuf::from(SYSTEM),
                entry: elsewhere(),
            }
        );
    }

    /// The fallback is for a directory that cannot be written, never a way
    /// round something the user has not seen.
    #[test]
    fn a_conflict_is_not_skipped_for_the_fallback() {
        for entry in [elsewhere(), Entry::NotALink] {
            let mut first = candidate(SYSTEM, entry.clone());
            first.usable = false;
            assert_eq!(
                decide(&[first, candidate(USER, Entry::Absent)], None),
                Step::Conflict {
                    link: PathBuf::from(SYSTEM),
                    entry,
                }
            );
        }
    }

    #[test]
    fn a_link_the_user_was_shown_is_replaced() {
        let candidates = [
            candidate(SYSTEM, elsewhere()),
            candidate(USER, Entry::Absent),
        ];
        assert_eq!(
            decide(&candidates, Some(&shown(SYSTEM, OTHER_BUILD))),
            Step::Replace {
                link: PathBuf::from(SYSTEM),
                shown_target: PathBuf::from(OTHER_BUILD),
            }
        );
    }

    #[test]
    fn a_link_that_is_not_the_one_shown_is_reported_again() {
        let candidates = [
            candidate(SYSTEM, elsewhere()),
            candidate(USER, Entry::Absent),
        ];
        let conflict = Step::Conflict {
            link: PathBuf::from(SYSTEM),
            entry: elsewhere(),
        };
        // The link changed after the user saw it.
        assert_eq!(
            decide(&candidates, Some(&shown(SYSTEM, "/somewhere/else"))),
            conflict
        );
        // The user was shown the other candidate's link.
        assert_eq!(
            decide(&candidates, Some(&shown(USER, OTHER_BUILD))),
            conflict
        );
        // A path that is no candidate at all selects nothing.
        assert_eq!(
            decide(&candidates, Some(&shown("/etc/hosts", OTHER_BUILD))),
            conflict
        );
    }

    #[test]
    fn a_file_is_never_replaced() {
        let candidates = [
            candidate(SYSTEM, Entry::NotALink),
            candidate(USER, Entry::Absent),
        ];
        for replace in [None, Some(shown(SYSTEM, "")), Some(shown(SYSTEM, SYSTEM))] {
            assert_eq!(
                decide(&candidates, replace.as_ref()),
                Step::Conflict {
                    link: PathBuf::from(SYSTEM),
                    entry: Entry::NotALink,
                }
            );
        }
    }

    /// A replace request is only ever a confirmation. With nothing in the
    /// way it changes nothing about a create, and it never turns a link that
    /// is already right into a write.
    #[test]
    fn a_replace_request_with_nothing_to_replace_is_ignored() {
        let replace = shown(SYSTEM, OTHER_BUILD);
        assert_eq!(
            decide(&[candidate(SYSTEM, Entry::Absent)], Some(&replace)),
            Step::Create {
                link: PathBuf::from(SYSTEM)
            }
        );
        assert_eq!(
            decide(&[candidate(SYSTEM, Entry::LinkToSource)], Some(&replace)),
            Step::AlreadyLinked {
                link: PathBuf::from(SYSTEM)
            }
        );
    }

    // -------------------------------------------------------------------
    // PATH
    // -------------------------------------------------------------------

    #[test]
    fn a_directory_is_found_on_path_by_whole_entry() {
        let path = OsStr::new("/usr/bin:/bin:/usr/local/bin/:/Users/me/.local/bin");
        assert_eq!(
            dir_on_path(Path::new("/usr/local/bin"), Some(path)),
            Some(true)
        );
        assert_eq!(
            dir_on_path(Path::new("/Users/me/.local/bin"), Some(path)),
            Some(true)
        );
        assert_eq!(
            dir_on_path(Path::new("/usr/local"), Some(path)),
            Some(false)
        );
        assert_eq!(dir_on_path(Path::new("/opt/bin"), Some(path)), Some(false));
        assert_eq!(
            dir_on_path(Path::new("/usr/bin"), Some(OsStr::new(""))),
            Some(false)
        );
    }

    #[test]
    fn an_unknown_path_gives_no_answer() {
        assert_eq!(dir_on_path(Path::new("/usr/local/bin"), None), None);
    }

    // -------------------------------------------------------------------
    // The wire shape
    // -------------------------------------------------------------------

    #[test]
    fn outcomes_serialize_as_the_frontend_reads_them() {
        let path = OsStr::new("/usr/bin:/usr/local/bin");
        assert_eq!(
            serde_json::to_value(LinkOutcome::new(
                Installed::Linked(PathBuf::from(SYSTEM)),
                Some(path)
            ))
            .unwrap(),
            json!({ "outcome": "linked", "link": SYSTEM, "onPath": true })
        );
        assert_eq!(
            serde_json::to_value(LinkOutcome::new(
                Installed::AlreadyLinked(PathBuf::from(USER)),
                Some(path)
            ))
            .unwrap(),
            json!({ "outcome": "alreadyLinked", "link": USER, "onPath": false })
        );
        assert_eq!(
            serde_json::to_value(LinkOutcome::new(
                Installed::Linked(PathBuf::from(USER)),
                None
            ))
            .unwrap(),
            json!({ "outcome": "linked", "link": USER, "onPath": null })
        );
        assert_eq!(
            serde_json::to_value(LinkOutcome::new(
                Installed::Conflict {
                    link: PathBuf::from(SYSTEM),
                    entry: elsewhere(),
                },
                Some(path)
            ))
            .unwrap(),
            json!({
                "outcome": "conflict",
                "link": SYSTEM,
                "existing": { "kind": "link", "target": OTHER_BUILD },
            })
        );
        assert_eq!(
            serde_json::to_value(LinkOutcome::new(
                Installed::Conflict {
                    link: PathBuf::from(SYSTEM),
                    entry: Entry::NotALink,
                },
                Some(path)
            ))
            .unwrap(),
            json!({ "outcome": "conflict", "link": SYSTEM, "existing": { "kind": "other" } })
        );
    }

    /// The replace confirmation is the conflict's own `link` and `target`
    /// sent back, so it must read the same field names.
    #[test]
    fn a_shown_link_deserializes_from_the_conflict_fields() {
        let replace: ShownLink =
            serde_json::from_value(json!({ "link": SYSTEM, "target": OTHER_BUILD })).unwrap();
        assert_eq!(replace, shown(SYSTEM, OTHER_BUILD));
    }
}
