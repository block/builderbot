use super::cli::{self, GitError};
use super::refs;
use super::types::*;
use git2::{DiffOptions, Repository};
use std::cell::RefCell;
use std::path::Path;

/// Resolve a GitRef, converting MergeBase to a concrete SHA.
fn resolve_ref(repo: &Path, git_ref: &GitRef) -> Result<GitRef, GitError> {
    match git_ref {
        GitRef::MergeBase => {
            let default_branch = refs::detect_default_branch(repo)?;
            let sha = refs::merge_base(repo, &default_branch, "HEAD")?;
            Ok(GitRef::Rev(sha))
        }
        other => Ok(other.clone()),
    }
}

/// Resolve a DiffSpec, converting any MergeBase refs to concrete SHAs.
fn resolve_spec(repo: &Path, spec: &DiffSpec) -> Result<DiffSpec, GitError> {
    Ok(DiffSpec {
        base: resolve_ref(repo, &spec.base)?,
        head: resolve_ref(repo, &spec.head)?,
    })
}

/// A hunk from git diff (0-indexed line numbers)
#[derive(Debug, Clone, Copy)]
struct Hunk {
    /// Start line in old file (0-indexed)
    old_start: u32,
    /// Number of lines in old file
    old_lines: u32,
    /// Start line in new file (0-indexed)
    new_start: u32,
    /// Number of lines in new file
    new_lines: u32,
}

/// List files changed in a diff (for sidebar)
///
/// For working tree diffs: uses `git status --porcelain -z` which leverages fsmonitor
/// for fast performance on large repos.
///
/// For commit..commit diffs: uses `git diff --name-status -z` since status doesn't
/// support arbitrary commit ranges.
pub fn list_diff_files(repo: &Path, spec: &DiffSpec) -> Result<Vec<FileDiffSummary>, GitError> {
    // Resolve MergeBase to concrete SHA
    let spec = resolve_spec(repo, spec)?;

    match (&spec.base, &spec.head) {
        (GitRef::Rev(base), GitRef::WorkingTree) => {
            // Working tree diff - use git status for fsmonitor support
            list_working_tree_changes(repo, base)
        }
        (GitRef::Rev(base), GitRef::Rev(head)) => {
            // Commit range - use git diff
            let args = ["diff", "--name-status", "-z", base.as_str(), head.as_str()];
            let output = cli::run(repo, &args)?;
            parse_name_status(&output)
        }
        (GitRef::WorkingTree, _) => Err(GitError::CommandFailed(
            "Cannot use working tree as base".to_string(),
        )),
        (GitRef::MergeBase, _) | (_, GitRef::MergeBase) => {
            unreachable!("MergeBase should have been resolved")
        }
    }
}

/// List working tree changes using `git status --porcelain -z`.
/// This uses fsmonitor when available, making it fast on large repos.
///
/// When base is HEAD, we show all uncommitted changes (staged + unstaged + untracked).
/// When base is another ref, we show what would change if you committed now and compared to that ref.
fn list_working_tree_changes(repo: &Path, base: &str) -> Result<Vec<FileDiffSummary>, GitError> {
    // Get status (includes staged, unstaged, and untracked)
    let output = cli::run(repo, &["status", "--porcelain", "-z"])?;
    let status_files = parse_porcelain_status(repo, &output)?;

    // If base is HEAD, status gives us exactly what we need
    if base == "HEAD" {
        return Ok(status_files);
    }

    // For other bases (e.g., main), we need to combine:
    // 1. Files changed between base and HEAD (committed changes)
    // 2. Files with uncommitted changes (from status)
    let diff_output = cli::run(repo, &["diff", "--name-status", "-z", base, "HEAD"])?;
    let committed_files = parse_name_status(&diff_output)?;

    // Merge: status files take precedence (they reflect current working tree state)
    let mut result_map = std::collections::HashMap::new();

    // Add committed changes first
    for file in committed_files {
        let path = file.after.clone().or(file.before.clone()).unwrap();
        result_map.insert(path, file);
    }

    // Override with status (current state)
    for file in status_files {
        let path = file.after.clone().or(file.before.clone()).unwrap();
        result_map.insert(path, file);
    }

    Ok(result_map.into_values().collect())
}

