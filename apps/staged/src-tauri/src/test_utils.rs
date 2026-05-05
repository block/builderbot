use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

const GIT_LOCAL_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

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
        let mut command = Command::new("git");
        command
            .arg("-c")
            .arg("core.hooksPath=/dev/null")
            .arg("-C")
            .arg(&self.path)
            .args(args);
        for key in GIT_LOCAL_ENV_VARS {
            command.env_remove(key);
        }

        let output = command.output().unwrap();

        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for TempGitRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
