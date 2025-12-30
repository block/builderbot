//! Git operations for computing diffs.
//!
//! All functions are stateless - they discover the repo fresh each call.

use std::collections::HashMap;
use std::path::Path;

use git2::{Delta, DiffOptions, Repository, Tree};
use serde::{Deserialize, Serialize};

use super::types::{Alignment, File, FileContent, FileDiff, Span};

/// Error type for git operations.
#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitError {}

impl From<git2::Error> for GitError {
    fn from(e: git2::Error) -> Self {
        GitError(e.message().to_string())
    }
}

type Result<T> = std::result::Result<T, GitError>;

/// A git reference with its type for display purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRef {
    pub name: String,
    pub ref_type: RefType,
}

/// The type of a git reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefType {
    Branch,
    Tag,
    Special,
}

/// Open the repository containing the given path.
pub fn open_repo(path: &Path) -> Result<Repository> {
    Repository::discover(path).map_err(Into::into)
}

/// Get all refs with type information for autocomplete UI.
///
/// Includes special refs (@, HEAD, HEAD~1), local branches, and tags.
pub fn get_refs(repo: &Repository) -> Result<Vec<GitRef>> {
    let mut refs = Vec::new();

    // Special refs first (most commonly used)
    refs.push(GitRef {
        name: "@".to_string(),
        ref_type: RefType::Special,
    });
    refs.push(GitRef {
        name: "HEAD".to_string(),
        ref_type: RefType::Special,
    });
    refs.push(GitRef {
        name: "HEAD~1".to_string(),
        ref_type: RefType::Special,
    });

    // Local branches
    for branch in repo.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = branch?;
        if let Some(name) = branch.name()? {
            refs.push(GitRef {
                name: name.to_string(),
                ref_type: RefType::Branch,
            });
        }
    }

    // Tags
    repo.tag_foreach(|_oid, name| {
        if let Ok(name) = std::str::from_utf8(name) {
            let name = name.strip_prefix("refs/tags/").unwrap_or(name);
            refs.push(GitRef {
                name: name.to_string(),
                ref_type: RefType::Tag,
            });
        }
        true
    })?;

    Ok(refs)
}

/// Resolve a ref to a short SHA for display, or validate it exists.
///
/// Returns "working tree" for "@", otherwise returns the short (8-char) SHA.
pub fn resolve_ref(repo: &Repository, ref_str: &str) -> Result<String> {
    if ref_str == "@" {
        return Ok("working tree".to_string());
    }

    let obj = repo
        .revparse_single(ref_str)
        .map_err(|e| GitError(format!("Cannot resolve '{}': {}", ref_str, e)))?;

    // Return short SHA (first 8 characters)
    let full_sha = obj.id().to_string();
    Ok(full_sha[..8.min(full_sha.len())].to_string())
}

/// Get the current branch name.
pub fn current_branch(repo: &Repository) -> Result<Option<String>> {
    match repo.head() {
        Ok(head) if head.is_branch() => Ok(head.shorthand().map(String::from)),
        Ok(_) => Ok(None),  // Detached HEAD
        Err(_) => Ok(None), // No commits yet
    }
}

/// Basic repository info needed by the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    /// Absolute path to the repository root.
    pub repo_path: String,
    /// Current branch name, if on a branch.
    pub branch: Option<String>,
}

/// Get basic repository info (path and branch).
pub fn get_repo_info(repo: &Repository) -> Result<RepoInfo> {
    let repo_path = repo
        .workdir()
        .ok_or_else(|| GitError("Bare repository".into()))?
        .to_string_lossy()
        .to_string();

    let branch = current_branch(repo)?;

    Ok(RepoInfo { repo_path, branch })
}

/// Get the last commit message (for amend).
pub fn last_commit_message(repo: &Repository) -> Result<Option<String>> {
    let head = repo.head()?;
    let commit = head.peel_to_commit()?;
    Ok(commit.message().map(String::from))
}