/// Parse `git status --porcelain -z` output.
/// Format: XY PATH\0 (or XY OLD\0NEW\0 for renames)
/// X = index status, Y = worktree status
///
/// Note: git status reports untracked directories as just the directory name (with trailing slash).
/// We expand these into individual files using `git ls-files --others`.
fn parse_porcelain_status(repo: &Path, output: &str) -> Result<Vec<FileDiffSummary>, GitError> {
    let mut results = Vec::new();
    let mut chars = output.chars().peekable();

    while chars.peek().is_some() {
        // Read XY status (2 chars)
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');

        // Skip the space after XY
        if chars.peek() == Some(&' ') {
            chars.next();
        }

        // Read path until null
        let path: String = chars.by_ref().take_while(|&c| c != '\0').collect();
        if path.is_empty() {
            continue;
        }

        // Handle renames/copies - they have a second path
        let (old_path, new_path) = if x == 'R' || x == 'C' {
            let new: String = chars.by_ref().take_while(|&c| c != '\0').collect();
            (Some(path), if new.is_empty() { None } else { Some(new) })
        } else {
            (None, Some(path))
        };

        // Determine file status from XY
        // We care about the combined effect: is the file added, deleted, modified, or renamed?
        match (x, y) {
            ('?', '?') => {
                // Untracked: could be a file or directory
                // git status reports directories with trailing slash
                if let Some(ref p) = new_path {
                    if p.ends_with('/') {
                        // It's a directory - expand into individual files
                        let files = expand_untracked_dir(repo, p)?;
                        for file in files {
                            results.push(FileDiffSummary {
                                before: None,
                                after: Some(file.into()),
                            });
                        }
                    } else {
                        results.push(FileDiffSummary {
                            before: None,
                            after: Some(p.clone().into()),
                        });
                    }
                }
            }
            ('A', _) | (_, 'A') => {
                results.push(FileDiffSummary {
                    before: None,
                    after: new_path.map(Into::into),
                });
            }
            ('D', _) | (_, 'D') => {
                results.push(FileDiffSummary {
                    before: new_path.map(Into::into),
                    after: None,
                });
            }
            ('R', _) | ('C', _) => {
                results.push(FileDiffSummary {
                    before: old_path.map(Into::into),
                    after: new_path.map(Into::into),
                });
            }
            _ => {
                results.push(FileDiffSummary {
                    before: new_path.clone().map(Into::into),
                    after: new_path.map(Into::into),
                });
            }
        };
    }

    Ok(results)
}

