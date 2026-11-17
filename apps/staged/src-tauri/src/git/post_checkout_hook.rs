//! Which `post-checkout` hooks git would fire in a worktree, for a caller
//! that has something to settle before they run, or that means to keep
//! them quiet through a checkout of its own.
//!
//! A hook can do anything: allow a `.envrc`, trust a tool-versions file,
//! write either. The one exception told apart here is the hook `git lfs
//! install` writes, which is common and is known to do none of that.
//!
//! Git finds hooks in two places: the hook directory's file, and, from
//! git 2.54, config (`hook.<name>.event` with `hook.<name>.command`). Both
//! are read here from what decides them, the file and the config, so the
//! answer needs no `git hook list`, which gits before 2.54 lack. Whatever
//! cannot be told for certain counts as a hook.

use std::path::{Path, PathBuf};

use super::branch_config::parse_config_entries_z;
use super::cli::{self, GitOutput};

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
    /// could not be asked, or what it said could not be read.
    Other,
}

/// What a repo can tell of the `post-checkout` hooks of a worktree it has
/// yet to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostCheckoutHooksToCome {
    /// Its config names one, or could not be read. Git runs such a hook
    /// wherever `core.hooksPath` points, so
    /// [`create_detached_worktree_without_hooks`] and
    /// [`switch_detached_worktree_to_branch`] would each fire it.
    ///
    /// [`create_detached_worktree_without_hooks`]: super::create_detached_worktree_without_hooks
    /// [`switch_detached_worktree_to_branch`]: super::switch_detached_worktree_to_branch
    NamedInConfig,
    /// Config names none, and the hook directory is the same from every
    /// worktree (the default one, or an absolute `core.hooksPath`): this
    /// is what the worktree will have.
    Known(PostCheckoutHooks),
    /// Config names none, and `core.hooksPath` is relative, so the hook is
    /// the one in the worktree's own files: its branch decides, and
    /// [`post_checkout_hooks_for_worktree`] tells once it is checked out.
    OfTheBranch,
}

/// The `post-checkout` hooks `git -C <worktree_path> hook run` would run
/// for `worktree_path`, a worktree of `repo`, as
/// [`run_post_checkout_as_for_a_new_worktree`] is about to.
///
/// Git is asked for the hook directory's file, which follows
/// `core.hooksPath` as the run does (a relative one from the worktree), and
/// for the hooks its config names there. The file counts when git would
/// run it: there and executable. A hook config names counts whether or not
/// this git runs such hooks, or config goes on to disable it.
///
/// [`run_post_checkout_as_for_a_new_worktree`]: super::run_post_checkout_as_for_a_new_worktree
pub fn post_checkout_hooks_for_worktree(repo: &Path, worktree_path: &Path) -> PostCheckoutHooks {
    let Some(worktree_str) = worktree_path.to_str() else {
        return PostCheckoutHooks::Other;
    };
    if config_names_post_checkout_hook(repo, &["-C", worktree_str]) {
        return PostCheckoutHooks::Other;
    }
    hook_file_git_finds(repo, worktree_path, &["-C", worktree_str])
        .map_or(PostCheckoutHooks::Other, |file| hook_file_at(&file))
}

/// What `repo` can tell of the `post-checkout` hooks of a worktree before
/// adding it: enough to plan around all but a relative `core.hooksPath`.
///
/// Config is read as `repo`'s main checkout reads it. A conditional
/// include that only the worktree will match (`includeIf "onbranch:…"`)
/// is not seen here; [`post_checkout_hooks_for_worktree`] sees it once the
/// worktree is there.
pub fn post_checkout_hooks_to_come(repo: &Path) -> PostCheckoutHooksToCome {
    if config_names_post_checkout_hook(repo, &[]) {
        return PostCheckoutHooksToCome::NamedInConfig;
    }

    // git config -z --type=path --get core.hooksPath
    // As git reads it: the last value, with a leading `~` expanded.
    let hooks_path = ["config", "-z", "--type=path", "--get", "core.hooksPath"];
    let hooks_dir = match cli::run_output(repo, &hooks_path) {
        Ok(output) => hooks_dir_to_come(&output),
        Err(e) => Err(e.to_string()),
    };
    let file = match hooks_dir {
        Ok(HooksDirToCome::Default) => hook_file_git_finds(repo, repo, &[]),
        Ok(HooksDirToCome::At(dir)) => Some(dir.join("post-checkout")),
        Ok(HooksDirToCome::OfTheBranch) => return PostCheckoutHooksToCome::OfTheBranch,
        Err(why) => {
            log::warn!(
                "[git] cannot read core.hooksPath of {}: {why}",
                repo.display()
            );
            None
        }
    };
    PostCheckoutHooksToCome::Known(
        file.map_or(PostCheckoutHooks::Other, |file| hook_file_at(&file)),
    )
}

