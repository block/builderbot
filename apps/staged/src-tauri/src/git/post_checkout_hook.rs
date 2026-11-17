//! Which `post-checkout` hooks git would fire in a worktree, for a caller
//! that has something to settle before they run.
//!
//! A hook can do anything: allow a `.envrc`, trust a tool-versions file,
//! write either. The one exception told apart here is the hook `git lfs
//! install` writes, which is common and is known to do none of that.

use std::path::Path;

use super::cli;

/// What `git hook run post-checkout` would run in a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostCheckoutHooks {
    /// Nothing.
    None,
    /// Only the hook `git lfs install` writes, unedited. `git lfs
    /// post-checkout` sets the write flag of lockable files and nothing
    /// else, so it changes nothing a shell reads on starting in the
    /// worktree.
    GitLfsOnly,
    /// Something else, which may do anything. Also the answer when git
    /// could not be asked.
    Other,
}

/// The `post-checkout` hooks `git -C <worktree_path> hook run` would run
/// for `worktree_path`, a worktree of `repo`, as
/// [`run_post_checkout_as_for_a_new_worktree`] is about to.
///
/// Git is asked for the hook directory's file, which follows
/// `core.hooksPath` as the run does (a relative one from the worktree), and
/// for its own list of hooks where it has one (`git hook list`, which
/// came with the hooks git runs from config). The file counts when git
/// would run it: there and executable.
///
/// [`run_post_checkout_as_for_a_new_worktree`]: super::run_post_checkout_as_for_a_new_worktree
pub fn post_checkout_hooks_for_worktree(repo: &Path, worktree_path: &Path) -> PostCheckoutHooks {
    let Some(worktree_str) = worktree_path.to_str() else {
        return PostCheckoutHooks::Other;
    };

    // git -C <worktree> rev-parse --git-path hooks/post-checkout
    let git_path = [
        "-C",
        worktree_str,
        "rev-parse",
        "--git-path",
        "hooks/post-checkout",
    ];
    let in_hooks_dir = match cli::run(repo, &git_path) {
        // Relative to the worktree when it is relative.
        Ok(path) => HookFile::at(&worktree_path.join(path.trim_end_matches(['\n', '\r']))),
        Err(e) => {
            log::warn!(
                "[git] cannot find the post-checkout hook of {}: {e}",
                worktree_path.display()
            );
            return PostCheckoutHooks::Other;
        }
    };

    // git -C <worktree> hook list -z post-checkout
    // One name per hook it would run, or would but for being disabled. It
    // fails when there are none, and on a git that has no list, which has
    // no hooks but the file either.
    let listed = cli::run(
        repo,
        &["-C", worktree_str, "hook", "list", "-z", "post-checkout"],
    )
    .ok()
    .map(|names| names.split('\0').filter(|name| !name.is_empty()).count());

    classify(in_hooks_dir, listed)
}

/// `listed` is how many hooks git listed, `None` when it listed none or
/// has no list. Whatever does not add up is [`PostCheckoutHooks::Other`].
fn classify(in_hooks_dir: HookFile, listed: Option<usize>) -> PostCheckoutHooks {
    match (in_hooks_dir, listed) {
        (HookFile::Absent, None | Some(0)) => PostCheckoutHooks::None,
        // Listed alone, the file is the one hook there is.
        (HookFile::StockGitLfs, None | Some(1)) => PostCheckoutHooks::GitLfsOnly,
        _ => PostCheckoutHooks::Other,
    }
}

/// The `post-checkout` file of a hook directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookFile {
    /// Not there, or not executable, which git ignores.
    Absent,
    StockGitLfs,
    Other,
}

impl HookFile {
    fn at(path: &Path) -> Self {
        let runnable =
            std::fs::metadata(path).is_ok_and(|file| file.is_file() && executable(&file));
        if !runnable {
            return Self::Absent;
        }
        match std::fs::read_to_string(path) {
            Ok(script) if is_stock_git_lfs_post_checkout(&script) => Self::StockGitLfs,
            _ => Self::Other,
        }
    }
}

/// Whether anyone may execute the file. Git asks whether it may itself, so
/// this errs towards a hook being there.
#[cfg(unix)]
fn executable(file: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    file.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_file: &std::fs::Metadata) -> bool {
    true
}

/// Whether `script` is the `post-checkout` hook `git lfs install` writes,
/// with nothing added: a `#!/bin/sh` line, a line that reports a missing
/// `git-lfs` and exits, and `git lfs post-checkout "$@"`.
///
/// Git LFS has reworded the report several times, so its text is free, but
/// only as far as it stays one double-quoted string the shell runs nothing
/// from. A hook this doesn't know is not the stock one.
fn is_stock_git_lfs_post_checkout(script: &str) -> bool {
    let lines: Vec<&str> = script.lines().collect();
    let [shebang, guard, run] = lines[..] else {
        return false;
    };
    shebang == "#!/bin/sh"
        && reports_missing_git_lfs_and_exits(guard)
        && run == r#"git lfs post-checkout "$@""#
}

fn reports_missing_git_lfs_and_exits(line: &str) -> bool {
    let report = line
        .strip_prefix("command -v git-lfs >/dev/null 2>&1 || { ")
        .and_then(|rest| rest.strip_suffix(r#""; exit 2; }"#));
    let message = report.and_then(|report| {
        // What Git LFS writes now, and what it wrote before.
        report
            .strip_prefix(r#"printf >&2 "\n%s\n\n" ""#)
            .or_else(|| report.strip_prefix(r#"echo >&2 ""#))
    });
    // Inside double quotes only these end the string or run something, and
    // a backslash last would keep the closing quote from closing it.
    message.is_some_and(|message| !message.contains(['"', '$', '`']) && !message.ends_with('\\'))
}

/// The hook Git LFS 3.8.1 installs, for tests elsewhere that need a
/// worktree to have it.
#[cfg(test)]
pub(crate) const STOCK_GIT_LFS_POST_CHECKOUT: &str = concat!(
    "#!/bin/sh\n",
    r#"command -v git-lfs >/dev/null 2>&1 || { printf >&2 "\n%s\n\n" "This repository is configured for Git LFS but 'git-lfs' was not found on your path. If you no longer wish to use Git LFS, remove this hook by deleting the 'post-checkout' file in the hooks directory (set by 'core.hookspath'; usually '.git/hooks')."; exit 2; }"#,
    "\n",
    r#"git lfs post-checkout "$@""#,
    "\n",
);

#[cfg(test)]
mod tests;