/// Expand an untracked directory into its individual files.
/// Uses `git ls-files --others --exclude-standard` to list untracked files.
fn expand_untracked_dir(repo: &Path, dir: &str) -> Result<Vec<String>, GitError> {
    let output = cli::run(
        repo,
        &["ls-files", "--others", "--exclude-standard", "-z", dir],
    )?;

    Ok(output
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

/// Parse `git diff --name-status -z` output
/// Format: STATUS\0PATH\0 (or STATUS\0OLD\0NEW\0 for renames)
fn parse_name_status(output: &str) -> Result<Vec<FileDiffSummary>, GitError> {
    let mut results = Vec::new();
    let mut parts = output.split('\0').peekable();

    while let Some(status) = parts.next() {
        if status.is_empty() {
            continue;
        }

        let status_char = status.chars().next().unwrap_or(' ');

        match status_char {
            'A' => {
                // Added: just one path
                if let Some(path) = parts.next() {
                    results.push(FileDiffSummary {
                        before: None,
                        after: Some(path.into()),
                    });
                }
            }
            'D' => {
                // Deleted: just one path
                if let Some(path) = parts.next() {
                    results.push(FileDiffSummary {
                        before: Some(path.into()),
                        after: None,
                    });
                }
            }
            'M' | 'T' => {
                // Modified or Type changed: just one path
                if let Some(path) = parts.next() {
                    results.push(FileDiffSummary {
                        before: Some(path.into()),
                        after: Some(path.into()),
                    });
                }
            }
            'R' | 'C' => {
                // Renamed or Copied: two paths (old, new)
                // Status might include similarity percentage like R100
                if let (Some(old), Some(new)) = (parts.next(), parts.next()) {
                    results.push(FileDiffSummary {
                        before: Some(old.into()),
                        after: Some(new.into()),
                    });
                }
            }
            _ => {
                // Unknown status, skip the path
                parts.next();
            }
        }
    }

    Ok(results)
}

/// Get full diff content for a single file using libgit2.
/// This is reliable and battle-tested - we use git CLI only for list_diff_files
/// where fsmonitor support matters for performance.
pub fn get_file_diff(repo_path: &Path, spec: &DiffSpec, path: &Path) -> Result<FileDiff, GitError> {
    // Resolve MergeBase to concrete SHA
    let spec = resolve_spec(repo_path, spec)?;

    let repo = Repository::discover(repo_path).map_err(|e| GitError::NotARepo(e.to_string()))?;

    // Resolve trees
    let base_tree = resolve_to_tree(&repo, &spec.base)?;
    let head_tree = resolve_to_tree(&repo, &spec.head)?;
    let is_working_tree = matches!(spec.head, GitRef::WorkingTree);

    // Load file content
    let before = load_file_from_tree(&repo, base_tree.as_ref(), path)?;
    let after = if is_working_tree {
        load_file_from_workdir(&repo, path)?
    } else {
        load_file_from_tree(&repo, head_tree.as_ref(), path)?
    };

    // Get hunks via libgit2
    let hunks = get_hunks_libgit2(
        &repo,
        base_tree.as_ref(),
        head_tree.as_ref(),
        is_working_tree,
        path,
    )?;

    // Compute alignments from hunks
    let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

    Ok(FileDiff {
        before,
        after,
        alignments,
    })
}

/// Resolve a GitRef to a tree (or None for working tree)
/// Note: MergeBase should already be resolved before calling this
fn resolve_to_tree<'a>(
    repo: &'a Repository,
    git_ref: &GitRef,
) -> Result<Option<git2::Tree<'a>>, GitError> {
    match git_ref {
        GitRef::WorkingTree => Ok(None),
        GitRef::Rev(rev) => {
            let obj = repo
                .revparse_single(rev)
                .map_err(|e| GitError::CommandFailed(format!("Cannot resolve '{}': {}", rev, e)))?;
            let tree = obj.peel_to_tree().map_err(|e| {
                GitError::CommandFailed(format!("Cannot get tree for '{}': {}", rev, e))
            })?;
            Ok(Some(tree))
        }
        GitRef::MergeBase => {
            unreachable!("MergeBase should have been resolved before calling resolve_to_tree")
        }
    }
}

/// Load file content from a git tree
fn load_file_from_tree(
    repo: &Repository,
    tree: Option<&git2::Tree>,
    path: &Path,
) -> Result<Option<File>, GitError> {
    let tree = match tree {
        Some(t) => t,
        None => return Ok(None),
    };

    let entry = match tree.get_path(path) {
        Ok(e) => e,
        Err(_) => return Ok(None), // File doesn't exist in this tree
    };

    let obj = entry
        .to_object(repo)
        .map_err(|e| GitError::CommandFailed(format!("Cannot load object: {}", e)))?;

    let blob = match obj.as_blob() {
        Some(b) => b,
        None => return Ok(None), // Not a file (maybe a submodule)
    };

    let content = bytes_to_content(blob.content());

    Ok(Some(File {
        path: path.to_string_lossy().to_string(),
        content,
    }))
}

/// Load file content from the working directory
fn load_file_from_workdir(repo: &Repository, path: &Path) -> Result<Option<File>, GitError> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| GitError::CommandFailed("Bare repository".into()))?;
    let full_path = workdir.join(path);

    if !full_path.exists() {
        return Ok(None);
    }

    // Skip directories (e.g., submodules)
    if full_path.is_dir() {
        return Ok(None);
    }

    let bytes = std::fs::read(&full_path)
        .map_err(|e| GitError::CommandFailed(format!("Cannot read file: {}", e)))?;

    Ok(Some(File {
        path: path.to_string_lossy().to_string(),
        content: bytes_to_content(&bytes),
    }))
}