/// Resolve a ref string to a tree.
///
/// Special values:
/// - "@" means the working tree (returns None, caller handles specially)
/// - "HEAD" resolves to the current HEAD commit
fn resolve_to_tree<'a>(repo: &'a Repository, refspec: &str) -> Result<Option<Tree<'a>>> {
    if refspec == "@" {
        return Ok(None); // Working tree - no tree object
    }

    let obj = repo
        .revparse_single(refspec)
        .map_err(|e| GitError(format!("Cannot resolve '{}': {}", refspec, e)))?;

    let commit = obj
        .peel_to_commit()
        .map_err(|e| GitError(format!("'{}' is not a commit: {}", refspec, e)))?;

    Ok(Some(commit.tree()?))
}

/// Info about a changed file collected from git diff.
struct FileChange {
    before_path: Option<String>,
    after_path: Option<String>,
    status: Delta,
}

/// Compute the diff between two refs.
///
/// Returns a list of FileDiff objects with full content and alignments.
pub fn compute_diff(repo: &Repository, before_ref: &str, after_ref: &str) -> Result<Vec<FileDiff>> {
    let before_tree = resolve_to_tree(repo, before_ref)?;
    let after_tree = resolve_to_tree(repo, after_ref)?;
    let is_working_tree = after_ref == "@";

    let mut opts = DiffOptions::new();
    opts.ignore_submodules(true);

    let diff = if is_working_tree {
        // Diff from before_tree to working directory
        repo.diff_tree_to_workdir_with_index(before_tree.as_ref(), Some(&mut opts))?
    } else {
        // Diff between two trees
        repo.diff_tree_to_tree(before_tree.as_ref(), after_tree.as_ref(), Some(&mut opts))?
    };

    // Collect changed files with their paths and status
    let mut file_changes: Vec<FileChange> = Vec::new();

    for delta in diff.deltas() {
        let before_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().to_string());
        let after_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().to_string());

        file_changes.push(FileChange {
            before_path,
            after_path,
            status: delta.status(),
        });
    }

    // Build FileDiff for each changed file
    let mut result: Vec<FileDiff> = Vec::new();

    for change in file_changes {
        let before_file = if let Some(ref path) = change.before_path {
            if change.status != Delta::Added {
                load_file(repo, before_tree.as_ref(), Path::new(path), false)?
            } else {
                None
            }
        } else {
            None
        };

        let after_file = if let Some(ref path) = change.after_path {
            if change.status != Delta::Deleted {
                if is_working_tree {
                    load_file_from_workdir(repo, Path::new(path))?
                } else {
                    load_file(repo, after_tree.as_ref(), Path::new(path), false)?
                }
            } else {
                None
            }
        } else {
            None
        };

        let alignments = compute_alignments(&before_file, &after_file);

        result.push(FileDiff {
            before: before_file,
            after: after_file,
            alignments,
        });
    }

    // Sort by path
    result.sort_by(|a, b| a.path().cmp(b.path()));
    Ok(result)
}

/// Load a file from a git tree.
fn load_file(
    repo: &Repository,
    tree: Option<&Tree>,
    path: &Path,
    _is_workdir: bool,
) -> Result<Option<File>> {
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
        .map_err(|e| GitError(format!("Cannot load object: {}", e)))?;

    let blob = match obj.as_blob() {
        Some(b) => b,
        None => return Ok(None), // Not a file (maybe a submodule)
    };

    let bytes = blob.content();
    let content = if FileContent::is_binary_data(bytes) {
        FileContent::Binary
    } else {
        let text = String::from_utf8_lossy(bytes);
        FileContent::from_text(&text)
    };

    Ok(Some(File {
        path: path.to_string_lossy().to_string(),
        content,
    }))
}

/// Load a file from the working directory.
fn load_file_from_workdir(repo: &Repository, path: &Path) -> Result<Option<File>> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| GitError("Bare repository".into()))?;
    let full_path = workdir.join(path);

    if !full_path.exists() {
        return Ok(None);
    }

    let bytes =
        std::fs::read(&full_path).map_err(|e| GitError(format!("Cannot read file: {}", e)))?;

    let content = if FileContent::is_binary_data(&bytes) {
        FileContent::Binary
    } else {
        let text = String::from_utf8_lossy(&bytes);
        FileContent::from_text(&text)
    };

    Ok(Some(File {
        path: path.to_string_lossy().to_string(),
        content,
    }))
}

