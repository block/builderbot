use std::ffi::{OsStr, OsString};
use std::process::Command;

/// Remove git-specific environment from a command before running git: every
/// `GIT_*` variable the child would inherit from this process, and any set on
/// the command itself. Under a git hook, `GIT_DIR` and friends point at the
/// repo running the hook and would override `-C`.
pub(crate) fn strip_git_env(command: &mut Command) {
    let keys: Vec<OsString> = std::env::vars_os()
        .map(|(key, _)| key)
        .chain(command.get_envs().map(|(key, _)| key.to_os_string()))
        .filter(|key| starts_with_git_prefix(key))
        .collect();
    for key in keys {
        command.env_remove(key);
    }
}

fn starts_with_git_prefix(key: &OsStr) -> bool {
    key.to_string_lossy().starts_with("GIT_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_entry<'a>(command: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
        command
            .get_envs()
            .find(|(k, _)| *k == OsStr::new(key))
            .map(|(_, v)| v)
    }

    /// The repo-location variables a git hook exports must end up explicitly
    /// removed (`get_envs` reports `None`), not merely left to inheritance,
    /// while unrelated variables on the command are untouched.
    #[test]
    fn strip_git_env_removes_git_vars_set_on_the_command() {
        let git_keys = [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_PREFIX",
        ];
        let mut command = Command::new("git");
        for key in git_keys {
            command.env(key, "/elsewhere");
        }
        command.env("LC_ALL", "C");

        strip_git_env(&mut command);

        for key in git_keys {
            assert_eq!(
                env_entry(&command, key),
                Some(None),
                "{key} should be removed"
            );
        }
        assert_eq!(env_entry(&command, "LC_ALL"), Some(Some(OsStr::new("C"))));
    }
}
