//! Copy build directories between worktrees using filesystem cloning.
//!
//! On macOS (APFS), `cp -Rc` uses `clonefile()` under the hood so the copy
//! is instant and shares physical disk blocks until one side diverges (CoW).
//! On Linux, `cp -R --reflink=auto` achieves the same on Btrfs/XFS and
//! silently falls back to a regular copy on other filesystems.
//!
//! Only directories that are both in the hardcoded whitelist AND ignored by
//! git are copied, to avoid accidentally duplicating source directories.

use std::path::Path;
use std::process::Command;

/// Default whitelist of build directory names to copy between worktrees.
const BUILD_DIR_WHITELIST: &[&str] = &[
    // JS/TS
    "node_modules",
    ".next",
    ".turbo",
    ".svelte-kit",
    // Python
    "__pycache__",
    // Xcode
    "DerivedData",
    // General
    "build",
];

/// Maximum directory depth to walk when searching for build directories.
const MAX_WALK_DEPTH: usize = 5;

/// Recursively discover and copy whitelisted build directories from `source`
/// worktree into `dest` worktree.
///
/// Walks the entire worktree up to `MAX_WALK_DEPTH`, copying any directory
/// whose name is in `BUILD_DIR_WHITELIST` and is gitignored. Copied
/// directories are not recursed into (no need to find nested build dirs
/// inside build dirs).
///
/// This is best-effort: failures are logged but never propagated.
pub fn copy_build_dirs(source: &Path, dest: &Path) {
    if !source.is_dir() {
        log::debug!(
            "build_dir_copy: source root does not exist: {}",
            source.display()
        );
        return;
    }

    walk_and_copy(source, dest, source, dest, 0);
}

/// Recursively walk `current_src` looking for whitelisted build directories.
///
/// `source_root` and `dest_root` are the worktree roots, used to compute
/// relative paths for gitignore checks and destination paths.
fn walk_and_copy(
    source_root: &Path,
    dest_root: &Path,
    current_src: &Path,
    current_dest: &Path,
    depth: usize,
) {
    if depth > MAX_WALK_DEPTH {
        return;
    }

    let entries = match std::fs::read_dir(current_src) {
        Ok(entries) => entries,
        Err(e) => {
            log::debug!("build_dir_copy: cannot read {}: {e}", current_src.display());
            return;
        }
    };

    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };

        // Never descend into .git directories.
        if name_str == ".git" {
            continue;
        }

        let src_dir = current_src.join(name_str);
        let dest_dir = current_dest.join(name_str);

        if BUILD_DIR_WHITELIST.contains(&name_str) {
            // Compute path relative to worktree root for gitignore check.
            let rel_path = match src_dir.strip_prefix(source_root) {
                Ok(rel) => rel.to_string_lossy().to_string(),
                Err(_) => continue,
            };

            if !is_gitignored(dest_root, &rel_path) {
                log::debug!(
                    "build_dir_copy: skipping '{}' — not gitignored in dest",
                    rel_path
                );
                continue;
            }

            if dest_dir.exists() {
                log::debug!(
                    "build_dir_copy: skipping '{}' — already exists in dest",
                    rel_path
                );
                continue;
            }

            log::info!(
                "build_dir_copy: copying '{}' from {} to {}",
                rel_path,
                source_root.display(),
                dest_root.display()
            );

            if let Err(e) = clone_directory(&src_dir, &dest_dir) {
                log::warn!("build_dir_copy: failed to copy '{}': {e}", rel_path);
            }

            // Don't recurse into copied build dirs.
        } else {
            // Not a whitelisted name — recurse deeper.
            walk_and_copy(source_root, dest_root, &src_dir, &dest_dir, depth + 1);
        }
    }
}

/// Check if a path would be ignored by git in the given working directory.
fn is_gitignored(working_dir: &Path, rel_path: &str) -> bool {
    // `git check-ignore` exits 0 if the path is ignored, 1 if not.
    Command::new("git")
        .args(["check-ignore", "-q", rel_path])
        .current_dir(working_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Clone a directory using the platform-appropriate copy strategy.
fn clone_directory(src: &Path, dest: &Path) -> Result<(), String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("cp").args(["-Rc"]).arg(src).arg(dest).output()
    } else {
        // Linux: --reflink=auto gives CoW on Btrfs/XFS, regular copy elsewhere.
        Command::new("cp")
            .args(["-R", "--reflink=auto"])
            .arg(src)
            .arg(dest)
            .output()
    };

    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).to_string()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_build_dir_whitelist_is_not_empty() {
        assert!(!BUILD_DIR_WHITELIST.is_empty());
    }

    #[test]
    fn test_clone_directory_copies_files() {
        let tmp = std::env::temp_dir().join(format!("staged-bdc-test-{}", std::process::id()));
        let src = tmp.join("src");
        let dest = tmp.join("dest");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/file.txt"), "hello").unwrap();

        let result = clone_directory(&src, &dest);
        assert!(result.is_ok(), "clone_directory failed: {:?}", result);
        assert_eq!(
            fs::read_to_string(dest.join("sub/file.txt")).unwrap(),
            "hello"
        );

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_max_walk_depth_is_reasonable() {
        assert!(MAX_WALK_DEPTH >= 4);
        assert!(MAX_WALK_DEPTH <= 10);
    }
}
