//! Writing a local clone's `info/exclude` alongside everything else that does.
//!
//! Every worktree of a clone reads that one file, and more than one thing
//! writes it: a transfer's finish appends the entries for the ignored files
//! it places (`ignored_lock` in `scripts/ignored.rs`), and Staged takes them
//! back out — on a rollback, as a worktree goes, at a start's sweep — which
//! may overlap another project's finish on the same clone. So each takes
//! [`ExcludeLock`] first and reads the file under it, and Staged's rewrite
//! replaces the file in one rename ([`replace`]), so `git status` in another
//! worktree never reads it half written, or empty.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// What the lock beside a file is named: `info/exclude.staged-lock`.
pub const LOCK_SUFFIX: &str = ".staged-lock";

/// How old a lock is when whoever made it is taken to have gone (crashed,
/// been killed) without removing it, so it is taken over. A finish adding
/// entries for a few thousand files only its source ignored holds it for
/// seconds.
pub const STALE_AFTER: Duration = Duration::from_secs(30);

/// How long taking the lock waits before giving up. Longer than
/// [`STALE_AFTER`], so a lock left behind is always taken over first.
pub const WAIT_AT_MOST: Duration = Duration::from_secs(40);

const POLL: Duration = Duration::from_millis(20);

/// `<pid>-<n>`, different for every call in this process.
fn unique() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// `<file>.staged-lock`, held until dropped. The lock is a file made
/// exclusively (`O_EXCL`; the script's `set -C`) holding who made it — a
/// token here, a pid there — and each side removes it only while it still
/// holds what that side wrote, so a lock taken over as stale isn't then
/// removed from under whoever took it.
#[derive(Debug)]
pub struct ExcludeLock {
    path: PathBuf,
    token: String,
}

impl ExcludeLock {
    /// Take the lock beside `file`, waiting for up to [`WAIT_AT_MOST`] and
    /// taking over one older than [`STALE_AFTER`]. Fails with `NotFound`
    /// when `file`'s directory isn't there, and `WouldBlock` when the wait
    /// runs out.
    pub fn acquire(file: &Path) -> io::Result<Self> {
        Self::acquire_within(file, WAIT_AT_MOST, STALE_AFTER)
    }

    fn acquire_within(file: &Path, wait: Duration, stale: Duration) -> io::Result<Self> {
        let path = lock_path(file);
        let token = unique();
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut made) => {
                    // From here on, dropping it removes it.
                    let lock = Self { path, token };
                    writeln!(made, "{}", lock.token)?;
                    return Ok(lock);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            if is_stale(&path, stale) {
                let _ = fs::remove_file(&path);
            }
            if started.elapsed() >= wait {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("{} is still held", path.display()),
                ));
            }
            std::thread::sleep(POLL);
        }
    }
}

