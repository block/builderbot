//! Which directory a working directory's shell-env snapshot is captured in.
//!
//! An interactive login shell's environment depends on where it starts only
//! through tools that react to the directory. Hermit activates the nearest
//! `bin/activate-hermit` above the cwd, and direnv, mise, asdf and nvm-style
//! auto-switch hooks read marker files in the cwd and its ancestors. A
//! subdirectory of a worktree with none of those in between gets the same env
//! as the worktree root apart from `PWD` and `OLDPWD`, so the cache captures
//! once at the root and shares the snapshot (see [`ShellEnv`]).
//!
//! [`ShellEnv`]: super::ShellEnv

use std::path::{Component, Path, PathBuf};

/// Files that mean a directory's env can differ from its env root's, because
/// a directory-scoped tool would apply them in a shell started there but not
/// in one started at the root. Any of these in the requested directory, or
/// in a directory between it and the env root, keeps the requested directory
/// as its own key. Markers at the root itself don't: the root's capture
/// already applied them.
///
/// - `.envrc`: direnv.
/// - `mise.toml`, `.mise.toml`, `mise.local.toml`, `.mise.local.toml`,
///   `.mise/config.toml`, `.config/mise.toml`, `.config/mise/config.toml`:
///   mise project config.
/// - `.tool-versions`: asdf, and mise reads it too.
/// - `.nvmrc`, `.node-version`: nvm/fnm/volta-style Node version pins, which
///   `cd` hooks switch PATH on.
/// - `.python-version`, `.ruby-version`: pyenv-virtualenv and chruby/rbenv
///   auto-switching, which export `VIRTUAL_ENV`, `GEM_HOME` and PATH.
///
/// A missing entry costs a shell env that silently lacks what the tool would
/// have set; an extra one only costs one more capture.
pub(super) const DIR_ENV_MARKERS: &[&str] = &[
    ".envrc",
    "mise.toml",
    ".mise.toml",
    "mise.local.toml",
    ".mise.local.toml",
    ".mise/config.toml",
    ".config/mise.toml",
    ".config/mise/config.toml",
    ".tool-versions",
    ".nvmrc",
    ".node-version",
    ".python-version",
    ".ruby-version",
];

/// Resolve the directory whose captured env `dir` can share: its env root.
///
/// Walking up from `dir` itself, the first directory that is a Hermit root
/// (contains `bin/activate-hermit`) or a git top level (contains `.git`,
/// either a directory or the file a linked worktree or submodule has) is the
/// env root. A Hermit root inside a repo is met before the repo's top level,
/// so it wins. The walk stops at the first git top level rather than looking
/// past it for a Hermit root further up. Two repos, or two worktrees of one
/// repo, therefore never share a key: each one's own Hermit pins, or the lack
/// of them, can give it a different env.
///
/// `dir` keeps its own key when:
/// - no env root is found below `$HOME` or the filesystem root. Neither is
///   examined on behalf of a directory beneath it, so a dotfiles repo at
///   `$HOME` never folds unrelated directories into home's env, and `$HOME`
///   itself stays its own key.
/// - `dir`, or a directory between it and the env root, has one of
///   [`DIR_ENV_MARKERS`].
/// - `dir` is relative or contains `..`, where a lexical walk would visit the
///   wrong ancestors.
///
/// Only stats files; it never runs git.
pub(super) fn resolve_env_root(dir: &Path, home: Option<&Path>) -> PathBuf {
    if !dir.is_absolute() || dir.components().any(|c| c == Component::ParentDir) {
        return dir.to_path_buf();
    }
    for candidate in dir.ancestors() {
        if home == Some(candidate) || candidate.parent().is_none() {
            break;
        }
        if is_env_root(candidate) {
            return candidate.to_path_buf();
        }
        if has_dir_env_marker(candidate) {
            break;
        }
    }
    dir.to_path_buf()
}

fn is_env_root(dir: &Path) -> bool {
    dir.join("bin/activate-hermit").exists() || dir.join(".git").exists()
}

