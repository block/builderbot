//! Staging operations (stage, unstage, discard)

use super::repo::find_repo;
use super::GitError;
use git2::IndexAddOption;
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
