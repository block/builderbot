//! The filesystem half of linking `stagedctl` onto PATH: look at what is at
//! each candidate path, run the one step [`decide`] chose, and look again
//! when the disk changed underneath.
//!
//! Every path comes from the caller. Nothing here knows `/usr/local/bin` or
//! the user's home, so the tests below run the whole flow inside a temporary
//! directory.

use std::path::{Path, PathBuf};

use super::{Installed, ShownLink};

/// Link `source` at the first of `links` that can take it.
///
/// `replace` is the link the user was shown and asked to have replaced, if
/// any. An `Err` is a sentence for the user: nothing was linked, and it says
/// why.
#[cfg(unix)]
pub(crate) fn install(
    source: &Path,
    links: &[PathBuf],
    replace: Option<&ShownLink>,
) -> Result<Installed, String> {
    unix::install(source, links, replace)
}

#[cfg(not(unix))]
pub(crate) fn install(
    _source: &Path,
    _links: &[PathBuf],
    _replace: Option<&ShownLink>,
) -> Result<Installed, String> {
    Err("Linking stagedctl onto your PATH is only supported on macOS.".to_string())
}

#[cfg(unix)]
mod unix {
    use std::io::{self, ErrorKind};
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    use super::super::{decide, Candidate, Entry, Installed, ShownLink, Step, CLI_NAME};

