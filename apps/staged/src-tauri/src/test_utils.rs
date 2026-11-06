use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

use crate::git::strip_git_env;

/// A temporary git repository for use in tests.
///
/// Creates a fresh git repo in a temp directory and cleans it up on drop.
pub struct TempGitRepo {
    path: PathBuf,
}

impl Default for TempGitRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl TempGitRepo {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("staged-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();

        let repo = Self { path };
        repo.run_git(&["init", "--initial-branch=main"]);
        repo.run_git(&["config", "user.email", "test@example.com"]);
        repo.run_git(&["config", "user.name", "Test"]);
        repo
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_file(&self, name: &str, content: &str) {
        fs::write(self.path.join(name), content).unwrap();
    }

    pub fn commit(&self, message: &str) -> String {
        self.run_git(&["add", "."]);
        self.run_git(&["commit", "-m", message]);
        self.run_git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    pub fn run_git(&self, args: &[&str]) -> String {
        self.try_run_git(args)
            .unwrap_or_else(|stderr| panic!("git {args:?} failed: {stderr}"))
    }

    /// Run git and report the exit status instead of asserting on it, for
    /// commands whose failure is the point — a `git rebase` that stops on a
    /// conflict, say. `Err` carries stderr.
    pub fn try_run_git(&self, args: &[&str]) -> Result<String, String> {
        let mut command = Command::new("git");
        command
            .arg("-c")
            .arg("core.hooksPath=/dev/null")
            .arg("-C")
            .arg(&self.path)
            .args(args);
        strip_git_env(&mut command);

        let output = command.output().unwrap();

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }

        Ok(String::from_utf8(output.stdout).unwrap())
    }
}

impl Drop for TempGitRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A temporary directory removed on drop, for tests that need more than one
/// repository (a bare origin plus clones) or a worktree root.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("staged-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Run git in `repo` the way [`TempGitRepo::run_git`] does, for repositories
/// the test created itself (clones, bare origins, worktrees).
pub fn run_git_in(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-C")
        .arg(repo)
        .args(args);
    strip_git_env(&mut command);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A repo whose committed `data.bin` goes through a `required` clean filter
/// that can't run, rewritten since, so `git status` has to run the filter and
/// fails. That is a checkout with LFS files, seen by a git whose PATH lacks
/// git-lfs: an app launched from Finder before the login shell's env is in.
pub fn repo_with_unrunnable_clean_filter() -> TempGitRepo {
    let repo = TempGitRepo::new();
    repo.write_file(".gitattributes", "*.bin filter=unrunnable\n");
    repo.write_file("data.bin", "v1\n");
    repo.commit("init");
    repo.run_git(&[
        "config",
        "filter.unrunnable.clean",
        "staged-test-no-such-filter",
    ]);
    repo.run_git(&["config", "filter.unrunnable.required", "true"]);
    repo.write_file("data.bin", "v2\n");
    repo
}

/// Write `content` to a 0755 tempfile suitable for use as `$SHELL`.
///
/// Hold the returned handle for the test's lifetime — dropping it removes
/// the file. Returns a `TempPath` (not `NamedTempFile`) so the writable fd
/// is closed before the script gets exec'd: Linux's exec path refuses to
/// load a binary whose inode has `i_writecount > 0` and returns ETXTBSY.
/// macOS has no equivalent check, which is why the prior `NamedTempFile`
/// shape passed locally on Darwin but failed on Linux CI. Unix-only
/// because we set the executable mode.
#[cfg(unix)]
pub fn write_fake_shell(content: &str) -> tempfile::TempPath {
    use std::os::unix::fs::PermissionsExt;
    let temp_path = tempfile::Builder::new()
        .prefix("staged-fake-shell-")
        .suffix(".sh")
        .tempfile()
        .expect("create fake shell tempfile")
        .into_temp_path();
    fs::write(&temp_path, content).expect("write script");
    let mut perms = fs::metadata(&temp_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&temp_path, perms).expect("chmod 755");
    temp_path
}

/// A line of `sh` for a fake shell, or something it starts, that waits for
/// the file at `quoted_path` (already shell-quoted) to exist. It polls every
/// 50 ms and gives up with `exit 1` after 1200 polls, about a minute, so a
/// process a failed test never releases doesn't outlive the test binary
/// polling a deleted tempdir.
#[cfg(unix)]
pub fn wait_for_file_sh(quoted_path: &str) -> String {
    format!(
        "i=0; while [ ! -e {quoted_path} ]; do \
         [ $i -lt 1200 ] || exit 1; i=$((i+1)); sleep 0.05; done"
    )
}
