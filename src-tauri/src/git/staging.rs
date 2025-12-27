//! Staging operations (stage, unstage, discard)
//!
//! Supports both file-level and line-level operations.
//! Line-level operations work by reconstructing file content with specific
//! lines reverted, rather than using git's hunk-based apply API.

use super::repo::find_repo;
use super::GitError;
use git2::{IndexAddOption, Repository};
use std::path::Path;

/// Stage a file (add to index)
pub fn stage_file(repo_path: Option<&str>, file_path: &str) -> Result<(), GitError> {
    let repo = find_repo(repo_path)?;
    let mut index = repo.index()?;

    // Check if file exists in working directory
    let workdir = repo.workdir().ok_or_else(|| GitError {
        message: "Repository has no working directory".to_string(),
    })?;

    let full_path = workdir.join(file_path);

    if full_path.exists() {
        // File exists - add it to index
        index.add_path(Path::new(file_path))?;
    } else {
        // File was deleted - remove from index
        index.remove_path(Path::new(file_path))?;
    }

    index.write()?;
    Ok(())
}

/// Unstage a file (remove from index, restore to HEAD state)
pub fn unstage_file(repo_path: Option<&str>, file_path: &str) -> Result<(), GitError> {
    let repo = find_repo(repo_path)?;

    // Get HEAD commit
    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    match head {
        Some(commit) => {
            // Reset the file in index to match HEAD
            repo.reset_default(Some(&commit.into_object()), [file_path])?;
        }
        None => {
            // No HEAD (initial commit) - remove from index entirely
            let mut index = repo.index()?;
            index.remove_path(Path::new(file_path))?;
            index.write()?;
        }
    }

    Ok(())
}

/// Discard changes in working directory (restore file to index state)
pub fn discard_file(repo_path: Option<&str>, file_path: &str) -> Result<(), GitError> {
    let repo = find_repo(repo_path)?;
    let workdir = repo.workdir().ok_or_else(|| GitError {
        message: "Repository has no working directory".to_string(),
    })?;

    // Get the file from the index
    let index = repo.index()?;
    let entry = index.get_path(Path::new(file_path), 0);

    match entry {
        Some(entry) => {
            // File exists in index - restore it from index
            let blob = repo.find_blob(entry.id)?;
            let content = blob.content();
            let full_path = workdir.join(file_path);

            // Create parent directories if needed
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| GitError {
                    message: format!("Failed to create directories: {}", e),
                })?;
            }

            std::fs::write(&full_path, content).map_err(|e| GitError {
                message: format!("Failed to write file: {}", e),
            })?;

            // Also need to update the file's mode/permissions if needed
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = entry.mode;
                if mode & 0o111 != 0 {
                    // File should be executable
                    let mut perms = std::fs::metadata(&full_path)
                        .map_err(|e| GitError {
                            message: format!("Failed to get metadata: {}", e),
                        })?
                        .permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&full_path, perms).map_err(|e| GitError {
                        message: format!("Failed to set permissions: {}", e),
                    })?;
                }
            }
        }
        None => {
            // File not in index - it's untracked, delete it
            let full_path = workdir.join(file_path);
            if full_path.exists() {
                if full_path.is_dir() {
                    std::fs::remove_dir_all(&full_path).map_err(|e| GitError {
                        message: format!("Failed to delete directory: {}", e),
                    })?;
                } else {
                    std::fs::remove_file(&full_path).map_err(|e| GitError {
                        message: format!("Failed to delete file: {}", e),
                    })?;
                }
            }
        }
    }

    Ok(())
}

/// Stage all files
pub fn stage_all(repo_path: Option<&str>) -> Result<(), GitError> {
    let repo = find_repo(repo_path)?;
    let mut index = repo.index()?;

    index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
    index.write()?;

    Ok(())
}

/// Unstage all files
pub fn unstage_all(repo_path: Option<&str>) -> Result<(), GitError> {
    let repo = find_repo(repo_path)?;

    let head = repo.head().ok().and_then(|h| h.peel_to_commit().ok());

    match head {
        Some(commit) => {
            // Reset index to HEAD
            repo.reset_default(Some(&commit.into_object()), ["*"])?;
        }
        None => {
            // No HEAD - clear the index
            let mut index = repo.index()?;
            index.clear()?;
            index.write()?;
        }
    }

    Ok(())
}

