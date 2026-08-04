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