/// Convert raw bytes to FileContent, detecting binary
fn bytes_to_content(bytes: &[u8]) -> FileContent {
    // Check for binary: look for null bytes in first 8KB
    let check_len = bytes.len().min(8192);
    if bytes[..check_len].contains(&0) {
        return FileContent::Binary;
    }

    // Parse as UTF-8 (lossy for display)
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    FileContent::Text { lines }
}

/// Get hunks for a single file using libgit2
fn get_hunks_libgit2(
    repo: &Repository,
    base_tree: Option<&git2::Tree>,
    head_tree: Option<&git2::Tree>,
    is_working_tree: bool,
    path: &Path,
) -> Result<Vec<Hunk>, GitError> {
    let mut opts = DiffOptions::new();
    opts.context_lines(0); // No context, just the changes
    opts.pathspec(path);

    let diff = if is_working_tree {
        repo.diff_tree_to_workdir_with_index(base_tree, Some(&mut opts))
    } else {
        repo.diff_tree_to_tree(base_tree, head_tree, Some(&mut opts))
    }
    .map_err(|e| GitError::CommandFailed(format!("Failed to compute diff: {}", e)))?;

    // Collect hunks
    let hunks: RefCell<Vec<Hunk>> = RefCell::new(Vec::new());

    diff.foreach(
        &mut |_delta, _progress| true, // file callback
        None,                          // binary callback
        Some(&mut |_delta, hunk| {
            // Git uses 1-indexed line numbers, convert to 0-indexed
            let old_start = if hunk.old_start() == 0 {
                0
            } else {
                hunk.old_start() - 1
            };
            let new_start = if hunk.new_start() == 0 {
                0
            } else {
                hunk.new_start() - 1
            };

            hunks.borrow_mut().push(Hunk {
                old_start,
                old_lines: hunk.old_lines(),
                new_start,
                new_lines: hunk.new_lines(),
            });
            true
        }),
        None, // line callback
    )
    .map_err(|e| GitError::CommandFailed(format!("Failed to iterate diff: {}", e)))?;

    Ok(hunks.into_inner())
}