// =============================================================================
// Line-level operations
// =============================================================================

/// Line range for a change to discard.
/// Line numbers are 1-indexed and inclusive.
#[derive(Debug, Clone)]
pub struct DiscardRange {
    /// Lines to remove from the "old" file (before state)
    /// None if this is a pure addition
    pub old_start: Option<u32>,
    pub old_end: Option<u32>,
    /// Lines to remove from the "new" file (after state)
    /// None if this is a pure deletion
    pub new_start: Option<u32>,
    pub new_end: Option<u32>,
}

/// Discard specific lines from a file.
///
/// This operates at the line level rather than git hunk level, allowing
/// fine-grained control over which changes to discard.
///
/// For unstaged changes: reverts the specified lines in the working directory
/// to match the index.
///
/// For staged changes: reverts the specified lines in the index to match HEAD,
/// and also reverts the working directory if those lines exist there.
pub fn discard_lines(
    repo_path: Option<&str>,
    file_path: &str,
    range: DiscardRange,
    staged: bool,
) -> Result<(), GitError> {
    let repo = find_repo(repo_path)?;
    let workdir = repo.workdir().ok_or_else(|| GitError {
        message: "Repository has no working directory".to_string(),
    })?;

    if staged {
        discard_lines_staged(&repo, workdir, file_path, &range)
    } else {
        discard_lines_unstaged(&repo, workdir, file_path, &range)
    }
}

/// Discard unstaged lines: revert working directory to index state for specific lines.
fn discard_lines_unstaged(
    repo: &Repository,
    workdir: &std::path::Path,
    file_path: &str,
    range: &DiscardRange,
) -> Result<(), GitError> {
    let full_path = workdir.join(file_path);

    // Get index content (the "before" state for unstaged changes)
    let index_content = get_content_from_index(repo, file_path)?;

    // Get working directory content (the "after" state)
    let workdir_content = std::fs::read_to_string(&full_path).ok();

    // Reconstruct the file with the specified lines reverted
    let new_content =
        apply_line_revert(index_content.as_deref(), workdir_content.as_deref(), range)?;

    // Write the result
    match new_content {
        Some(content) => {
            std::fs::write(&full_path, content).map_err(|e| GitError {
                message: format!("Failed to write file: {}", e),
            })?;
        }
        None => {
            // File should be deleted
            if full_path.exists() {
                std::fs::remove_file(&full_path).map_err(|e| GitError {
                    message: format!("Failed to delete file: {}", e),
                })?;
            }
        }
    }

    Ok(())
}

/// Discard staged lines: revert index to HEAD state for specific lines.
fn discard_lines_staged(
    repo: &Repository,
    workdir: &std::path::Path,
    file_path: &str,
    range: &DiscardRange,
) -> Result<(), GitError> {
    // Get HEAD content (the "before" state for staged changes)
    let head_content = get_content_from_head(repo, file_path)?;

    // Get index content (the "after" state for staged changes)
    let index_content = get_content_from_index(repo, file_path)?;

    // Reconstruct the index content with the specified lines reverted
    let new_index_content =
        apply_line_revert(head_content.as_deref(), index_content.as_deref(), range)?;

    // Update the index with the new content
    match new_index_content {
        Some(content) => {
            // Write to a temp blob and update index
            let blob_oid = repo.blob(content.as_bytes())?;
            let mut index = repo.index()?;

            // Get the existing entry to preserve mode, or use default
            let mode = index
                .get_path(Path::new(file_path), 0)
                .map(|e| e.mode)
                .unwrap_or(0o100644);

            let entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode,
                uid: 0,
                gid: 0,
                file_size: content.len() as u32,
                id: blob_oid,
                flags: 0,
                flags_extended: 0,
                path: file_path.as_bytes().to_vec(),
            };

            index.add(&entry)?;
            index.write()?;
        }
        None => {
            // Remove from index
            let mut index = repo.index()?;
            index.remove_path(Path::new(file_path))?;
            index.write()?;
        }
    }

    // Also update workdir if the file exists there
    let full_path = workdir.join(file_path);
    if full_path.exists() {
        let workdir_content = std::fs::read_to_string(&full_path).ok();

        // For workdir, we want to revert the same lines
        // But the workdir might have additional unstaged changes
        // For simplicity, we apply the same revert to workdir
        if let Some(ref wc) = workdir_content {
            let new_workdir_content = apply_line_revert(head_content.as_deref(), Some(wc), range)?;

            if let Some(content) = new_workdir_content {
                std::fs::write(&full_path, content).map_err(|e| GitError {
                    message: format!("Failed to write file: {}", e),
                })?;
            }
        }
    }

    Ok(())
}

