//! Every repo here names its own `core.hooksPath`, so a hook directory set
//! for the whole machine (a hook manager's, say) is not what gets found.

use std::path::{Path, PathBuf};

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

/// A repo with one commit, its hook directory an empty one under `root`,
/// and a linked worktree at that commit. Returns the hook directory and
/// the worktree.
fn repo_with_worktree(root: &Path) -> (TempGitRepo, PathBuf, PathBuf) {
    let repo = TempGitRepo::new();
    repo.write_file("tracked.txt", "base\n");
    let base = repo.commit("base");
    let hooks = root.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    repo.run_git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let worktree = root.join("wt");
    super::super::create_detached_worktree_without_hooks(repo.path(), &base, &worktree, false)
        .unwrap();
    (repo, hooks, worktree)
}

#[cfg(unix)]
fn write_hook(hooks: &Path, script: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(hooks).unwrap();
    let hook = hooks.join("post-checkout");
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Whether this machine's git lists hooks, and so runs the ones config
/// names: it lists the one in `hooks`.
#[cfg(unix)]
fn git_lists_hooks(repo: &Path, hooks: &Path) -> bool {
    write_hook(hooks, "#!/bin/sh\n", 0o755);
    let lists = cli::run(repo, &["hook", "list", "-z", "post-checkout"]).is_ok();
    std::fs::remove_file(hooks.join("post-checkout")).unwrap();
    lists
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

#[test]
fn only_a_file_with_nothing_else_listed_counts_as_none_or_as_git_lfs() {
    use HookFile::{Absent, Other, StockGitLfs};

    // A git with no list, or one that listed nothing: the file decides.
    assert_eq!(classify(Absent, None), PostCheckoutHooks::None);
    assert_eq!(classify(StockGitLfs, None), PostCheckoutHooks::GitLfsOnly);
    assert_eq!(classify(Other, None), PostCheckoutHooks::Other);

    // The file is what git listed.
    assert_eq!(
        classify(StockGitLfs, Some(1)),
        PostCheckoutHooks::GitLfsOnly
    );
    assert_eq!(classify(Other, Some(1)), PostCheckoutHooks::Other);

    // A hook that config names, alone or beside the file.
    assert_eq!(classify(Absent, Some(1)), PostCheckoutHooks::Other);
    assert_eq!(classify(StockGitLfs, Some(2)), PostCheckoutHooks::Other);
    assert_eq!(classify(Absent, Some(2)), PostCheckoutHooks::Other);

    // A list and a file that disagree.
    assert_eq!(classify(Absent, Some(0)), PostCheckoutHooks::None);
    assert_eq!(classify(StockGitLfs, Some(0)), PostCheckoutHooks::Other);
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

/// A hook directory with no `post-checkout`, or one git would ignore for
/// not being executable, has no hook; any other file is one that may do
/// anything, and the stock Git LFS hook is told apart from those.
#[cfg(unix)]
#[test]
fn the_hook_directory_file_is_none_other_or_git_lfs() {
    let root = TempDir::new("post-checkout-hooks");
    let (repo, hooks, worktree) = repo_with_worktree(root.path());
    let found = || post_checkout_hooks_for_worktree(repo.path(), &worktree);

    assert_eq!(found(), PostCheckoutHooks::None, "an empty hook directory");

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
    let (repo, hooks, worktree) = repo_with_worktree(root.path());

    // Not through `repo.run_git`, which turns the hook directory off.
    cli::run(repo.path(), &["lfs", "install", "--local"]).expect("git lfs install");

    let installed = std::fs::read_to_string(hooks.join("post-checkout")).expect("the hook");
    assert_eq!(
        post_checkout_hooks_for_worktree(repo.path(), &worktree),
        PostCheckoutHooks::GitLfsOnly,
        "{installed}"
    );
}

/// A relative `core.hooksPath` is looked up from the worktree, as `git
/// hook run` there does: the hook is the branch's copy, and a branch
/// without the directory has none, whatever the repo's main checkout has.
#[cfg(unix)]
#[test]
fn a_relative_hooks_path_is_the_worktrees_own() {
    let root = TempDir::new("post-checkout-relative");
    let (repo, _, without) = repo_with_worktree(root.path());
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
}

/// A git that runs hooks named in config lists them, and one of those is a
/// hook that may do anything, with the stock Git LFS file or without it.
#[cfg(unix)]
#[test]
fn a_hook_named_in_config_is_another_hook() {
    let root = TempDir::new("post-checkout-config");
    let (repo, hooks, worktree) = repo_with_worktree(root.path());
    if !git_lists_hooks(repo.path(), &hooks) {
        eprintln!(
            "skipping a_hook_named_in_config_is_another_hook: this git has no `git hook list`"
        );
        return;
    }
    let found = || post_checkout_hooks_for_worktree(repo.path(), &worktree);
    assert_eq!(found(), PostCheckoutHooks::None);

    repo.run_git(&["config", "hook.setup.event", "post-checkout"]);
    repo.run_git(&["config", "hook.setup.command", "direnv allow"]);
    assert_eq!(found(), PostCheckoutHooks::Other, "config's hook alone");

    write_hook(&hooks, STOCK_GIT_LFS_POST_CHECKOUT, 0o755);
    assert_eq!(found(), PostCheckoutHooks::Other, "beside the stock file");

    // Another event's hook is not this one's.
    repo.run_git(&["config", "hook.setup.event", "post-merge"]);
    assert_eq!(found(), PostCheckoutHooks::GitLfsOnly);
}