    pub(super) fn install(
        source: &Path,
        links: &[PathBuf],
        replace: Option<&ShownLink>,
    ) -> Result<Installed, String> {
        if !source.is_file() {
            return Err(format!(
                "This copy of Staged has no stagedctl inside it (looked for {}). Reinstall \
                 Staged and try again.",
                source.display()
            ));
        }

        // The directories that would not take a link, each with the reason.
        let mut refused: Vec<(PathBuf, String)> = Vec::new();
        // Each pass ends the call, rules one directory out, or looks again
        // because the disk changed between the look and the step. The last
        // can only repeat if something else keeps rewriting the same path,
        // so the passes are bounded rather than trusted to stop.
        for _ in 0..links.len() * 2 + 2 {
            let candidates = links
                .iter()
                .map(|link| {
                    Ok(Candidate {
                        entry: observe(link, source)?,
                        usable: !refused.iter().any(|(refused, _)| refused == link),
                        link: link.clone(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;

            match decide(&candidates, replace) {
                Step::AlreadyLinked { link } => return Ok(Installed::AlreadyLinked(link)),
                Step::Conflict { link, entry } => return Ok(Installed::Conflict { link, entry }),
                Step::Create { link } => match create_link(source, &link) {
                    Ok(()) => return Ok(Installed::Linked(link)),
                    Err(CreateError::Appeared) => {}
                    Err(CreateError::Unusable(why)) => refused.push((link, why)),
                },
                Step::Replace { link, shown_target } => {
                    match replace_link(source, &link, &shown_target) {
                        Ok(true) => return Ok(Installed::Linked(link)),
                        // No longer the link the user saw: look again, and
                        // report what is there now.
                        Ok(false) => {}
                        Err(e) => {
                            return Err(format!(
                                "Staged could not replace {}: {e}. The link is unchanged.",
                                link.display()
                            ));
                        }
                    }
                }
                Step::NowhereUsable => return Err(nowhere_usable(&refused)),
            }
        }
        Err(
            "The stagedctl link kept changing while Staged was setting it up. Nothing more was \
             changed; try again."
                .to_string(),
        )
    }

    /// What is at `link`, without following it.
    fn observe(link: &Path, source: &Path) -> Result<Entry, String> {
        let metadata = match std::fs::symlink_metadata(link) {
            Ok(metadata) => metadata,
            // `NotADirectory`: the directory itself is missing and a file
            // sits where one of its parents should be. Creating the link
            // fails there, which is what moves on to the fallback.
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok(Entry::Absent);
            }
            Err(e) => return Err(could_not_read(link, &e)),
        };
        if !metadata.file_type().is_symlink() {
            return Ok(Entry::NotALink);
        }
        let target = std::fs::read_link(link).map_err(|e| could_not_read(link, &e))?;
        if target == source || same_file(link, source) {
            Ok(Entry::LinkToSource)
        } else {
            Ok(Entry::LinkElsewhere { target })
        }
    }

    /// Whether `link` resolves to `source`, however its target is spelled: a
    /// relative link, or one through a symlinked directory. A link that
    /// dangles resolves to nothing.
    fn same_file(link: &Path, source: &Path) -> bool {
        match (std::fs::canonicalize(link), std::fs::canonicalize(source)) {
            (Ok(link), Ok(source)) => link == source,
            _ => false,
        }
    }

    fn could_not_read(link: &Path, error: &io::Error) -> String {
        format!("Staged could not read {}: {error}.", link.display())
    }

    enum CreateError {
        /// Something took the path after it was seen empty.
        Appeared,
        /// The directory cannot be made or written, with the OS's reason.
        Unusable(String),
    }

    /// Create the link, and its directory if needed. `symlink` fails when
    /// anything is at the path, a dangling link included, so this never
    /// overwrites.
    fn create_link(source: &Path, link: &Path) -> Result<(), CreateError> {
        if let Some(dir) = link.parent() {
            std::fs::create_dir_all(dir).map_err(|e| CreateError::Unusable(e.to_string()))?;
        }
        match symlink(source, link) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(CreateError::Appeared),
            Err(e) => Err(CreateError::Unusable(e.to_string())),
        }
    }

    /// Point `link` at `source`, provided it is still a link to
    /// `shown_target`. `Ok(false)` means it is not, and nothing was changed.
    ///
    /// The new link is made beside the old one and renamed over it, so the
    /// name on PATH is never missing and a failure leaves the old link in
    /// place. The check and the rename are two calls, the closest the
    /// filesystem allows to a compare-and-swap.
    fn replace_link(source: &Path, link: &Path, shown_target: &Path) -> io::Result<bool> {
        match std::fs::read_link(link) {
            Ok(target) if target == shown_target => {}
            _ => return Ok(false),
        }
        let staging = staging_path(link);
        clear_stale_staging(&staging)?;
        symlink(source, &staging)?;
        if let Err(e) = std::fs::rename(&staging, link) {
            let _ = std::fs::remove_file(&staging);
            return Err(e);
        }
        Ok(true)
    }

    /// Where a replacement for `link` is made before it is renamed over it:
    /// a dot-name beside it, carrying this process's id.
    pub(super) fn staging_path(link: &Path) -> PathBuf {
        link.with_file_name(format!(".{CLI_NAME}.link-{}", std::process::id()))
    }

    /// Clear what an earlier replace left at `staging`, so this one can make
    /// its own. A replace that was cut short after making the staging link,
    /// or whose clean-up failed, leaves it behind, and `symlink` then fails
    /// with "File exists" on every later replace by a process with this id.
    ///
    /// Only a symlink is removed. That is the one thing this code ever puts
    /// at the name, and removing a link loses nothing but the path it held.
    /// A file or a directory there is not ours: it is left where it is and
    /// the replace is refused, naming it. Staging names that carry another
    /// process's id are not looked at.
    ///
    /// The look and the removal are two calls, like the check and the rename
    /// in [`replace_link`]. `remove_file` never removes a directory.
    fn clear_stale_staging(staging: &Path) -> io::Result<()> {
        let Ok(metadata) = std::fs::symlink_metadata(staging) else {
            // Nothing there. Or the directory cannot be read, and `symlink`
            // says why next.
            return Ok(());
        };
        if !metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                format!(
                    "{} is in the way and is not a link Staged made, so it was left alone",
                    staging.display()
                ),
            ));
        }
        match std::fs::remove_file(staging) {
            Ok(()) => Ok(()),
            // Gone between the look and the removal: nothing left to clear.
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn nowhere_usable(refused: &[(PathBuf, String)]) -> String {
        let mut message =
            "Staged could not link stagedctl into a directory on your PATH.".to_string();
        for (link, why) in refused {
            let dir = link.parent().unwrap_or(link);
            message.push_str(&format!(" {}: {why}.", dir.display()));
        }
        message
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::Entry;
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    /// A bundle with a sidecar, and two candidate directories, all under one
    /// temporary directory. Neither candidate directory exists yet.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        source: PathBuf,
        system: PathBuf,
        user: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            // Canonical, so `/var` against `/private/var` never decides a test.
            let root = dir.path().canonicalize().unwrap();
            let macos = root.join("Applications/Staged.app/Contents/MacOS");
            std::fs::create_dir_all(&macos).unwrap();
            let source = macos.join("stagedctl");
            std::fs::write(&source, "#!/bin/sh\n").unwrap();
            Self {
                system: root.join("usr/local/bin/stagedctl"),
                user: root.join("home/.local/bin/stagedctl"),
                source,
                root,
                _dir: dir,
            }
        }

        fn links(&self) -> Vec<PathBuf> {
            vec![self.system.clone(), self.user.clone()]
        }

        fn install(&self, replace: Option<&ShownLink>) -> Result<Installed, String> {
            install(&self.source, &self.links(), replace)
        }

        /// Another build's sidecar, for links that point elsewhere.
        fn other_build(&self) -> PathBuf {
            let macos = self.root.join("old/Staged.app/Contents/MacOS");
            std::fs::create_dir_all(&macos).unwrap();
            let other = macos.join("stagedctl");
            std::fs::write(&other, "old").unwrap();
            other
        }

        fn place_link(&self, link: &Path, target: &Path) {
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(target, link).unwrap();
        }
    }

    fn shown(link: &Path, target: &Path) -> ShownLink {
        ShownLink {
            link: link.to_string_lossy().into_owned(),
            target: target.to_string_lossy().into_owned(),
        }
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn set_mode(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Whether the directory really refuses writes. It does not for root,
    /// which ignores the mode bits, so those tests have nothing to check.
    fn read_only(dir: &Path) -> bool {
        let probe = dir.join(".probe");
        match std::fs::write(&probe, "") {
            Ok(()) => {
                std::fs::remove_file(&probe).unwrap();
                false
            }
            Err(_) => true,
        }
    }

    #[test]
    fn the_link_is_created_in_the_first_directory() {
        let fx = Fixture::new();

        assert_eq!(fx.install(None), Ok(Installed::Linked(fx.system.clone())));

        assert_eq!(std::fs::read_link(&fx.system).unwrap(), fx.source);
        assert_eq!(std::fs::read_to_string(&fx.system).unwrap(), "#!/bin/sh\n");
        // The fallback was not needed, so nothing was made for it.
        assert!(!fx.root.join("home").exists());
    }

    #[test]
    fn a_second_run_finds_the_link_and_changes_nothing() {
        let fx = Fixture::new();
        fx.install(None).unwrap();

        assert_eq!(
            fx.install(None),
            Ok(Installed::AlreadyLinked(fx.system.clone()))
        );
        assert_eq!(names_in(fx.system.parent().unwrap()), ["stagedctl"]);
        assert!(!fx.root.join("home").exists());
    }

    /// A link that reaches the sidecar by another spelling is the same link:
    /// here, through a directory that is itself a symlink to the bundle's.
    #[test]
    fn a_link_that_resolves_to_the_sidecar_counts_however_it_is_written() {
        let fx = Fixture::new();
        let alias = fx.root.join("alias");
        symlink(fx.source.parent().unwrap(), &alias).unwrap();
        fx.place_link(&fx.system, &alias.join("stagedctl"));

        assert_eq!(
            fx.install(None),
            Ok(Installed::AlreadyLinked(fx.system.clone()))
        );
        assert_eq!(
            std::fs::read_link(&fx.system).unwrap(),
            alias.join("stagedctl")
        );
    }

    #[test]
    fn a_directory_that_cannot_be_made_falls_back_to_the_next() {
        let fx = Fixture::new();
        // A file where the first directory's parent should be.
        std::fs::create_dir_all(fx.root.join("usr")).unwrap();
        std::fs::write(fx.root.join("usr/local"), "").unwrap();

        assert_eq!(fx.install(None), Ok(Installed::Linked(fx.user.clone())));
        assert_eq!(std::fs::read_link(&fx.user).unwrap(), fx.source);
        assert_eq!(
            std::fs::read_to_string(fx.root.join("usr/local")).unwrap(),
            ""
        );
    }

    #[test]
    fn a_directory_that_cannot_be_written_falls_back_to_the_next() {
        let fx = Fixture::new();
        let system_dir = fx.system.parent().unwrap();
        std::fs::create_dir_all(system_dir).unwrap();
        set_mode(system_dir, 0o555);
        if !read_only(system_dir) {
            return;
        }

        let installed = fx.install(None);
        set_mode(system_dir, 0o755);

        assert_eq!(installed, Ok(Installed::Linked(fx.user.clone())));
        assert!(names_in(system_dir).is_empty());
        assert_eq!(std::fs::read_link(&fx.user).unwrap(), fx.source);
    }

    #[test]
    fn no_usable_directory_names_each_one_and_why() {
        let fx = Fixture::new();
        for blocked in ["usr", "home"] {
            std::fs::write(fx.root.join(blocked), "").unwrap();
        }

        let error = fx.install(None).unwrap_err();

        assert!(error.starts_with("Staged could not link stagedctl into a directory"));
        for link in fx.links() {
            let dir = link.parent().unwrap().display().to_string();
            assert!(error.contains(&dir), "{error} should name {dir}");
        }
        assert_eq!(names_in(&fx.root), ["Applications", "home", "usr"]);
    }

    #[test]
    fn a_link_elsewhere_is_reported_and_left_alone() {
        let fx = Fixture::new();
        let other = fx.other_build();
        fx.place_link(&fx.system, &other);

        assert_eq!(
            fx.install(None),
            Ok(Installed::Conflict {
                link: fx.system.clone(),
                entry: Entry::LinkElsewhere {
                    target: other.clone()
                },
            })
        );
        assert_eq!(std::fs::read_link(&fx.system).unwrap(), other);
        // The fallback is not used as a way round it.
        assert!(!fx.root.join("home").exists());
    }

    #[test]
    fn a_dangling_link_is_reported_with_its_target() {
        let fx = Fixture::new();
        let gone = fx.root.join("deleted/Staged.app/Contents/MacOS/stagedctl");
        fx.place_link(&fx.system, &gone);

        assert_eq!(
            fx.install(None),
            Ok(Installed::Conflict {
                link: fx.system.clone(),
                entry: Entry::LinkElsewhere {
                    target: gone.clone()
                },
            })
        );
        assert_eq!(std::fs::read_link(&fx.system).unwrap(), gone);
    }

    #[test]
    fn a_link_the_user_was_shown_is_replaced() {
        let fx = Fixture::new();
        let other = fx.other_build();
        fx.place_link(&fx.system, &other);

        assert_eq!(
            fx.install(Some(&shown(&fx.system, &other))),
            Ok(Installed::Linked(fx.system.clone()))
        );

        assert_eq!(std::fs::read_link(&fx.system).unwrap(), fx.source);
        // The old target is untouched, and no staging link is left behind.
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "old");
        assert_eq!(names_in(fx.system.parent().unwrap()), ["stagedctl"]);
    }

    #[test]
    fn a_dangling_link_the_user_was_shown_is_replaced() {
        let fx = Fixture::new();
        let gone = fx.root.join("deleted/stagedctl");
        fx.place_link(&fx.system, &gone);

        assert_eq!(
            fx.install(Some(&shown(&fx.system, &gone))),
            Ok(Installed::Linked(fx.system.clone()))
        );
        assert_eq!(std::fs::read_link(&fx.system).unwrap(), fx.source);
    }

    /// The link changed between the user seeing it and confirming: what is
    /// there now is reported, and it is not replaced.
    #[test]
    fn a_link_that_changed_since_it_was_shown_is_not_replaced() {
        let fx = Fixture::new();
        let other = fx.other_build();
        let newer = fx.root.join("newer/stagedctl");
        fx.place_link(&fx.system, &newer);

        assert_eq!(
            fx.install(Some(&shown(&fx.system, &other))),
            Ok(Installed::Conflict {
                link: fx.system.clone(),
                entry: Entry::LinkElsewhere {
                    target: newer.clone()
                },
            })
        );
        assert_eq!(std::fs::read_link(&fx.system).unwrap(), newer);
    }

    #[test]
    fn a_file_is_reported_and_never_replaced() {
        let fx = Fixture::new();
        std::fs::create_dir_all(fx.system.parent().unwrap()).unwrap();
        std::fs::write(&fx.system, "someone's copy").unwrap();
        let conflict = Ok(Installed::Conflict {
            link: fx.system.clone(),
            entry: Entry::NotALink,
        });

        assert_eq!(fx.install(None), conflict);
        assert_eq!(fx.install(Some(&shown(&fx.system, &fx.system))), conflict);
        assert_eq!(
            fx.install(Some(&shown(&fx.system, Path::new("")))),
            conflict
        );

        assert_eq!(
            std::fs::read_to_string(&fx.system).unwrap(),
            "someone's copy"
        );
        assert!(!fx.root.join("home").exists());
    }

    #[test]
    fn a_directory_at_the_link_path_is_reported() {
        let fx = Fixture::new();
        std::fs::create_dir_all(&fx.system).unwrap();

        assert_eq!(
            fx.install(None),
            Ok(Installed::Conflict {
                link: fx.system.clone(),
                entry: Entry::NotALink,
            })
        );
        assert!(fx.system.is_dir());
    }

    /// A replace that the directory refuses is an error, not a reason to
    /// link somewhere else: the old link would still be the one in the way.
    #[test]
    fn a_replace_that_fails_leaves_the_old_link_and_says_so() {
        let fx = Fixture::new();
        let other = fx.other_build();
        fx.place_link(&fx.system, &other);
        let system_dir = fx.system.parent().unwrap();
        set_mode(system_dir, 0o555);
        if !read_only(system_dir) {
            return;
        }

        let replaced = fx.install(Some(&shown(&fx.system, &other)));
        set_mode(system_dir, 0o755);

        let error = replaced.unwrap_err();
        assert!(error.starts_with(&format!("Staged could not replace {}", fx.system.display())));
        assert!(error.ends_with("The link is unchanged."));
        assert_eq!(std::fs::read_link(&fx.system).unwrap(), other);
        assert_eq!(names_in(system_dir), ["stagedctl"]);
        assert!(!fx.root.join("home").exists());
    }

    /// Where a replace of `link` stages its new link, in this process.
    fn staging_for(link: &Path) -> PathBuf {
        super::unix::staging_path(link)
    }

    /// A replace that was cut short after making its staging link, or whose
    /// clean-up failed, leaves that link behind. The next replace clears it
    /// and goes ahead instead of failing with "File exists".
    #[test]
    fn a_staging_link_left_by_an_earlier_replace_is_cleared() {
        let fx = Fixture::new();
        let other = fx.other_build();
        fx.place_link(&fx.system, &other);
        let system_dir = fx.system.parent().unwrap();
        let staging = staging_for(&fx.system);
        assert_eq!(staging.parent(), Some(system_dir));
        // Left pointing at the other build, and then dangling: whatever it
        // points at, it is a link at the staging name.
        for leftover_target in [other.clone(), fx.root.join("deleted/stagedctl")] {
            symlink(&leftover_target, &staging).unwrap();

            assert_eq!(
                fx.install(Some(&shown(&fx.system, &other))),
                Ok(Installed::Linked(fx.system.clone()))
            );

            assert_eq!(std::fs::read_link(&fx.system).unwrap(), fx.source);
            assert_eq!(names_in(system_dir), ["stagedctl"]);
            // Only the leftover link went, not what it pointed at.
            assert_eq!(std::fs::read_to_string(&other).unwrap(), "old");

            std::fs::remove_file(&fx.system).unwrap();
            fx.place_link(&fx.system, &other);
        }
    }

    /// Only a symlink at the staging name is this code's to clear. A file or
    /// a directory there is left exactly as it was, and the replace is
    /// refused with its path.
    #[test]
    fn a_file_or_directory_at_the_staging_name_is_never_removed() {
        let fx = Fixture::new();
        let other = fx.other_build();
        fx.place_link(&fx.system, &other);
        let system_dir = fx.system.parent().unwrap();
        let staging = staging_for(&fx.system);
        let staging_name = staging.file_name().unwrap().to_string_lossy().into_owned();
        let refused = |fx: &Fixture| {
            let error = fx.install(Some(&shown(&fx.system, &other))).unwrap_err();
            assert!(
                error.starts_with(&format!("Staged could not replace {}", fx.system.display())),
                "{error}"
            );
            assert!(error.contains(&staging.display().to_string()), "{error}");
            assert!(error.ends_with("The link is unchanged."), "{error}");
            assert_eq!(std::fs::read_link(&fx.system).unwrap(), other);
            assert_eq!(names_in(system_dir), [staging_name.as_str(), "stagedctl"]);
        };

        std::fs::write(&staging, "someone's file").unwrap();
        refused(&fx);
        assert_eq!(std::fs::read_to_string(&staging).unwrap(), "someone's file");

        std::fs::remove_file(&staging).unwrap();
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("kept"), "someone's directory").unwrap();
        refused(&fx);
        assert_eq!(
            std::fs::read_to_string(staging.join("kept")).unwrap(),
            "someone's directory"
        );
    }

    /// The staging name carries the process id, so another process's
    /// leftover has another name. It is not in the way, and it is not this
    /// process's to remove.
    #[test]
    fn a_staging_link_under_another_name_is_left_alone() {
        let fx = Fixture::new();
        let other = fx.other_build();
        fx.place_link(&fx.system, &other);
        let system_dir = fx.system.parent().unwrap();
        let theirs = system_dir.join(".stagedctl.link-0");
        assert_ne!(theirs, staging_for(&fx.system));
        symlink(&other, &theirs).unwrap();

        assert_eq!(
            fx.install(Some(&shown(&fx.system, &other))),
            Ok(Installed::Linked(fx.system.clone()))
        );

        assert_eq!(std::fs::read_link(&fx.system).unwrap(), fx.source);
        assert_eq!(std::fs::read_link(&theirs).unwrap(), other);
        assert_eq!(names_in(system_dir), [".stagedctl.link-0", "stagedctl"]);
    }

    /// With nothing to replace, the staging name is never looked at: a
    /// leftover there is not cleared by a plain create.
    #[test]
    fn a_create_does_not_touch_the_staging_name() {
        let fx = Fixture::new();
        let system_dir = fx.system.parent().unwrap();
        std::fs::create_dir_all(system_dir).unwrap();
        let staging = staging_for(&fx.system);
        symlink(fx.root.join("deleted/stagedctl"), &staging).unwrap();

        assert_eq!(fx.install(None), Ok(Installed::Linked(fx.system.clone())));

        assert!(staging.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn a_bundle_with_no_sidecar_links_nothing() {
        let fx = Fixture::new();
        std::fs::remove_file(&fx.source).unwrap();

        let error = fx.install(None).unwrap_err();

        assert!(error.contains("has no stagedctl inside it"));
        assert!(error.contains(&fx.source.display().to_string()));
        assert!(!fx.root.join("usr").exists());
        assert!(!fx.root.join("home").exists());
    }

    /// An existing link to the sidecar in the fallback is found when the
    /// first directory has nothing and will not take a link.
    #[test]
    fn a_link_already_in_the_fallback_is_found() {
        let fx = Fixture::new();
        std::fs::write(fx.root.join("usr"), "").unwrap();
        fx.place_link(&fx.user, &fx.source);

        assert_eq!(
            fx.install(None),
            Ok(Installed::AlreadyLinked(fx.user.clone()))
        );
    }
}
