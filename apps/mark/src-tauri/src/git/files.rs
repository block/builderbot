//! File browsing operations for viewing files outside the current diff.
//!
//! This module provides:
//! - `search_files`: Fuzzy search for files in a git tree
//! - `get_file_at_ref`: Load file content at a specific ref

use std::path::Path;

use super::cli::{self, GitError};
use super::types::{File, FileContent, WORKDIR};

/// Search for files matching a query in the repository at a given ref.
///
/// Uses fuzzy matching on file paths - matches if all query characters
/// appear in order in the path (case-insensitive).
///
/// Returns up to `limit` matching file paths, sorted by match quality:
/// - Exact filename matches first
/// - Then by path length (shorter paths ranked higher)
pub fn search_files(
    repo: &Path,
    ref_name: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<String>, GitError> {
    let query_lower = query.to_lowercase();

    // Use HEAD for WORKDIR since we're listing tracked files
    let tree_ref = if ref_name == WORKDIR {
        "HEAD"
    } else {
        ref_name
    };

    // git ls-tree -r --name-only <ref>
    let output = cli::run(repo, &["ls-tree", "-r", "--name-only", tree_ref])?;

    let mut matches: Vec<(String, MatchScore)> = Vec::new();

    for line in output.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }

        if let Some(score) = fuzzy_match(path, &query_lower) {
            matches.push((path.to_string(), score));
        }
    }

    // Sort by match quality
    matches.sort_by(|a, b| b.1.cmp(&a.1));

    // Return top results
    Ok(matches
        .into_iter()
        .take(limit)
        .map(|(path, _)| path)
        .collect())
}

/// Match quality score for sorting results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MatchScore {
    /// Exact filename match (highest priority)
    exact_filename: bool,
    /// Filename starts with query
    filename_prefix: bool,
    /// Query appears contiguously in path
    contiguous: bool,
    /// Negative path length (shorter = better)
    neg_path_len: i32,
}

/// Fuzzy match a path against a query.
///
/// Returns Some(score) if the path matches, None otherwise.
/// A path matches if all query characters appear in order (case-insensitive).
fn fuzzy_match(path: &str, query_lower: &str) -> Option<MatchScore> {
    if query_lower.is_empty() {
        return Some(MatchScore {
            exact_filename: false,
            filename_prefix: false,
            contiguous: true,
            neg_path_len: -(path.len() as i32),
        });
    }

    let path_lower = path.to_lowercase();

    // Check if all query chars appear in order
    let mut query_chars = query_lower.chars().peekable();
    let mut contiguous = true;
    let mut last_match_idx: Option<usize> = None;

    for (idx, c) in path_lower.chars().enumerate() {
        if query_chars.peek() == Some(&c) {
            // Check contiguity
            if let Some(last) = last_match_idx {
                if idx != last + 1 {
                    contiguous = false;
                }
            }
            last_match_idx = Some(idx);
            query_chars.next();
        }
    }

    // If we didn't match all query chars, no match
    if query_chars.peek().is_some() {
        return None;
    }

    // Extract filename for additional scoring
    let filename = path.rsplit('/').next().unwrap_or(path);
    let filename_lower = filename.to_lowercase();

    let exact_filename = filename_lower == query_lower;
    let filename_prefix = filename_lower.starts_with(query_lower);

    Some(MatchScore {
        exact_filename,
        filename_prefix,
        contiguous,
        neg_path_len: -(path.len() as i32),
    })
}

/// Get the content of a file at a specific ref.
///
/// For WORKDIR, reads from the working directory.
/// For other refs, reads from the git tree.
pub fn get_file_at_ref(repo: &Path, ref_name: &str, path: &str) -> Result<File, GitError> {
    if ref_name == WORKDIR {
        // Read from working directory
        let full_path = repo.join(path);

        if !full_path.exists() {
            return Err(GitError::CommandFailed(format!("File not found: {path}")));
        }

        if full_path.is_dir() {
            return Err(GitError::CommandFailed(format!(
                "Path is a directory: {path}"
            )));
        }

        let bytes = std::fs::read(&full_path)
            .map_err(|e| GitError::CommandFailed(format!("Cannot read file: {e}")))?;

        let content = if is_binary(&bytes) {
            FileContent::Binary
        } else {
            let text = String::from_utf8_lossy(&bytes);
            text_to_content(&text)
        };

        Ok(File {
            path: path.to_string(),
            content,
        })
    } else {
        // Read from git tree: git show <ref>:<path>
        let spec = format!("{ref_name}:{path}");
        let output = cli::run(repo, &["show", &spec]).map_err(|e| match e {
            GitError::CommandFailed(msg) if msg.contains("does not exist") => {
                GitError::CommandFailed(format!("File not found: {path}"))
            }
            other => other,
        })?;

        // git show returns text, check if it looks binary
        let bytes = output.as_bytes();
        let content = if is_binary(bytes) {
            FileContent::Binary
        } else {
            text_to_content(&output)
        };

        Ok(File {
            path: path.to_string(),
            content,
        })
    }
}

/// Check if data appears to be binary (contains null bytes in first 8KB)
fn is_binary(data: &[u8]) -> bool {
    let check_len = data.len().min(8192);
    data[..check_len].contains(&0)
}

/// Convert text to FileContent with lines
fn text_to_content(text: &str) -> FileContent {
    let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    FileContent::Text { lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_basic() {
        // Exact match
        assert!(fuzzy_match("src/main.rs", "main.rs").is_some());

        // Fuzzy match
        assert!(fuzzy_match("src/lib/utils/helpers.ts", "utils").is_some());
        assert!(fuzzy_match("src/lib/utils/helpers.ts", "uts").is_some());

        // No match
        assert!(fuzzy_match("src/main.rs", "xyz").is_none());
        assert!(fuzzy_match("src/main.rs", "nim").is_none()); // 'n' before 'i' before 'm' - but in path it's m-a-i-n
    }

    #[test]
    fn test_fuzzy_match_scoring() {
        // Exact filename should score higher
        let exact = fuzzy_match("src/utils.ts", "utils.ts").unwrap();
        let partial = fuzzy_match("src/utils/helpers.ts", "utils.ts").unwrap();
        assert!(exact > partial);

        // Shorter paths should score higher for same match
        let short = fuzzy_match("utils.ts", "ut").unwrap();
        let long = fuzzy_match("src/lib/utils.ts", "ut").unwrap();
        assert!(short > long);
    }

    #[test]
    fn test_fuzzy_match_empty_query() {
        // Empty query matches everything
        assert!(fuzzy_match("any/path.rs", "").is_some());
    }
}
