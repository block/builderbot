//! The git asked here reads no config but each repo's own
//! ([`OwnConfigOnly`]), so a hook directory or a hook the machine's config
//! sets (a hook manager's, say) is not what gets found.

use std::path::{Path, PathBuf};

use super::super::cli::own_config_only::OwnConfigOnly;
use super::*;
use crate::test_utils::{TempDir, TempGitRepo};

/// The stock hook as Git LFS wrote it before it took to `printf`, oldest
/// last.
const EARLIER_STOCK_HOOKS: [&str; 3] = [
    concat!(
        "#!/bin/sh\n",
        r#"command -v git-lfs >/dev/null 2>&1 || { echo >&2 "\nThis repository is configured for Git LFS but 'git-lfs' was not found on your path. If you no longer wish to use Git LFS, remove this hook by deleting the 'post-checkout' file in the hooks directory (set by 'core.hookspath'; usually '.git/hooks').\n"; exit 2; }"#,
        "\n",
        r#"git lfs post-checkout "$@""#,
    ),
    concat!(
        "#!/bin/sh\n",
        r#"command -v git-lfs >/dev/null 2>&1 || { echo >&2 "\nThis repository is configured for Git LFS but 'git-lfs' was not found on your path. If you no longer wish to use Git LFS, remove this hook by deleting '.git/hooks/post-checkout'.\n"; exit 2; }"#,
        "\n",
        r#"git lfs post-checkout "$@""#,
    ),
    concat!(
        "#!/bin/sh\n",
        r#"command -v git-lfs >/dev/null 2>&1 || { echo >&2 "\nThis repository is configured for Git LFS but 'git-lfs' was not found on your path. If you no longer wish to use Git LFS, remove this hook by deleting .git/hooks/post-checkout.\n"; exit 2; }"#,
        "\n",
        r#"git lfs post-checkout "$@""#,
    ),
];

/// A repo with one commit and a linked worktree at that commit under
/// `root`, asked about with no config but its own. Returns its hook
/// directory, the default one, and the worktree.
fn repo_with_worktree(root: &Path) -> (TempGitRepo, PathBuf, PathBuf, OwnConfigOnly) {
    let repo = TempGitRepo::new();
    let own_config = OwnConfigOnly::under(repo.path());
    repo.write_file("tracked.txt", "base\n");
    let base = repo.commit("base");
    let hooks = repo.path().join(".git").join("hooks");
    let worktree = root.join("wt");
    super::super::create_detached_worktree_without_hooks(repo.path(), &base, &worktree, false)
        .unwrap();
    (repo, hooks, worktree, own_config)
}