/// Apply a line-level revert operation.
///
/// Takes the "before" content, "after" content, and a range of lines to revert.
/// Returns the new content with those lines reverted to the "before" state.
///
/// The algorithm:
/// - For lines being removed (old_start..old_end): these were deleted, restore them
/// - For lines being added (new_start..new_end): these were added, remove them
fn apply_line_revert(
    before_content: Option<&str>,
    after_content: Option<&str>,
    range: &DiscardRange,
) -> Result<Option<String>, GitError> {
    let before_lines: Vec<&str> = before_content
        .map(|s| s.lines().collect())
        .unwrap_or_default();
    let after_lines: Vec<&str> = after_content
        .map(|s| s.lines().collect())
        .unwrap_or_default();

    // Convert to 0-indexed
    let old_start = range.old_start.map(|n| (n - 1) as usize);
    let old_end = range.old_end.map(|n| n as usize); // exclusive
    let new_start = range.new_start.map(|n| (n - 1) as usize);
    let new_end = range.new_end.map(|n| n as usize); // exclusive

    let mut result: Vec<&str> = Vec::new();

    // Add lines before the change
    if let Some(ns) = new_start {
        result.extend(&after_lines[..ns]);
    }

    // Add the "before" lines (restoring deleted lines)
    if let (Some(os), Some(oe)) = (old_start, old_end) {
        if os < before_lines.len() {
            let end = oe.min(before_lines.len());
            result.extend(&before_lines[os..end]);
        }
    }

    // Add lines after the change
    if let Some(ne) = new_end {
        if ne < after_lines.len() {
            result.extend(&after_lines[ne..]);
        }
    } else if new_start.is_none() {
        // Pure deletion case - keep all after_lines and insert before_lines
        // This case is trickier - we need to figure out where to insert
        // For now, if there's no new_start/new_end, it means we're restoring
        // deleted lines. We need to find where they go.

        // If old_start is set, insert at that position
        if let (Some(os), Some(oe)) = (old_start, old_end) {
            result.clear();
            // Lines before the deletion point
            let insert_point = os.min(after_lines.len());
            result.extend(&after_lines[..insert_point]);
            // The restored lines
            if os < before_lines.len() {
                let end = oe.min(before_lines.len());
                result.extend(&before_lines[os..end]);
            }
            // Lines after the deletion point
            result.extend(&after_lines[insert_point..]);
        }
    }

    if result.is_empty() && before_lines.is_empty() {
        return Ok(None);
    }

    // Preserve trailing newline if original had one
    let had_trailing_newline = after_content
        .map(|s| s.ends_with('\n'))
        .unwrap_or(before_content.map(|s| s.ends_with('\n')).unwrap_or(false));

    let mut output = result.join("\n");
    if had_trailing_newline || !output.is_empty() {
        output.push('\n');
    }

    Ok(Some(output))
}

// =============================================================================
// Content helpers
// =============================================================================

fn get_content_from_head(repo: &Repository, file_path: &str) -> Result<Option<String>, GitError> {
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };
    let tree = head.peel_to_tree().map_err(|e| GitError {
        message: format!("Failed to get HEAD tree: {}", e),
    })?;
    let entry = match tree.get_path(Path::new(file_path)) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };
    let blob = repo.find_blob(entry.id()).map_err(|e| GitError {
        message: format!("Failed to get blob: {}", e),
    })?;
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}

fn get_content_from_index(repo: &Repository, file_path: &str) -> Result<Option<String>, GitError> {
    let index = repo.index().map_err(|e| GitError {
        message: format!("Failed to get index: {}", e),
    })?;
    let entry = match index.get_path(Path::new(file_path), 0) {
        Some(e) => e,
        None => return Ok(None),
    };
    let blob = repo.find_blob(entry.id).map_err(|e| GitError {
        message: format!("Failed to get blob: {}", e),
    })?;
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}
