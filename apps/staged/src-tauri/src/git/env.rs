use std::ffi::OsStr;
use std::process::Command;

/// Remove inherited git-specific environment from a command before running git.
pub(crate) fn strip_git_env(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if starts_with_git_prefix(&key) {
            command.env_remove(key);
        }
    }
}

fn starts_with_git_prefix(key: &OsStr) -> bool {
    key.to_string_lossy().starts_with("GIT_")
}