/// Compute alignments from git hunks.
/// This uses git's authoritative diff output rather than recomputing.
fn compute_alignments_from_hunks(
    hunks: &[Hunk],
    before: &Option<File>,
    after: &Option<File>,
) -> Vec<Alignment> {
    let before_len = match before {
        Some(File {
            content: FileContent::Text { lines },
            ..
        }) => lines.len() as u32,
        _ => 0,
    };
    let after_len = match after {
        Some(File {
            content: FileContent::Text { lines },
            ..
        }) => lines.len() as u32,
        _ => 0,
    };

    // Handle empty files
    if before_len == 0 && after_len == 0 {
        return vec![];
    }

    // If no hunks but files exist, it's either all added or all deleted
    if hunks.is_empty() {
        if before_len == 0 {
            // All added
            return vec![Alignment {
                before: Span::new(0, 0),
                after: Span::new(0, after_len),
                changed: true,
            }];
        } else if after_len == 0 {
            // All deleted
            return vec![Alignment {
                before: Span::new(0, before_len),
                after: Span::new(0, 0),
                changed: true,
            }];
        } else {
            // No changes (shouldn't happen for files in a diff, but handle gracefully)
            return vec![Alignment {
                before: Span::new(0, before_len),
                after: Span::new(0, after_len),
                changed: false,
            }];
        }
    }

    let mut alignments = Vec::new();
    let mut before_pos = 0u32;
    let mut after_pos = 0u32;

    for hunk in hunks {
        // Unchanged region before this hunk
        if before_pos < hunk.old_start || after_pos < hunk.new_start {
            // The gap should be the same size on both sides for unchanged content
            let before_gap = hunk.old_start - before_pos;
            let after_gap = hunk.new_start - after_pos;

            // They should match for truly unchanged content, but handle edge cases
            if before_gap > 0 || after_gap > 0 {
                alignments.push(Alignment {
                    before: Span::new(before_pos, hunk.old_start),
                    after: Span::new(after_pos, hunk.new_start),
                    changed: false,
                });
            }
        }

        // The hunk itself (changed region)
        let hunk_before_end = hunk.old_start + hunk.old_lines;
        let hunk_after_end = hunk.new_start + hunk.new_lines;

        alignments.push(Alignment {
            before: Span::new(hunk.old_start, hunk_before_end),
            after: Span::new(hunk.new_start, hunk_after_end),
            changed: true,
        });

        before_pos = hunk_before_end;
        after_pos = hunk_after_end;
    }

    // Unchanged region after the last hunk
    if before_pos < before_len || after_pos < after_len {
        alignments.push(Alignment {
            before: Span::new(before_pos, before_len),
            after: Span::new(after_pos, after_len),
            changed: false,
        });
    }

    alignments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_name_status_added() {
        let output = "A\0new_file.txt\0";
        let result = parse_name_status(output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_added());
        assert_eq!(
            result[0].after.as_ref().unwrap().to_str(),
            Some("new_file.txt")
        );
    }

    #[test]
    fn test_parse_name_status_deleted() {
        let output = "D\0old_file.txt\0";
        let result = parse_name_status(output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_deleted());
    }

    #[test]
    fn test_parse_name_status_modified() {
        let output = "M\0changed.txt\0";
        let result = parse_name_status(output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_added());
        assert!(!result[0].is_deleted());
        assert!(!result[0].is_renamed());
    }

    #[test]
    fn test_parse_name_status_renamed() {
        let output = "R100\0old_name.txt\0new_name.txt\0";
        let result = parse_name_status(output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_renamed());
        assert_eq!(
            result[0].before.as_ref().unwrap().to_str(),
            Some("old_name.txt")
        );
        assert_eq!(
            result[0].after.as_ref().unwrap().to_str(),
            Some("new_name.txt")
        );
    }

    #[test]
    fn test_parse_name_status_multiple() {
        let output = "A\0added.txt\0M\0modified.txt\0D\0deleted.txt\0";
        let result = parse_name_status(output).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_parse_porcelain_untracked() {
        let dir = tempfile::tempdir().unwrap();
        let output = "?? untracked.txt\0";
        let result = parse_porcelain_status(dir.path(), output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_added());
        assert_eq!(
            result[0].after.as_ref().unwrap().to_str(),
            Some("untracked.txt")
        );
    }

    #[test]
    fn test_parse_porcelain_modified() {
        let dir = tempfile::tempdir().unwrap();
        let output = " M modified.txt\0";
        let result = parse_porcelain_status(dir.path(), output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_added());
        assert!(!result[0].is_deleted());
    }

    #[test]
    fn test_parse_porcelain_staged() {
        let dir = tempfile::tempdir().unwrap();
        let output = "M  staged.txt\0";
        let result = parse_porcelain_status(dir.path(), output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_added());
        assert!(!result[0].is_deleted());
    }

    #[test]
    fn test_parse_porcelain_added() {
        let dir = tempfile::tempdir().unwrap();
        let output = "A  new_file.txt\0";
        let result = parse_porcelain_status(dir.path(), output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_added());
    }

    #[test]
    fn test_parse_porcelain_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let output = " D deleted.txt\0";
        let result = parse_porcelain_status(dir.path(), output).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_deleted());
    }

    #[test]
    fn test_parse_porcelain_multiple() {
        let dir = tempfile::tempdir().unwrap();
        let output = "?? untracked.txt\0 M modified.txt\0A  added.txt\0";
        let result = parse_porcelain_status(dir.path(), output).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_parse_porcelain_untracked_directory() {
        // Create a temp git repo with an untracked directory
        let dir = tempfile::tempdir().unwrap();
        let repo_path = dir.path();

        // Initialize git repo
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // Create untracked directory with files
        let newdir = repo_path.join("newdir");
        std::fs::create_dir_all(newdir.join("subdir")).unwrap();
        std::fs::write(newdir.join("file1.txt"), "content1").unwrap();
        std::fs::write(newdir.join("subdir").join("file2.txt"), "content2").unwrap();

        // Parse status output that reports the directory (with trailing slash)
        let output = "?? newdir/\0";
        let result = parse_porcelain_status(repo_path, output).unwrap();

        // Should expand to 2 files
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|f| f.is_added()));

        let paths: Vec<_> = result
            .iter()
            .map(|f| f.after.as_ref().unwrap().to_str().unwrap())
            .collect();
        assert!(paths.contains(&"newdir/file1.txt"));
        assert!(paths.contains(&"newdir/subdir/file2.txt"));
    }
}