/// Where a worktree yet to be added will look for its hooks.
#[derive(Debug, PartialEq, Eq)]
enum HooksDirToCome {
    /// The repo's own hook directory: `core.hooksPath` is not set.
    Default,
    /// An absolute `core.hooksPath`.
    At(PathBuf),
    /// A relative one, resolved from the worktree's root.
    OfTheBranch,
}

/// Read a finished `git config -z --type=path --get core.hooksPath`. Exit
/// 1 with nothing said is git's "not set"; any other failure is not an
/// answer.
fn hooks_dir_to_come(output: &GitOutput) -> Result<HooksDirToCome, String> {
    if nothing_matched(output) {
        return Ok(HooksDirToCome::Default);
    }
    if !output.success {
        return Err(describe_failure(output));
    }
    let value = output.stdout.strip_suffix('\0').unwrap_or(&output.stdout);
    let dir = Path::new(value);
    Ok(if dir.is_absolute() {
        HooksDirToCome::At(dir.to_path_buf())
    } else {
        HooksDirToCome::OfTheBranch
    })
}

/// The hook directory's `post-checkout` as git finds it with `at` (`-C
/// <dir>`, or nothing for `repo` itself), a relative answer being relative
/// to `dir`. `None` when git cannot say.
fn hook_file_git_finds(repo: &Path, dir: &Path, at: &[&str]) -> Option<PathBuf> {
    // git [-C <dir>] rev-parse --git-path hooks/post-checkout
    let mut args = at.to_vec();
    args.extend(["rev-parse", "--git-path", "hooks/post-checkout"]);
    match cli::run(repo, &args) {
        Ok(path) => Some(dir.join(path.trim_end_matches(['\n', '\r']))),
        Err(e) => {
            log::warn!(
                "[git] cannot find the post-checkout hook of {}: {e}",
                dir.display()
            );
            None
        }
    }
}

/// Every `hook.<name>.event` in config.
const HOOK_EVENTS: &str = r"^hook\..*\.event$";

/// Whether the config git reads with `at` (`-C <dir>`, or nothing for
/// `repo` itself) names a `post-checkout` hook. Not being able to tell
/// counts as naming one.
fn config_names_post_checkout_hook(repo: &Path, at: &[&str]) -> bool {
    // git [-C <dir>] config -z --get-regexp '^hook\..*\.event$'
    let mut args = at.to_vec();
    args.extend(["config", "-z", "--get-regexp", HOOK_EVENTS]);
    let named = match cli::run_output(repo, &args) {
        Ok(output) => events_name_post_checkout(&output),
        Err(e) => Err(e.to_string()),
    };
    named.unwrap_or_else(|why| {
        log::warn!(
            "[git] cannot read the hooks config names for {}: {why}",
            repo.display()
        );
        true
    })
}

/// Read a finished `git config -z --get-regexp` over [`HOOK_EVENTS`]:
/// whether any hook is given the `post-checkout` event. One that is counts
/// even if a later empty value, or `hook.<name>.enabled`, takes it back.
/// Exit 1 with nothing said is git's "no key matched"; any other failure,
/// a child that was killed included, is not an answer.
fn events_name_post_checkout(output: &GitOutput) -> Result<bool, String> {
    if nothing_matched(output) {
        return Ok(false);
    }
    if !output.success {
        return Err(describe_failure(output));
    }
    Ok(parse_config_entries_z(&output.stdout)
        .iter()
        .any(|(_, event)| event.as_deref() == Some("post-checkout")))
}

/// Whether a `git config` query ended the way it does when nothing is set:
/// exit 1, with no output and no complaint.
fn nothing_matched(output: &GitOutput) -> bool {
    output.code == Some(1) && output.stdout.is_empty() && output.stderr.trim().is_empty()
}

fn describe_failure(output: &GitOutput) -> String {
    let stderr = output.stderr.trim();
    match (stderr.is_empty(), output.code) {
        (false, _) => stderr.to_string(),
        (true, Some(code)) => format!("git config exited with status {code}"),
        (true, None) => "git config was killed by a signal".to_string(),
    }
}

/// What the hook directory's `post-checkout` at `path` is, when config
/// names no hook and it is the one there can be: nothing when it is not
/// there or not executable, which git ignores.
fn hook_file_at(path: &Path) -> PostCheckoutHooks {
    let runnable = std::fs::metadata(path).is_ok_and(|file| file.is_file() && executable(&file));
    if !runnable {
        return PostCheckoutHooks::None;
    }
    match std::fs::read_to_string(path) {
        Ok(script) if is_stock_git_lfs_post_checkout(&script) => PostCheckoutHooks::GitLfsOnly,
        _ => PostCheckoutHooks::Other,
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
