//! Whether git would accept a name as a branch.
//!
//! A desktop form picks a base from the repository's real branches and
//! feeds a new name to the worktree code while the user is still looking. A
//! control caller's names are typed, and a name git refuses would write
//! every row and then fail in the background worktree setup, where only the
//! log says so. The rules are git's own (`git check-ref-format --branch`),
//! applied here so the refusal comes before any lookup or write.

/// `Ok` when git would accept `name` as a branch name. The error says which
/// rule it breaks, ready to follow "is not a valid branch name:".
pub(super) fn check_branch_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("it is empty");
    }
    // Git reads a leading `-` as an option wherever the name is passed on a
    // command line, and refuses it as a branch for that reason.
    if name.starts_with('-') {
        return Err("it starts with '-'");
    }
    if name == "HEAD" {
        return Err("'HEAD' is reserved");
    }
    if name.contains("@{") {
        return Err("it contains '@{'");
    }
    if name.contains("..") {
        return Err("it contains '..'");
    }
    if name.ends_with('.') {
        return Err("it ends with '.'");
    }
    if name.chars().any(|c| {
        c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    }) {
        return Err("it contains a space, a control character or one of ~ ^ : ? * [ \\");
    }
    for component in name.split('/') {
        if component.is_empty() {
            return Err("it starts or ends with '/', or contains '//'");
        }
        if component.starts_with('.') {
            return Err("a part of it starts with '.'");
        }
        if component.ends_with(".lock") {
            return Err("a part of it ends with '.lock'");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const ACCEPTED: &[&str] = &[
        "main",
        "feature-x",
        "feature/x",
        "release/2.x",
        "user/fix_login-2",
        "a.b",
        "caf\u{e9}",
        "x@y",
        "@",
        "head",
        "fix#12",
        "100%",
        "a-",
        "lock",
        "x.lockfile",
    ];

    const REFUSED: &[&str] = &[
        "",
        "-",
        "-x",
        "--upload-pack=evil",
        "-f",
        "HEAD",
        "a@{b",
        "a..b",
        "a.",
        "a b",
        "a~1",
        "a^",
        "a:b",
        "a?",
        "a*",
        "a[b",
        "a\\b",
        "a\tb",
        "a\u{7f}b",
        "/a",
        "a/",
        "a//b",
        ".a",
        "a/.b",
        "a.lock",
        "a.lock/b",
    ];

    #[test]
    fn names_git_accepts_are_accepted() {
        for name in ACCEPTED {
            assert_eq!(check_branch_name(name), Ok(()), "{name:?}");
        }
    }

    #[test]
    fn names_git_refuses_are_refused_with_the_rule_they_break() {
        for name in REFUSED {
            assert!(check_branch_name(name).is_err(), "{name:?}");
        }
        assert_eq!(check_branch_name("-x"), Err("it starts with '-'"));
        assert_eq!(check_branch_name("a..b"), Err("it contains '..'"));
        assert_eq!(
            check_branch_name("a.lock"),
            Err("a part of it ends with '.lock'")
        );
    }

    /// The rules are git's, so git is the oracle: every name in both tables
    /// gets the verdict `git check-ref-format --branch` gives it.
    #[test]
    fn the_verdicts_are_the_ones_git_gives() {
        for name in ACCEPTED.iter().chain(REFUSED) {
            // `--branch` also expands `@{-N}`, which needs a repository;
            // none of the names use it, and the temp dir keeps git from
            // finding one by accident.
            let git = Command::new("git")
                .args(["check-ref-format", "--branch", name])
                .current_dir(std::env::temp_dir())
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .output()
                .expect("git should run");
            assert_eq!(
                check_branch_name(name).is_ok(),
                git.status.success(),
                "{name:?}: git said {:?}",
                String::from_utf8_lossy(&git.stderr)
            );
        }
    }
}