fn has_dir_env_marker(dir: &Path) -> bool {
    DIR_ENV_MARKERS
        .iter()
        .any(|marker| dir.join(marker).exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Scratch tree for one test. Paths passed to [`resolve_env_root`] use it
    /// as `$HOME`, so the walk never leaves it and can't pick up a `.git` or
    /// Hermit root above the system temp dir.
    struct Tree(tempfile::TempDir);

    impl Tree {
        fn new() -> Self {
            Self(tempfile::tempdir().expect("tempdir"))
        }

        fn root(&self) -> &Path {
            self.0.path()
        }

        fn dir(&self, rel: &str) -> PathBuf {
            let path = self.root().join(rel);
            fs::create_dir_all(&path).expect("mkdir");
            path
        }

        fn file(&self, rel: &str) -> PathBuf {
            let path = self.root().join(rel);
            fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
            fs::write(&path, "").expect("write");
            path
        }

        fn resolve(&self, dir: &Path) -> PathBuf {
            resolve_env_root(dir, Some(self.root()))
        }
    }

    #[test]
    fn subdir_of_a_hermit_root_resolves_to_it() {
        let tree = Tree::new();
        let worktree = tree.dir("wt");
        tree.file("wt/.git");
        tree.file("wt/bin/activate-hermit");
        let sub = tree.dir("wt/apps/staged");

        assert_eq!(tree.resolve(&sub), worktree);
        assert_eq!(tree.resolve(&worktree), worktree);
    }

    #[test]
    fn hermit_root_inside_a_repo_wins_over_the_top_level() {
        let tree = Tree::new();
        let repo = tree.dir("repo");
        tree.dir("repo/.git");
        let app = tree.dir("repo/apps/foo");
        tree.file("repo/apps/foo/bin/activate-hermit");

        assert_eq!(tree.resolve(&tree.dir("repo/apps/foo/src")), app);
        assert_eq!(tree.resolve(&tree.dir("repo/apps/bar")), repo);
    }

    #[test]
    fn hermit_root_without_git_resolves_to_it() {
        let tree = Tree::new();
        let env = tree.dir("env");
        tree.file("env/bin/activate-hermit");

        assert_eq!(tree.resolve(&tree.dir("env/a/b")), env);
    }

    #[test]
    fn linked_worktree_with_a_git_file_resolves_to_its_top_level() {
        let tree = Tree::new();
        let worktree = tree.dir("wt");
        fs::write(
            worktree.join(".git"),
            "gitdir: /elsewhere/.git/worktrees/wt\n",
        )
        .unwrap();

        assert_eq!(tree.resolve(&tree.dir("wt/src/deep")), worktree);
    }

    #[test]
    fn clone_with_a_git_dir_resolves_to_its_top_level() {
        let tree = Tree::new();
        let clone = tree.dir("clone");
        tree.dir("clone/.git");

        assert_eq!(tree.resolve(&tree.dir("clone/sub")), clone);
    }

    #[test]
    fn bare_dir_is_its_own_key() {
        let tree = Tree::new();
        let dir = tree.dir("plain/nested");

        assert_eq!(tree.resolve(&dir), dir);
    }

    #[test]
    fn home_stays_its_own_key_even_as_a_repo() {
        let tree = Tree::new();
        let home = tree.root();
        // A dotfiles repo at `$HOME`.
        tree.dir(".git");
        tree.file("bin/activate-hermit");
        let notes = tree.dir("notes/2026");
        let repo = tree.dir("src/repo");
        tree.dir("src/repo/.git");

        assert_eq!(tree.resolve(home), home);
        assert_eq!(tree.resolve(&notes), notes);
        assert_eq!(tree.resolve(&tree.dir("src/repo/sub")), repo);
    }

    #[test]
    fn walk_does_not_climb_past_home() {
        let tree = Tree::new();
        tree.dir(".git");
        let home = tree.dir("home");
        let dir = tree.dir("home/projects");

        assert_eq!(resolve_env_root(&dir, Some(&home)), dir);
        assert_eq!(resolve_env_root(&home, Some(&home)), home);
    }

    #[test]
    fn worktrees_under_a_shared_hermit_parent_stay_distinct() {
        let tree = Tree::new();
        tree.file("parent/bin/activate-hermit");
        let first = tree.dir("parent/wt-a");
        tree.file("parent/wt-a/.git");
        let second = tree.dir("parent/wt-b");
        tree.file("parent/wt-b/.git");

        assert_eq!(tree.resolve(&tree.dir("parent/wt-a/apps")), first);
        assert_eq!(tree.resolve(&tree.dir("parent/wt-b/apps")), second);
    }

    #[test]
    fn every_marker_between_the_dir_and_its_root_forces_its_own_key() {
        for marker in DIR_ENV_MARKERS {
            let tree = Tree::new();
            tree.dir("wt/.git");
            tree.file("wt/bin/activate-hermit");
            let sub = tree.dir("wt/apps/staged");

            tree.file(&format!("wt/apps/{marker}"));
            assert_eq!(tree.resolve(&sub), sub, "{marker} in an ancestor");

            let tree = Tree::new();
            tree.dir("wt/.git");
            let sub = tree.dir("wt/apps/staged");
            tree.file(&format!("wt/apps/staged/{marker}"));
            assert_eq!(tree.resolve(&sub), sub, "{marker} in the dir itself");
        }
    }

    #[test]
    fn markers_at_the_root_keep_the_shared_key() {
        let tree = Tree::new();
        let worktree = tree.dir("wt");
        tree.dir("wt/.git");
        tree.file("wt/.envrc");
        tree.file("wt/.tool-versions");

        assert_eq!(tree.resolve(&tree.dir("wt/apps/staged")), worktree);
    }

    #[test]
    fn markers_without_a_root_leave_the_dir_its_own_key() {
        let tree = Tree::new();
        tree.file("proj/.envrc");
        let sub = tree.dir("proj/sub");

        assert_eq!(tree.resolve(&sub), sub);
    }

    #[test]
    fn relative_and_dotdot_paths_are_their_own_key() {
        let tree = Tree::new();
        tree.dir("repo/.git");
        let sub = tree.dir("repo/sub");
        let dotdot = sub.join("..").join("sub");

        assert_eq!(tree.resolve(Path::new("repo/sub")), Path::new("repo/sub"));
        assert_eq!(tree.resolve(&dotdot), dotdot);
    }
}