#[cfg(unix)]
fn write_hook(hooks: &Path, script: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(hooks).unwrap();
    let hook = hooks.join("post-checkout");
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn git_lfs_installed(test: &str) -> bool {
    let installed = std::process::Command::new("git")
        .args(["lfs", "version"])
        .output()
        .is_ok_and(|out| out.status.success());
    if !installed {
        eprintln!("skipping {test}: git-lfs is not installed");
    }
    installed
}

/// A `git config` query that ended with `code` (`None`: killed), having
/// printed these.
fn finished(code: Option<i32>, stdout: &str, stderr: &str) -> GitOutput {
    GitOutput {
        success: code == Some(0),
        code,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    }
}

/// What does not answer the question, whichever it was: a config git
/// cannot read, a usage error, an exit 1 that came with a complaint or
/// with output, a child that was killed.
const NO_ANSWER: [(Option<i32>, &str, &str); 6] = [
    (
        Some(128),
        "",
        "fatal: bad config line 3 in file .git/config\n",
    ),
    (Some(129), "", "usage: git config [<options>]\n"),
    (Some(3), "", ""),
    (Some(1), "", "warning: something else is wrong\n"),
    (Some(1), "hook.setup.event\npost-merge\0", ""),
    (None, "", ""),
];

#[test]
fn config_names_no_hook_only_when_git_says_so() {
    let named = |stdout: &str| events_name_post_checkout(&finished(Some(0), stdout, ""));

    // No `hook.<name>.event` at all: exit 1, and nothing said.
    assert_eq!(
        events_name_post_checkout(&finished(Some(1), "", "")),
        Ok(false)
    );
    // Hooks, for other events.
    assert_eq!(
        named("hook.lint.event\npre-commit\0hook.lint.event\npre-push\0"),
        Ok(false)
    );
    assert_eq!(
        named("hook.lint.event\npre-commit\0hook.setup.event\npost-checkout\0"),
        Ok(true)
    );
    // Taken back by an empty value, it still counts.
    assert_eq!(
        named("hook.setup.event\npost-checkout\0hook.setup.event\n\0"),
        Ok(true)
    );
    // The event, not a name or a value that only contains it.
    assert_eq!(
        named("hook.post-checkout-setup.event\npost-checkout-setup\0"),
        Ok(false)
    );

    for (code, stdout, stderr) in NO_ANSWER {
        let unreadable = events_name_post_checkout(&finished(code, stdout, stderr));
        assert!(unreadable.is_err(), "{code:?} {stdout:?} {stderr:?}");
    }
}

#[test]
fn the_hook_directory_to_come_is_the_default_an_absolute_one_or_the_branchs() {
    let read = |stdout: &str| hooks_dir_to_come(&finished(Some(0), stdout, ""));

    assert_eq!(
        hooks_dir_to_come(&finished(Some(1), "", "")),
        Ok(HooksDirToCome::Default)
    );
    assert_eq!(
        read("/opt/hook-manager/hooks\0"),
        Ok(HooksDirToCome::At(PathBuf::from("/opt/hook-manager/hooks")))
    );
    assert_eq!(read(".githooks\0"), Ok(HooksDirToCome::OfTheBranch));
    assert_eq!(read("tools/hooks\0"), Ok(HooksDirToCome::OfTheBranch));
    // Set to nothing: left for the worktree to tell.
    assert_eq!(read("\0"), Ok(HooksDirToCome::OfTheBranch));

    for (code, stdout, stderr) in NO_ANSWER {
        let unreadable = hooks_dir_to_come(&finished(code, stdout, stderr));
        assert!(unreadable.is_err(), "{code:?} {stdout:?} {stderr:?}");
    }
}

/// A repo whose config git cannot read answers neither question, and that
/// counts as a hook: one that may do anything in a worktree it has, and
/// one nothing would keep quiet in a worktree it is yet to add.
#[test]
fn a_repo_that_cannot_be_asked_counts_as_having_a_hook() {
    let root = TempDir::new("post-checkout-unreadable");
    let (repo, _, worktree, _own_config) = repo_with_worktree(root.path());
    assert_eq!(
        post_checkout_hooks_for_worktree(repo.path(), &worktree),
        PostCheckoutHooks::None
    );
    assert_eq!(
        post_checkout_hooks_to_come(repo.path()),
        PostCheckoutHooksToCome::Known(PostCheckoutHooks::None)
    );

    let config = repo.path().join(".git").join("config");
    let mut broken = std::fs::read_to_string(&config).unwrap();
    broken.push_str("[hook \"setup\n");
    std::fs::write(&config, broken).unwrap();

    assert_eq!(
        post_checkout_hooks_for_worktree(repo.path(), &worktree),
        PostCheckoutHooks::Other
    );
    assert_eq!(
        post_checkout_hooks_to_come(repo.path()),
        PostCheckoutHooksToCome::NamedInConfig
    );
    // As does a place that is no repo at all.
    let nowhere = root.path().join("nowhere");
    assert_eq!(
        post_checkout_hooks_for_worktree(&nowhere, &worktree),
        PostCheckoutHooks::Other
    );
    assert_eq!(
        post_checkout_hooks_to_come(&nowhere),
        PostCheckoutHooksToCome::NamedInConfig
    );
}

#[test]
fn the_hook_git_lfs_installs_is_told_by_its_three_lines() {
    assert!(is_stock_git_lfs_post_checkout(STOCK_GIT_LFS_POST_CHECKOUT));
    for earlier in EARLIER_STOCK_HOOKS {
        assert!(is_stock_git_lfs_post_checkout(earlier), "{earlier}");
    }
    // Reworded again, as it has been before.
    assert!(is_stock_git_lfs_post_checkout(concat!(
        "#!/bin/sh\n",
        r#"command -v git-lfs >/dev/null 2>&1 || { printf >&2 "\n%s\n\n" "Git LFS is missing; see 'core.hookspath'."; exit 2; }"#,
        "\n",
        r#"git lfs post-checkout "$@""#,
        "\n",
    )));
}

#[test]
fn a_git_lfs_hook_with_anything_added_or_changed_is_not_the_stock_one() {
    let stock = STOCK_GIT_LFS_POST_CHECKOUT;
    let guard = stock.lines().nth(1).unwrap();
    let with_guard = |guard: &str| format!("#!/bin/sh\n{guard}\ngit lfs post-checkout \"$@\"\n");
    assert!(is_stock_git_lfs_post_checkout(&with_guard(guard)));

    let edited = [
        // A line more, wherever it goes.
        format!("{stock}direnv allow\n"),
        stock.replacen("#!/bin/sh\n", "#!/bin/sh\ndirenv allow\n", 1),
        format!("{stock}\n"),
        // Another shell, another command, another hook's command.
        stock.replacen("#!/bin/sh", "#!/bin/bash", 1),
        stock.replacen(
            "git lfs post-checkout \"$@\"",
            "git lfs post-checkout \"$@\"; direnv allow",
            1,
        ),
        stock.replacen("git lfs post-checkout", "git lfs post-merge", 1),
        stock.replacen(
            "git lfs post-checkout \"$@\"",
            "git lfs post-checkout \"$@\" && mise trust",
            1,
        ),
        // A report that runs something, or stops being one string.
        with_guard(&guard.replacen("This repository", "$(direnv allow) This repository", 1)),
        with_guard(&guard.replacen("This repository", "`direnv allow` This repository", 1)),
        with_guard(&guard.replacen(
            "This repository",
            "\"; direnv allow; echo \"This repository",
            1,
        )),
        with_guard(&guard.replacen("'.git/hooks').\"", "'.git/hooks').\\\"", 1)),
        // A guard that goes on to something else, or doesn't exit.
        with_guard(&guard.replacen("exit 2; }", "exit 2; }; direnv allow", 1)),
        with_guard(&guard.replacen("exit 2; }", "direnv allow; }", 1)),
        with_guard(&guard.replacen("command -v git-lfs", "direnv allow; command -v git-lfs", 1)),
        with_guard(&guard.replacen("printf >&2", "direnv allow; printf >&2", 1)),
        // Nothing at all, and a hook of another kind.
        String::new(),
        "#!/bin/sh\nexec lefthook run post-checkout \"$@\"\n".to_string(),
    ];
    for script in edited {
        assert!(!is_stock_git_lfs_post_checkout(&script), "{script}");
    }
}

/// Asked of the worktree, and of the repo before it adds one: with the
/// hook directory the same from both, the two must agree.
#[cfg(unix)]
fn found_from_both(repo: &Path, worktree: &Path) -> PostCheckoutHooks {
    let found = post_checkout_hooks_for_worktree(repo, worktree);
    assert_eq!(
        post_checkout_hooks_to_come(repo),
        PostCheckoutHooksToCome::Known(found)
    );
    found
}

/// A hook directory with no `post-checkout`, or one git would ignore for
/// not being executable, has no hook; any other file is one that may do
/// anything, and the stock Git LFS hook is told apart from those. The
/// directory is the repo's default one, which a worktree shares, so the
/// repo can tell the same before it adds the worktree.
#[cfg(unix)]
#[test]
fn the_hook_directory_file_is_none_other_or_git_lfs() {
    let root = TempDir::new("post-checkout-hooks");
    let (repo, hooks, worktree, _own_config) = repo_with_worktree(root.path());
    let found = || found_from_both(repo.path(), &worktree);

    assert_eq!(found(), PostCheckoutHooks::None, "no hook but the samples");

    write_hook(&hooks, "#!/bin/sh\ndirenv allow\n", 0o644);
    assert_eq!(found(), PostCheckoutHooks::None, "not executable");

    write_hook(&hooks, "#!/bin/sh\ndirenv allow\n", 0o755);
    assert_eq!(found(), PostCheckoutHooks::Other);

    write_hook(&hooks, STOCK_GIT_LFS_POST_CHECKOUT, 0o755);
    assert_eq!(found(), PostCheckoutHooks::GitLfsOnly);

    write_hook(&hooks, STOCK_GIT_LFS_POST_CHECKOUT, 0o644);
    assert_eq!(
        found(),
        PostCheckoutHooks::None,
        "stock, but not executable"
    );

    write_hook(
        &hooks,
        &format!("{STOCK_GIT_LFS_POST_CHECKOUT}mise trust\n"),
        0o755,
    );
    assert_eq!(
        found(),
        PostCheckoutHooks::Other,
        "stock, with a line added"
    );
}

/// An absolute `core.hooksPath`, a hook manager's say, takes the place of
/// the default directory for the repo and its worktrees alike, so the repo
/// can tell what a worktree will have there too.
#[cfg(unix)]
#[test]
fn an_absolute_hooks_path_is_the_same_from_the_repo_and_its_worktrees() {
    let root = TempDir::new("post-checkout-absolute");
    let (repo, default_hooks, worktree, _own_config) = repo_with_worktree(root.path());
    let found = || found_from_both(repo.path(), &worktree);
    let hooks = root.path().join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    repo.run_git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    // No longer where git looks.
    write_hook(&default_hooks, "#!/bin/sh\ndirenv allow\n", 0o755);

    assert_eq!(found(), PostCheckoutHooks::None, "an empty hook directory");

    write_hook(&hooks, STOCK_GIT_LFS_POST_CHECKOUT, 0o755);
    assert_eq!(found(), PostCheckoutHooks::GitLfsOnly);

    write_hook(&hooks, "#!/bin/sh\nexec hook-manager \"$@\"\n", 0o755);
    assert_eq!(found(), PostCheckoutHooks::Other);
}

/// What `git lfs install` writes on this machine is recognised, whichever
/// Git LFS that is: a rewording this fails on needs teaching to
/// [`is_stock_git_lfs_post_checkout`], which until then costs such repos
/// only the head start.
#[cfg(unix)]
#[test]
fn the_hook_this_machines_git_lfs_installs_is_recognised() {
    if !git_lfs_installed("the_hook_this_machines_git_lfs_installs_is_recognised") {
        return;
    }
    let root = TempDir::new("post-checkout-lfs");
    let (repo, hooks, worktree, _own_config) = repo_with_worktree(root.path());

    // Not through `repo.run_git`, which turns the hook directory off.
    cli::run(repo.path(), &["lfs", "install", "--local"]).expect("git lfs install");

    let installed = std::fs::read_to_string(hooks.join("post-checkout")).expect("the hook");
    assert_eq!(
        post_checkout_hooks_for_worktree(repo.path(), &worktree),
        PostCheckoutHooks::GitLfsOnly,
        "{installed}"
    );
    assert_eq!(
        post_checkout_hooks_to_come(repo.path()),
        PostCheckoutHooksToCome::Known(PostCheckoutHooks::GitLfsOnly)
    );
}

/// A relative `core.hooksPath` is looked up from the worktree, as `git
/// hook run` there does: the hook is the branch's copy, and a branch
/// without the directory has none, whatever the repo's main checkout has.
/// So the repo cannot tell what a worktree yet to be added will have.
#[cfg(unix)]
#[test]
fn a_relative_hooks_path_is_the_worktrees_own() {
    let root = TempDir::new("post-checkout-relative");
    let (repo, _, without, _own_config) = repo_with_worktree(root.path());
    repo.run_git(&["config", "core.hooksPath", ".githooks"]);
    write_hook(
        &repo.path().join(".githooks"),
        "#!/bin/sh\ndirenv allow\n",
        0o755,
    );
    let with_hooks = repo.commit("hooks");
    let with = root.path().join("with");
    super::super::create_detached_worktree_without_hooks(repo.path(), &with_hooks, &with, false)
        .unwrap();

    assert_eq!(
        post_checkout_hooks_for_worktree(repo.path(), &with),
        PostCheckoutHooks::Other
    );
    assert_eq!(
        post_checkout_hooks_for_worktree(repo.path(), &without),
        PostCheckoutHooks::None,
        "the main checkout's copy is not this worktree's"
    );
    assert_eq!(
        post_checkout_hooks_to_come(repo.path()),
        PostCheckoutHooksToCome::OfTheBranch
    );
}

/// A hook that config names is one that may do anything, with the stock
/// Git LFS file or without it, and one the repo knows of before it adds a
/// worktree. That is read from config, so it is the answer on a git too
/// old to run such a hook as well.
#[cfg(unix)]
#[test]
fn a_hook_named_in_config_is_another_hook() {
    let root = TempDir::new("post-checkout-config");
    let (repo, hooks, worktree, _own_config) = repo_with_worktree(root.path());
    let found = || post_checkout_hooks_for_worktree(repo.path(), &worktree);
    let to_come = || post_checkout_hooks_to_come(repo.path());
    assert_eq!(found(), PostCheckoutHooks::None);

    // Another event's hook is not this one's.
    repo.run_git(&["config", "hook.setup.event", "post-merge"]);
    repo.run_git(&["config", "hook.setup.command", "direnv allow"]);
    assert_eq!(found(), PostCheckoutHooks::None);
    assert_eq!(
        to_come(),
        PostCheckoutHooksToCome::Known(PostCheckoutHooks::None)
    );

    repo.run_git(&["config", "--add", "hook.setup.event", "post-checkout"]);
    assert_eq!(found(), PostCheckoutHooks::Other, "config's hook alone");
    assert_eq!(to_come(), PostCheckoutHooksToCome::NamedInConfig);

    write_hook(&hooks, STOCK_GIT_LFS_POST_CHECKOUT, 0o755);
    assert_eq!(found(), PostCheckoutHooks::Other, "beside the stock file");
    assert_eq!(to_come(), PostCheckoutHooksToCome::NamedInConfig);

    repo.run_git(&["config", "--unset-all", "hook.setup.event"]);
    assert_eq!(found(), PostCheckoutHooks::GitLfsOnly);
    assert_eq!(
        to_come(),
        PostCheckoutHooksToCome::Known(PostCheckoutHooks::GitLfsOnly)
    );
}