impl Drop for ExcludeLock {
    fn drop(&mut self) {
        let ours = fs::read_to_string(&self.path).is_ok_and(|held| held.trim_end() == self.token);
        if ours {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// `<file>.staged-lock`.
pub fn lock_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(LOCK_SUFFIX);
    file.with_file_name(name)
}

fn is_stale(lock: &Path, stale: Duration) -> bool {
    fs::metadata(lock)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|made| SystemTime::now().duration_since(made).ok())
        .is_some_and(|age| age >= stale)
}

/// Replace what `file` holds with `content` in one rename, keeping the
/// file's permissions: through a sibling temporary file in the same
/// directory, so a reader sees the old content or the new, never part of
/// either. A symlink is written through, as a plain write would have.
pub fn replace(file: &Path, content: &[u8]) -> io::Result<()> {
    let target = if fs::symlink_metadata(file)?.file_type().is_symlink() {
        fs::canonicalize(file)?
    } else {
        file.to_path_buf()
    };
    let mut name = OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(format!(".staged-tmp-{}", unique()));
    let temporary = target.with_file_name(name);
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut out| {
            out.write_all(content)?;
            out.set_permissions(fs::metadata(&target)?.permissions())
        })
        .and_then(|()| fs::rename(&temporary, &target));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::super::scripts::EXCLUDE_LOCK_FUNCTIONS;
    use super::super::undo::remove_exclude_lines;
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    fn shells() -> Vec<&'static str> {
        ["/bin/sh", "/bin/dash"]
            .into_iter()
            .filter(|shell| Path::new(shell).exists())
            .collect()
    }

    /// `body` after the script's lock functions, under `shell`, with the
    /// file to lock as `$1` and `args` after it.
    fn run_locking(shell: &str, body: &str, file: &Path, args: &[&str]) -> Output {
        Command::new(shell)
            .arg("-c")
            .arg(format!("set -u\n{EXCLUDE_LOCK_FUNCTIONS}{body}"))
            .arg("sh")
            .arg(file)
            .args(args)
            .output()
            .unwrap()
    }

    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_lock_is_exclusive_and_goes_with_its_holder() {
        let root = crate::test_utils::TempDir::new("exclude-lock-exclusive");
        let file = root.path().join("exclude");
        let lock = ExcludeLock::acquire(&file).unwrap();
        assert!(lock_path(&file).is_file());
        // Held: a second one waits, then gives up.
        let second = ExcludeLock::acquire_within(&file, Duration::from_millis(100), STALE_AFTER);
        assert_eq!(
            second.unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "a fresh lock isn't taken over"
        );
        drop(lock);
        assert!(!lock_path(&file).exists());
        ExcludeLock::acquire(&file).unwrap();
        assert_eq!(names_in(root.path()), Vec::<String>::new());
        // No directory for the file: nothing to lock.
        let nowhere = root.path().join("gone/exclude");
        assert_eq!(
            ExcludeLock::acquire(&nowhere).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn a_stale_lock_is_taken_over_and_kept_from_its_maker() {
        let root = crate::test_utils::TempDir::new("exclude-lock-stale");
        let file = root.path().join("exclude");
        // Made by someone who never came back for it.
        let left = fs::File::create(lock_path(&file)).unwrap();
        left.set_modified(SystemTime::now() - Duration::from_secs(120))
            .unwrap();
        drop(left);
        let started = Instant::now();
        let taken = ExcludeLock::acquire(&file).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));

        // Taken over from a holder that was only slow: when that one lets
        // go, the lock stays with whoever has it now.
        let slow = ExcludeLock {
            path: lock_path(&file),
            token: "someone-slow".to_string(),
        };
        drop(slow);
        assert!(lock_path(&file).is_file());
        drop(taken);
        assert!(!lock_path(&file).exists());
    }

    #[test]
    fn replacing_keeps_the_mode_writes_through_a_symlink_and_leaves_nothing_behind() {
        let root = crate::test_utils::TempDir::new("exclude-replace");
        let file = root.path().join("exclude");
        fs::write(&file, "old\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        replace(&file, b"new\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "new\n");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(names_in(root.path()), vec!["exclude".to_string()]);

        let link = root.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        replace(&link, b"through\n").unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&file).unwrap(), "through\n");
        assert_eq!(
            names_in(root.path()),
            vec!["exclude".to_string(), "link".to_string()]
        );

        // Nothing to replace.
        assert!(replace(&root.path().join("missing"), b"x").is_err());
        assert_eq!(names_in(root.path()).len(), 2);
    }

    #[test]
    fn the_script_takes_the_same_lock_with_the_same_ages() {
        let expected = format!(
            "_lk=$1{LOCK_SUFFIX} _stale={} _wait={}",
            STALE_AFTER.as_secs(),
            WAIT_AT_MOST.as_secs()
        );
        assert!(
            EXCLUDE_LOCK_FUNCTIONS.contains(&expected),
            "{EXCLUDE_LOCK_FUNCTIONS}"
        );
    }

    /// The script's side: it waits for Staged's lock, takes over a stale
    /// one, lets go of its own as it exits however it does, and leaves one
    /// someone took over from it to them.
    #[test]
    fn the_scripts_lock_waits_takes_over_and_lets_go_as_staged_does() {
        let root = crate::test_utils::TempDir::new("exclude-lock-script");
        let file = root.path().join("exclude");
        let lock = lock_path(&file);
        for shell in shells() {
            let left = fs::File::create(&lock).unwrap();
            left.set_modified(SystemTime::now() - Duration::from_secs(120))
                .unwrap();
            drop(left);
            let started = Instant::now();
            let out = run_locking(
                shell,
                "ignored_lock \"$1\" || exit 9\n\
                 [ \"$(cat \"$1.staged-lock\")\" = \"$$\" ] || exit 8\n\
                 ignored_unlock\n\
                 [ ! -e \"$1.staged-lock\" ]\n",
                &file,
                &[],
            );
            assert!(out.status.success(), "{shell}: {out:?}");
            assert!(started.elapsed() < Duration::from_secs(10), "{shell}");

            // A `fail` exits with the lock held: the exit lets go of it.
            let out = run_locking(shell, "ignored_lock \"$1\" || exit 9\nexit 3\n", &file, &[]);
            assert_eq!(out.status.code(), Some(3), "{shell}: {out:?}");
            assert!(!lock.exists(), "{shell}");

            // Taken over meanwhile: it stays with whoever took it.
            let out = run_locking(
                shell,
                "ignored_lock \"$1\" || exit 9\n\
                 printf 'someone-else\\n' > \"$1.staged-lock\"\n\
                 ignored_unlock\n",
                &file,
                &[],
            );
            assert!(out.status.success(), "{shell}: {out:?}");
            assert_eq!(fs::read_to_string(&lock).unwrap(), "someone-else\n");
            fs::remove_file(&lock).unwrap();

            // Held by Staged: waited for until it lets go.
            let held = ExcludeLock::acquire(&file).unwrap();
            let started = Instant::now();
            let out = std::thread::scope(|s| {
                s.spawn(|| {
                    std::thread::sleep(Duration::from_millis(300));
                    drop(held);
                });
                run_locking(
                    shell,
                    "ignored_lock \"$1\" || exit 9\nignored_unlock\n",
                    &file,
                    &[],
                )
            });
            assert!(out.status.success(), "{shell}: {out:?}");
            assert!(started.elapsed() >= Duration::from_millis(250), "{shell}");
            assert!(!lock.exists(), "{shell}");
        }
    }

    /// Appends under the script's lock, from every shell at once, racing
    /// Staged taking other lines out under its own: no line appended is
    /// lost, every line taken out goes, and the file never reads empty, or
    /// without the lines before them all.
    #[test]
    fn appends_and_removals_under_the_lock_lose_nothing_and_never_read_empty() {
        const EACH: usize = 30;
        const REMOVERS: usize = 3;
        let root = crate::test_utils::TempDir::new("exclude-lock-race");
        let info = root.path().join("info");
        let file = info.join("exclude");
        fs::create_dir_all(&info).unwrap();
        let base = "# git ls-files --others --exclude-from=.git/info/exclude\n.penpal\n";
        let removed: Vec<Vec<String>> = (0..REMOVERS)
            .map(|t| (0..EACH).map(|i| format!("/rm-{t}-{i}")).collect())
            .collect();
        let mut initial = base.to_string();
        for line in removed.iter().flatten() {
            initial.push_str(line);
            initial.push('\n');
        }
        fs::write(&file, &initial).unwrap();
        let shells = shells();
        let appender = format!(
            "i=0\n\
             while [ \"$i\" -lt {EACH} ]; do\n\
               ignored_lock \"$1\" || exit 9\n\
               printf '/add-%s-%s\\n' \"$2\" \"$i\" >> \"$1\" || exit 8\n\
               ignored_unlock\n\
               i=$((i + 1))\n\
             done\n"
        );
        let done = AtomicBool::new(false);
        let reads = AtomicUsize::new(0);

        let outcomes: Vec<Result<(), String>> = std::thread::scope(|s| {
            let reader = s.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    let text = fs::read_to_string(&file).unwrap_or_default();
                    if !text.starts_with(base) {
                        return Err(format!("read mid-write: {text:?}"));
                    }
                    reads.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            });
            let appenders = shells.iter().enumerate().map(|(k, shell)| {
                let (appender, file) = (&appender, &file);
                s.spawn(move || {
                    let out = run_locking(shell, appender, file, &[&k.to_string()]);
                    out.status
                        .success()
                        .then_some(())
                        .ok_or_else(|| format!("{shell}: {out:?}"))
                })
            });
            let removers = removed.iter().map(|lines| {
                let file = &file;
                s.spawn(move || {
                    for line in lines {
                        remove_exclude_lines(file, std::slice::from_ref(line), false, false)?;
                    }
                    Ok(())
                })
            });
            let writers: Vec<_> = appenders.chain(removers).collect();
            let mut outcomes: Vec<_> = writers
                .into_iter()
                .map(|w| w.join().unwrap_or_else(|_| Err("panicked".into())))
                .collect();
            done.store(true, Ordering::Relaxed);
            outcomes.push(reader.join().unwrap_or_else(|_| Err("panicked".into())));
            outcomes
        });

        for outcome in outcomes {
            outcome.unwrap();
        }
        assert!(reads.load(Ordering::Relaxed) > 0);
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.starts_with(base), "{text}");
        assert!(!text.contains("/rm-"), "{text}");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2 + shells.len() * EACH, "{text}");
        for k in 0..shells.len() {
            for i in 0..EACH {
                let line = format!("/add-{k}-{i}");
                assert_eq!(
                    lines.iter().filter(|l| **l == line).count(),
                    1,
                    "{line} in {text}"
                );
            }
        }
        assert_eq!(names_in(&info), vec!["exclude".to_string()]);
    }
}
