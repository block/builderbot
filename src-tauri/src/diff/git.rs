//! Git operations for computing diffs.
//!
//! All functions are stateless - they discover the repo fresh each call.

use std::cell::RefCell;
use std::path::Path;

use git2::{Delta, Diff, DiffOptions, Repository, Tree};
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
    /// Hunks from git diff: (old_start, old_lines, new_start, new_lines)
    /// Line numbers are 1-indexed from git, we convert to 0-indexed.
    hunks: Vec<Hunk>,
}

/// A hunk from git diff, converted to 0-indexed line numbers.
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

/// Compute the diff between two refs.
///
/// Returns a list of FileDiff objects with full content and alignments.
pub fn compute_diff(repo: &Repository, before_ref: &str, after_ref: &str) -> Result<Vec<FileDiff>> {
    let before_tree = resolve_to_tree(repo, before_ref)?;
    let after_tree = resolve_to_tree(repo, after_ref)?;
    let is_working_tree = after_ref == "@";

    let mut opts = DiffOptions::new();
    opts.ignore_submodules(true);
    // Use 0 context lines so hunks contain only the actual changes,
    // not surrounding context. This gives us precise alignment boundaries.
    opts.context_lines(0);

    let diff = if is_working_tree {
        // Diff from before_tree to working directory
        // Include untracked files so new files show up
        opts.include_untracked(true);
        // Recurse into untracked directories to show individual files
        opts.recurse_untracked_dirs(true);
        repo.diff_tree_to_workdir_with_index(before_tree.as_ref(), Some(&mut opts))?
    } else {
        // Diff between two trees
        repo.diff_tree_to_tree(before_tree.as_ref(), after_tree.as_ref(), Some(&mut opts))?
    };

    // Collect changed files with their paths, status, and hunks
    let file_changes = collect_file_changes(&diff)?;

    // Build FileDiff for each changed file
    let mut result: Vec<FileDiff> = Vec::new();

    for change in file_changes {
        let before_file = if let Some(ref path) = change.before_path {
            if change.status != Delta::Added {
                load_file(repo, before_tree.as_ref(), Path::new(path))?
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
                    load_file(repo, after_tree.as_ref(), Path::new(path))?
                }
            } else {
                None
            }
        } else {
            None
        };

        // Skip entries where we couldn't load either file (e.g., submodules, directories)
        if before_file.is_none() && after_file.is_none() {
            log::debug!(
                "Skipping diff entry with no loadable files: before={:?}, after={:?}",
                change.before_path,
                change.after_path
            );
            continue;
        }

        let alignments = compute_alignments_from_hunks(&change.hunks, &before_file, &after_file);

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

/// Collect file changes with hunks from a git diff.
fn collect_file_changes(diff: &Diff) -> Result<Vec<FileChange>> {
    // We need to collect hunks per file. The foreach callback gives us deltas and hunks,
    // but we need to associate hunks with their files.
    let file_changes: RefCell<Vec<FileChange>> = RefCell::new(Vec::new());
    let current_file_idx: RefCell<Option<usize>> = RefCell::new(None);

    diff.foreach(
        &mut |delta, _progress| {
            let before_path = delta
                .old_file()
                .path()
                .map(|p| p.to_string_lossy().to_string());
            let after_path = delta
                .new_file()
                .path()
                .map(|p| p.to_string_lossy().to_string());

            let mut changes = file_changes.borrow_mut();
            changes.push(FileChange {
                before_path,
                after_path,
                status: delta.status(),
                hunks: Vec::new(),
            });
            *current_file_idx.borrow_mut() = Some(changes.len() - 1);
            true
        },
        None, // binary callback
        Some(&mut |_delta, hunk| {
            // Git uses 1-indexed line numbers, convert to 0-indexed
            // Also handle the special case where old_start/new_start is 0 for empty files
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

            let h = Hunk {
                old_start,
                old_lines: hunk.old_lines(),
                new_start,
                new_lines: hunk.new_lines(),
            };

            if let Some(idx) = *current_file_idx.borrow() {
                file_changes.borrow_mut()[idx].hunks.push(h);
            }
            true
        }),
        None, // line callback
    )?;

    Ok(file_changes.into_inner())
}

/// Compute alignments from git hunks.
///
/// This ensures our alignments match what git diff reports.
/// Hunks define the changed regions; we fill in unchanged regions between them.
fn compute_alignments_from_hunks(
    hunks: &[Hunk],
    before: &Option<File>,
    after: &Option<File>,
) -> Vec<Alignment> {
    let before_len = before
        .as_ref()
        .map(|f| f.content.lines().len() as u32)
        .unwrap_or(0);
    let after_len = after
        .as_ref()
        .map(|f| f.content.lines().len() as u32)
        .unwrap_or(0);

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

/// Load a file from a git tree.
fn load_file(repo: &Repository, tree: Option<&Tree>, path: &Path) -> Result<Option<File>> {
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

    // Skip directories (e.g., submodules)
    if full_path.is_dir() {
        log::debug!(
            "Skipping directory in load_file_from_workdir: {}",
            path.display()
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a File with text content
    fn text_file(path: &str, lines: Vec<&str>) -> Option<File> {
        Some(File {
            path: path.into(),
            content: FileContent::Text {
                lines: lines.into_iter().map(String::from).collect(),
            },
        })
    }

    #[test]
    fn test_alignments_from_single_hunk() {
        // Simulates: lines 0-1 unchanged, line 2 changed (1 line -> 1 line), lines 3-4 unchanged
        // Git hunk: @@ -2,1 +2,1 @@ (1-indexed: line 3 in both)
        let hunks = vec![Hunk {
            old_start: 2, // 0-indexed
            old_lines: 1,
            new_start: 2,
            new_lines: 1,
        }];

        let before = text_file("test.txt", vec!["a", "b", "X", "c", "d"]);
        let after = text_file("test.txt", vec!["a", "b", "Y", "c", "d"]);

        let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

        assert_eq!(alignments.len(), 3);

        // Lines 0-1 unchanged
        assert!(!alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 2));
        assert_eq!(alignments[0].after, Span::new(0, 2));

        // Line 2 changed
        assert!(alignments[1].changed);
        assert_eq!(alignments[1].before, Span::new(2, 3));
        assert_eq!(alignments[1].after, Span::new(2, 3));

        // Lines 3-4 unchanged
        assert!(!alignments[2].changed);
        assert_eq!(alignments[2].before, Span::new(3, 5));
        assert_eq!(alignments[2].after, Span::new(3, 5));
    }

    #[test]
    fn test_alignments_from_hunk_with_different_sizes() {
        // Simulates: 2 lines deleted, 3 lines added
        // Git hunk: @@ -1,2 +1,3 @@ (delete 2 lines at position 1, add 3 lines)
        let hunks = vec![Hunk {
            old_start: 0,
            old_lines: 2,
            new_start: 0,
            new_lines: 3,
        }];

        let before = text_file("test.txt", vec!["old1", "old2", "same"]);
        let after = text_file("test.txt", vec!["new1", "new2", "new3", "same"]);

        let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

        assert_eq!(alignments.len(), 2);

        // Changed region: 2 lines -> 3 lines
        assert!(alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 2));
        assert_eq!(alignments[0].after, Span::new(0, 3));

        // Unchanged: "same"
        assert!(!alignments[1].changed);
        assert_eq!(alignments[1].before, Span::new(2, 3));
        assert_eq!(alignments[1].after, Span::new(3, 4));
    }

    #[test]
    fn test_alignments_from_multiple_hunks() {
        // Two separate changes with unchanged content between them
        let hunks = vec![
            Hunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
            },
            Hunk {
                old_start: 4,
                old_lines: 1,
                new_start: 4,
                new_lines: 1,
            },
        ];

        let before = text_file("test.txt", vec!["a", "X", "b", "c", "Y", "d"]);
        let after = text_file("test.txt", vec!["a", "X'", "b", "c", "Y'", "d"]);

        let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

        assert_eq!(alignments.len(), 5);

        // Line 0 unchanged
        assert!(!alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 1));

        // Line 1 changed
        assert!(alignments[1].changed);
        assert_eq!(alignments[1].before, Span::new(1, 2));

        // Lines 2-3 unchanged
        assert!(!alignments[2].changed);
        assert_eq!(alignments[2].before, Span::new(2, 4));

        // Line 4 changed
        assert!(alignments[3].changed);
        assert_eq!(alignments[3].before, Span::new(4, 5));

        // Line 5 unchanged
        assert!(!alignments[4].changed);
        assert_eq!(alignments[4].before, Span::new(5, 6));
    }

    #[test]
    fn test_alignments_added_file() {
        // New file with no hunks (git reports the whole file as added)
        let hunks = vec![];
        let before = None;
        let after = text_file("new.txt", vec!["line1", "line2"]);

        let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

        assert_eq!(alignments.len(), 1);
        assert!(alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 0));
        assert_eq!(alignments[0].after, Span::new(0, 2));
    }

    #[test]
    fn test_alignments_deleted_file() {
        // Deleted file with no hunks
        let hunks = vec![];
        let before = text_file("old.txt", vec!["line1", "line2"]);
        let after = None;

        let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

        assert_eq!(alignments.len(), 1);
        assert!(alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 2));
        assert_eq!(alignments[0].after, Span::new(0, 0));
    }

    #[test]
    fn test_alignments_exhaustive_coverage() {
        // Verify alignments cover the entire file with no gaps or overlaps
        let hunks = vec![
            Hunk {
                old_start: 2,
                old_lines: 2,
                new_start: 2,
                new_lines: 3,
            },
            Hunk {
                old_start: 6,
                old_lines: 1,
                new_start: 7,
                new_lines: 2,
            },
        ];

        let before = text_file(
            "test.txt",
            vec!["0", "1", "2", "3", "4", "5", "6", "7", "8"],
        );
        let after = text_file(
            "test.txt",
            vec!["0", "1", "a", "b", "c", "4", "5", "x", "y", "7", "8"],
        );

        let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

        // Verify no gaps in before
        let mut expected_before = 0u32;
        for a in &alignments {
            assert_eq!(
                a.before.start, expected_before,
                "Gap in before at {}",
                expected_before
            );
            expected_before = a.before.end;
        }
        assert_eq!(expected_before, 9, "Before not fully covered");

        // Verify no gaps in after
        let mut expected_after = 0u32;
        for a in &alignments {
            assert_eq!(
                a.after.start, expected_after,
                "Gap in after at {}",
                expected_after
            );
            expected_after = a.after.end;
        }
        assert_eq!(expected_after, 11, "After not fully covered");
    }

    #[test]
    fn test_alignments_hunk_at_start() {
        // Change at the very beginning of the file
        let hunks = vec![Hunk {
            old_start: 0,
            old_lines: 1,
            new_start: 0,
            new_lines: 1,
        }];

        let before = text_file("test.txt", vec!["X", "a", "b"]);
        let after = text_file("test.txt", vec!["Y", "a", "b"]);

        let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

        assert_eq!(alignments.len(), 2);

        // First line changed
        assert!(alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 1));
        assert_eq!(alignments[0].after, Span::new(0, 1));

        // Rest unchanged
        assert!(!alignments[1].changed);
        assert_eq!(alignments[1].before, Span::new(1, 3));
        assert_eq!(alignments[1].after, Span::new(1, 3));
    }

    #[test]
    fn test_alignments_hunk_at_end() {
        // Change at the very end of the file
        let hunks = vec![Hunk {
            old_start: 2,
            old_lines: 1,
            new_start: 2,
            new_lines: 1,
        }];

        let before = text_file("test.txt", vec!["a", "b", "X"]);
        let after = text_file("test.txt", vec!["a", "b", "Y"]);

        let alignments = compute_alignments_from_hunks(&hunks, &before, &after);

        assert_eq!(alignments.len(), 2);

        // First two lines unchanged
        assert!(!alignments[0].changed);
        assert_eq!(alignments[0].before, Span::new(0, 2));

        // Last line changed
        assert!(alignments[1].changed);
        assert_eq!(alignments[1].before, Span::new(2, 3));
    }
}