/// Compute alignments between before and after content.
///
/// Alignments exhaustively partition both files, marking which regions changed.
fn compute_alignments(before: &Option<File>, after: &Option<File>) -> Vec<Alignment> {
    let before_lines: &[String] = before
        .as_ref()
        .map(|f| f.content.lines())
        .unwrap_or_default();
    let after_lines: &[String] = after
        .as_ref()
        .map(|f| f.content.lines())
        .unwrap_or_default();

    if before_lines.is_empty() && after_lines.is_empty() {
        return vec![];
    }

    // Handle simple cases: all added or all deleted
    if before_lines.is_empty() {
        return vec![Alignment {
            before: Span::new(0, 0),
            after: Span::new(0, after_lines.len() as u32),
            changed: true,
        }];
    }

    if after_lines.is_empty() {
        return vec![Alignment {
            before: Span::new(0, before_lines.len() as u32),
            after: Span::new(0, 0),
            changed: true,
        }];
    }

    // Find matching blocks between the two files
    let matches = find_matching_blocks(before_lines, after_lines);

    // Convert matching blocks to alignments
    let mut alignments = Vec::new();
    let mut before_pos = 0u32;
    let mut after_pos = 0u32;

    for (before_start, after_start, len) in matches {
        let before_start = before_start as u32;
        let after_start = after_start as u32;
        let len = len as u32;

        // Gap before this match = changed region
        if before_pos < before_start || after_pos < after_start {
            alignments.push(Alignment {
                before: Span::new(before_pos, before_start),
                after: Span::new(after_pos, after_start),
                changed: true,
            });
        }

        // The matching region itself = unchanged
        if len > 0 {
            alignments.push(Alignment {
                before: Span::new(before_start, before_start + len),
                after: Span::new(after_start, after_start + len),
                changed: false,
            });
        }

        before_pos = before_start + len;
        after_pos = after_start + len;
    }

    // Handle any remaining content after the last match
    let before_len = before_lines.len() as u32;
    let after_len = after_lines.len() as u32;
    if before_pos < before_len || after_pos < after_len {
        alignments.push(Alignment {
            before: Span::new(before_pos, before_len),
            after: Span::new(after_pos, after_len),
            changed: true,
        });
    }

    alignments
}

/// Find matching blocks between two sequences of lines.
///
/// Returns a list of (before_start, after_start, length) tuples.
/// The matches are guaranteed to be monotonically increasing in both dimensions,
/// i.e., for consecutive matches A and B: A.before_end <= B.before_start AND A.after_end <= B.after_start
fn find_matching_blocks(before: &[String], after: &[String]) -> Vec<(usize, usize, usize)> {
    if before.is_empty() || after.is_empty() {
        return vec![];
    }

    // Build a map of line -> positions in "after"
    let mut after_positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, line) in after.iter().enumerate() {
        after_positions.entry(line.as_str()).or_default().push(i);
    }

    // Find matching blocks greedily
    let mut matches = Vec::new();
    let mut after_used = vec![false; after.len()];

    let mut before_idx = 0;
    while before_idx < before.len() {
        let line = &before[before_idx];

        // Find the first unused occurrence in after
        if let Some(positions) = after_positions.get(line.as_str()) {
            if let Some(&after_idx) = positions.iter().find(|&&i| !after_used[i]) {
                // Found a match - extend it as far as possible
                let mut len = 1;
                after_used[after_idx] = true;

                while before_idx + len < before.len()
                    && after_idx + len < after.len()
                    && !after_used[after_idx + len]
                    && before[before_idx + len] == after[after_idx + len]
                {
                    after_used[after_idx + len] = true;
                    len += 1;
                }

                matches.push((before_idx, after_idx, len));
                before_idx += len;
                continue;
            }
        }

        before_idx += 1;
    }

    // Sort by position in before
    matches.sort_by_key(|m| m.0);

    // Filter to ensure monotonicity in both dimensions.
    // We need matches where both before and after positions are strictly increasing.
    // Use a greedy approach: keep a match if it doesn't violate monotonicity with the last kept match.
    let mut filtered = Vec::new();
    let mut last_after_end = 0usize;

    for (before_start, after_start, len) in matches {
        // Skip this match if it would go backwards in the after dimension
        if after_start >= last_after_end {
            filtered.push((before_start, after_start, len));
            last_after_end = after_start + len;
        }
    }

    filtered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_matching_blocks() {
        let before: Vec<String> = vec!["a", "b", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect();
        let after: Vec<String> = vec!["a", "x", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect();

        let matches = find_matching_blocks(&before, &after);
        // Should find "a" at (0,0,1) and "c","d" at (2,2,2)
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0], (0, 0, 1));
        assert_eq!(matches[1], (2, 2, 2));
    }

    #[test]
    fn test_compute_alignments() {
        let before = Some(File {
            path: "test.txt".into(),
            content: FileContent::Text {
                lines: vec!["a".into(), "b".into(), "c".into()],
            },
        });
        let after = Some(File {
            path: "test.txt".into(),
            content: FileContent::Text {
                lines: vec!["a".into(), "x".into(), "c".into()],
            },
        });

        let alignments = compute_alignments(&before, &after);

        // Should have: "a" (unchanged), "b"->"x" (changed), "c" (unchanged)
        assert_eq!(alignments.len(), 3);

        assert!(!alignments[0].changed); // "a"
        assert_eq!(alignments[0].before, Span::new(0, 1));
        assert_eq!(alignments[0].after, Span::new(0, 1));

        assert!(alignments[1].changed); // "b" -> "x"
        assert_eq!(alignments[1].before, Span::new(1, 2));
        assert_eq!(alignments[1].after, Span::new(1, 2));

        assert!(!alignments[2].changed); // "c"
        assert_eq!(alignments[2].before, Span::new(2, 3));
        assert_eq!(alignments[2].after, Span::new(2, 3));
    }

    #[test]
    fn test_compute_alignments_added_file() {
        let before = None;
        let after = Some(File {
            path: "new.txt".into(),
            content: FileContent::Text {
                lines: vec!["line1".into(), "line2".into()],
            },
        });

        let alignments = compute_alignments(&before, &after);

        assert_eq!(alignments.len(), 1);
        assert!(alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 0));
        assert_eq!(alignments[0].after, Span::new(0, 2));
    }

    #[test]
    fn test_compute_alignments_deleted_file() {
        let before = Some(File {
            path: "old.txt".into(),
            content: FileContent::Text {
                lines: vec!["line1".into(), "line2".into()],
            },
        });
        let after = None;

        let alignments = compute_alignments(&before, &after);

        assert_eq!(alignments.len(), 1);
        assert!(alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 2));
        assert_eq!(alignments[0].after, Span::new(0, 0));
    }

    #[test]
    fn test_find_matching_blocks_monotonicity() {
        // Test case where greedy matching could produce non-monotonic results.
        // If "x" appears at position 5 in after, and "y" appears at position 2 in after,
        // but "x" comes before "y" in before, we must skip the "y" match.
        let before: Vec<String> = vec!["x", "y", "z"].into_iter().map(String::from).collect();
        let after: Vec<String> = vec!["a", "b", "y", "c", "d", "x", "z"]
            .into_iter()
            .map(String::from)
            .collect();

        let matches = find_matching_blocks(&before, &after);

        // Verify monotonicity: each match's after_start must be >= previous match's after_end
        let mut last_after_end = 0;
        for (before_start, after_start, len) in &matches {
            assert!(
                *after_start >= last_after_end,
                "Non-monotonic match: after_start {} < last_after_end {} (before_start={})",
                after_start,
                last_after_end,
                before_start
            );
            last_after_end = after_start + len;
        }
    }

    #[test]
    fn test_alignments_no_overlap() {
        // Test with content that previously caused overlaps
        let before = Some(File {
            path: "test.txt".into(),
            content: FileContent::Text {
                lines: vec!["a", "b", "c", "d", "e"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            },
        });
        let after = Some(File {
            path: "test.txt".into(),
            content: FileContent::Text {
                lines: vec!["x", "c", "y", "a", "z"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            },
        });

        let alignments = compute_alignments(&before, &after);

        // Verify no overlaps in before spans
        let mut last_before_end = 0u32;
        for a in &alignments {
            assert!(
                a.before.start >= last_before_end,
                "Overlap in before: start {} < last_end {}",
                a.before.start,
                last_before_end
            );
            last_before_end = a.before.end;
        }

        // Verify no overlaps in after spans
        let mut last_after_end = 0u32;
        for a in &alignments {
            assert!(
                a.after.start >= last_after_end,
                "Overlap in after: start {} < last_end {}",
                a.after.start,
                last_after_end
            );
            last_after_end = a.after.end;
        }
    }
}
