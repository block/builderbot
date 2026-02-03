//! GitHub integration for fetching pull requests.
//!
//! Uses the GitHub CLI (`gh`) for authentication and API access.
//! Includes caching to minimize API calls.

use super::cli::GitError;
use super::DiffSpec;
use super::GitRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::RwLock;
use std::time::{Duration, Instant};

// =============================================================================
// Types
// =============================================================================

/// GitHub authentication status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAuthStatus {
    pub authenticated: bool,
    /// Help text if not authenticated (e.g., "run: gh auth login")
    pub setup_hint: Option<String>,
}

/// A pull request from GitHub (for display in picker)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// Target branch (e.g., "main")
    pub base_ref: String,
    /// Source branch (e.g., "feature-x") - not useful for forks
    pub head_ref: String,
    pub draft: bool,
    pub updated_at: String,
}

// =============================================================================
// Cache
// =============================================================================

/// How long to cache PR lists before they're considered stale.
const CACHE_TTL: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// Cached PR list with expiration.
struct CachedPRList {
    prs: Vec<PullRequest>,
    fetched_at: Instant,
}

/// Global cache for PR lists, keyed by repo path.
static PR_CACHE: RwLock<Option<HashMap<String, CachedPRList>>> = RwLock::new(None);

fn get_cached_prs(repo: &Path) -> Option<Vec<PullRequest>> {
    let key = repo.to_string_lossy().to_string();
    let cache = PR_CACHE.read().ok()?;
    let cache = cache.as_ref()?;
    let entry = cache.get(&key)?;

    if entry.fetched_at.elapsed() < CACHE_TTL {
        Some(entry.prs.clone())
    } else {
        None
    }
}

fn set_cached_prs(repo: &Path, prs: Vec<PullRequest>) {
    let key = repo.to_string_lossy().to_string();
    let mut cache = match PR_CACHE.write() {
        Ok(c) => c,
        Err(_) => return,
    };

    let cache = cache.get_or_insert_with(HashMap::new);
    cache.insert(
        key,
        CachedPRList {
            prs,
            fetched_at: Instant::now(),
        },
    );
}

/// Clear the cache for a specific repo, forcing a fresh fetch.
pub fn invalidate_cache(repo: &Path) {
    let key = repo.to_string_lossy().to_string();
    if let Ok(mut cache) = PR_CACHE.write() {
        if let Some(ref mut map) = *cache {
            map.remove(&key);
        }
    }
}

// =============================================================================
// GitHub CLI Integration
// =============================================================================

/// Common paths where `gh` might be installed.
/// GUI apps on macOS don't inherit the shell's PATH, so we check these explicitly.
const GH_SEARCH_PATHS: &[&str] = &[
    "/opt/homebrew/bin",              // Homebrew on Apple Silicon
    "/usr/local/bin",                 // Homebrew on Intel Mac, common Linux location
    "/usr/bin",                       // System binaries
    "/home/linuxbrew/.linuxbrew/bin", // Linuxbrew
];

/// Find the `gh` CLI executable.
fn find_gh() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    // First, check if `gh` is directly available
    if let Ok(output) = Command::new("gh").arg("--version").output() {
        if output.status.success() {
            return Some(PathBuf::from("gh"));
        }
    }

    // Check common installation paths
    for dir in GH_SEARCH_PATHS {
        let path = PathBuf::from(dir).join("gh");
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Run a gh command in the context of a repo
fn run_gh(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    let gh_path = find_gh().ok_or_else(|| {
        GitError::CommandFailed("GitHub CLI not found. Install with: brew install gh".to_string())
    })?;

    let output = Command::new(&gh_path)
        .current_dir(repo)
        .args(args)
        .output()
        .map_err(|e| GitError::CommandFailed(format!("Failed to run gh: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not logged in") || stderr.contains("no oauth token") {
            return Err(GitError::CommandFailed(
                "Not authenticated with GitHub CLI. Run: gh auth login".to_string(),
            ));
        }
        return Err(GitError::CommandFailed(stderr.into_owned()));
    }

    String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8)
}

// =============================================================================
// Public API
// =============================================================================

/// Check if GitHub CLI is installed and authenticated
pub fn check_github_auth() -> GitHubAuthStatus {
    let gh_path = match find_gh() {
        Some(p) => p,
        None => {
            return GitHubAuthStatus {
                authenticated: false,
                setup_hint: Some("GitHub CLI not found. Install with: brew install gh".to_string()),
            }
        }
    };

    let output = match Command::new(&gh_path).args(["auth", "status"]).output() {
        Ok(o) => o,
        Err(e) => {
            return GitHubAuthStatus {
                authenticated: false,
                setup_hint: Some(format!("Failed to run gh: {}", e)),
            }
        }
    };

    if output.status.success() {
        GitHubAuthStatus {
            authenticated: true,
            setup_hint: None,
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        GitHubAuthStatus {
            authenticated: false,
            setup_hint: Some(if stderr.contains("not logged in") {
                "Run: gh auth login".to_string()
            } else {
                stderr.trim().to_string()
            }),
        }
    }
}

/// Response from `gh pr list --json`
#[derive(Debug, Deserialize)]
struct GhPrListItem {
    number: u64,
    title: String,
    author: GhAuthor,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: String,
}

impl From<GhPrListItem> for PullRequest {
    fn from(item: GhPrListItem) -> Self {
        PullRequest {
            number: item.number,
            title: item.title,
            author: item.author.login,
            base_ref: item.base_ref_name,
            head_ref: item.head_ref_name,
            draft: item.is_draft,
            updated_at: item.updated_at,
        }
    }
}

/// List open pull requests for the repo
pub fn list_pull_requests(repo: &Path) -> Result<Vec<PullRequest>, GitError> {
    // Check cache first
    if let Some(cached) = get_cached_prs(repo) {
        return Ok(cached);
    }

    let output = run_gh(
        repo,
        &[
            "pr",
            "list",
            "--state=open",
            "--limit=50",
            "--json=number,title,author,baseRefName,headRefName,isDraft,updatedAt",
        ],
    )?;

    let items: Vec<GhPrListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    let prs: Vec<PullRequest> = items.into_iter().map(Into::into).collect();

    // Cache the result
    set_cached_prs(repo, prs.clone());

    Ok(prs)
}

/// Search for pull requests on GitHub using a query string.
/// Uses GitHub's search syntax via `gh pr list --search`.
/// Does not use caching since search queries vary.
pub fn search_pull_requests(repo: &Path, query: &str) -> Result<Vec<PullRequest>, GitError> {
    let output = run_gh(
        repo,
        &[
            "pr",
            "list",
            "--state=open",
            "--limit=50",
            &format!("--search={}", query),
            "--json=number,title,author,baseRefName,headRefName,isDraft,updatedAt",
        ],
    )?;

    let items: Vec<GhPrListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items.into_iter().map(Into::into).collect())
}

/// Fetch PR refs and compute merge-base
///
/// - Fetches refs/pull/{number}/head
/// - Fetches origin/{base_ref}
/// - Computes merge-base
///
/// Returns DiffSpec with two concrete SHAs: Rev(merge_base)..Rev(head_sha)
pub fn fetch_pr(repo: &Path, base_ref: &str, pr_number: u64) -> Result<DiffSpec, GitError> {
    use super::cli;

    // Fetch the PR head ref
    let pr_ref = format!("refs/pull/{}/head", pr_number);
    cli::run(repo, &["fetch", "origin", &pr_ref])?;

    // Get the SHA of the fetched PR head IMMEDIATELY (before next fetch overwrites FETCH_HEAD)
    let head_sha = cli::run(repo, &["rev-parse", "FETCH_HEAD"])?
        .trim()
        .to_string();

    // Fetch the base branch
    let base_remote_ref = format!("origin/{}", base_ref);
    cli::run(repo, &["fetch", "origin", base_ref])?;

    // Compute merge-base between base and PR head
    let merge_base_sha = cli::run(repo, &["merge-base", &base_remote_ref, &head_sha])?
        .trim()
        .to_string();

    Ok(DiffSpec {
        base: GitRef::Rev(merge_base_sha),
        head: GitRef::Rev(head_sha),
    })
}

// =============================================================================
// Review Sync
// =============================================================================

use crate::review::Comment;

/// Result of syncing a review to GitHub.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubSyncResult {
    /// URL to the pending review on GitHub
    pub review_url: String,
    /// Number of comments synced
    pub comment_count: usize,
}

/// Get the GitHub token from `gh auth token`.
fn get_github_token() -> Result<String, GitError> {
    let gh_path = find_gh().ok_or_else(|| {
        GitError::CommandFailed("GitHub CLI not found. Install with: brew install gh".to_string())
    })?;

    let output = Command::new(&gh_path)
        .args(["auth", "token"])
        .output()
        .map_err(|e| GitError::CommandFailed(format!("Failed to run gh: {}", e)))?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            Err(GitError::CommandFailed(
                "GitHub CLI returned empty token. Run: gh auth login".to_string(),
            ))
        } else {
            Ok(token)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not logged in") || stderr.contains("no oauth token") {
            Err(GitError::CommandFailed(
                "Not authenticated with GitHub CLI. Run: gh auth login".to_string(),
            ))
        } else {
            Err(GitError::CommandFailed(format!(
                "GitHub CLI error: {}",
                stderr.trim()
            )))
        }
    }
}

/// Get the GitHub owner/repo from the repo's origin remote.
fn get_github_repo(repo: &Path) -> Result<(String, String), GitError> {
    use super::cli;

    let url = cli::run(repo, &["remote", "get-url", "origin"])?;
    let url = url.trim();

    // Parse SSH format: git@github.com:owner/repo.git
    // Also handles org-*@github.com:owner/repo.git (GitHub App installs)
    if url.contains("github.com:") {
        if let Some(idx) = url.find("github.com:") {
            let after = &url[idx + "github.com:".len()..];
            let path = after.strip_suffix(".git").unwrap_or(after);
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() == 2 {
                return Ok((parts[0].to_string(), parts[1].to_string()));
            }
        }
    }

    // Parse HTTPS format: https://github.com/owner/repo.git
    if url.contains("github.com/") {
        if let Some(idx) = url.find("github.com/") {
            let after = &url[idx + "github.com/".len()..];
            let path = after.strip_suffix(".git").unwrap_or(after);
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() >= 2 {
                return Ok((parts[0].to_string(), parts[1].to_string()));
            }
        }
    }

    Err(GitError::CommandFailed(format!(
        "Could not parse GitHub repo from origin URL: {}",
        url
    )))
}

/// Comment for creating a review (request body format).
#[derive(Debug, Serialize)]
struct GitHubReviewComment {
    path: String,
    body: String,
    line: u32,
    side: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_side: Option<&'static str>,
}

/// Request body for creating a review.
#[derive(Debug, Serialize)]
struct CreateReviewRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<String>,
    comments: Vec<GitHubReviewComment>,
}

/// Response from creating a review.
#[derive(Debug, Deserialize)]
struct CreateReviewResponse {
    #[allow(dead_code)]
    id: u64,
    html_url: String,
}

/// A review on GitHub (from list reviews endpoint).
#[derive(Debug, Deserialize)]
struct GitHubReview {
    id: u64,
    state: String,
    user: GhUser,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

/// A comment that couldn't be placed on a specific line (outside the diff).
struct OutOfDiffComment {
    path: String,
    line_info: String,
    content: String,
}

/// Convert a local Comment to a GitHub review comment.
///
/// If `valid_lines` is provided, checks if the comment's lines are within the diff.
/// Returns Err for comments outside the diff (they'll be added to the review body).
fn convert_comment(
    comment: &Comment,
    valid_lines: Option<&std::collections::HashSet<u32>>,
) -> std::result::Result<GitHubReviewComment, OutOfDiffComment> {
    // Convert 0-indexed span to 1-indexed line numbers
    let line = comment.span.end; // end line (1-indexed, since end is exclusive)
    let start_line = comment.span.start + 1; // start line (1-indexed)

    // Check if this line is within the diff
    let line_in_diff = valid_lines
        .map(|lines| lines.contains(&line))
        .unwrap_or(true);

    if line_in_diff {
        // For single-line comments, don't use start_line
        let is_multiline = comment.span.end > comment.span.start + 1;

        Ok(GitHubReviewComment {
            path: comment.path.clone(),
            body: comment.content.clone(),
            line,
            side: "RIGHT", // Always RIGHT since we only support comments on new code
            start_line: if is_multiline { Some(start_line) } else { None },
            start_side: if is_multiline { Some("RIGHT") } else { None },
        })
    } else {
        let line_info = if comment.span.end > comment.span.start + 1 {
            format!("Lines {}-{}", start_line, line)
        } else {
            format!("Line {}", line)
        };

        Err(OutOfDiffComment {
            path: comment.path.clone(),
            line_info,
            content: comment.content.clone(),
        })
    }
}

/// Fetch the valid line numbers for each file in a PR diff.
/// Returns a map of file path -> set of valid line numbers (1-indexed, RIGHT side).
async fn fetch_pr_diff_lines(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<std::collections::HashMap<String, std::collections::HashSet<u32>>, GitError> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/files",
        owner, repo, pr_number
    );

    log::info!("Fetching PR files from: {}", url);

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to fetch PR files: {}", e)))?;

    if !response.status().is_success() {
        return Err(GitError::CommandFailed(format!(
            "Failed to fetch PR files from {}/{} PR #{}: {}",
            owner,
            repo,
            pr_number,
            response.status()
        )));
    }

    #[derive(Deserialize)]
    struct PullRequestFile {
        filename: String,
        patch: Option<String>,
    }

    let files: Vec<PullRequestFile> = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse PR files: {}", e)))?;

    let mut result = std::collections::HashMap::new();

    for file in files {
        let mut valid_lines = std::collections::HashSet::new();

        if let Some(patch) = &file.patch {
            // Parse the unified diff to extract valid line numbers
            let mut current_line: u32 = 0;

            for line in patch.lines() {
                if line.starts_with("@@") {
                    // Parse hunk header: @@ -X,Y +Z,W @@
                    if let Some(plus_pos) = line.find('+') {
                        let after_plus = &line[plus_pos + 1..];
                        if let Some(comma_or_space) = after_plus.find([',', ' ']) {
                            if let Ok(start) = after_plus[..comma_or_space].parse::<u32>() {
                                current_line = start;
                            }
                        }
                    }
                } else if line.starts_with('-') {
                    // Deleted line - doesn't increment new file line number
                } else if line.starts_with('+') || !line.starts_with('\\') {
                    // Added line or context line - valid for RIGHT side comments
                    valid_lines.insert(current_line);
                    current_line += 1;
                }
            }
        }

        result.insert(file.filename, valid_lines);
    }

    Ok(result)
}

/// Get the current authenticated user's login.
async fn get_current_user(client: &reqwest::Client, token: &str) -> Result<String, GitError> {
    let response = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to get current user: {}", e)))?;

    if !response.status().is_success() {
        return Err(GitError::CommandFailed(format!(
            "Failed to get current user: {}",
            response.status()
        )));
    }

    let user: GhUser = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse user response: {}", e)))?;

    Ok(user.login)
}

/// Find an existing pending review by the current user.
async fn find_pending_review(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
    username: &str,
) -> Result<Option<GitHubReview>, GitError> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/reviews",
        owner, repo, pr_number
    );

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to list reviews: {}", e)))?;

    if !response.status().is_success() {
        return Err(GitError::CommandFailed(format!(
            "Failed to list reviews: {}",
            response.status()
        )));
    }

    let reviews: Vec<GitHubReview> = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse reviews: {}", e)))?;

    Ok(reviews
        .into_iter()
        .find(|r| r.state == "PENDING" && r.user.login == username))
}

/// Delete a pending review.
async fn delete_pending_review(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
    review_id: u64,
) -> Result<(), GitError> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/reviews/{}",
        owner, repo, pr_number, review_id
    );

    let response = client
        .delete(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to delete review: {}", e)))?;

    if !response.status().is_success() {
        return Err(GitError::CommandFailed(format!(
            "Failed to delete pending review: {}",
            response.status()
        )));
    }

    Ok(())
}

/// Sync local comments to a GitHub PR as a pending review.
///
/// This will:
/// 1. Delete any existing pending review by the current user
/// 2. Create a new pending review with all comments
/// 3. Return the URL to the review
pub async fn sync_review_to_github(
    repo: &Path,
    pr_number: u64,
    comments: &[Comment],
) -> Result<GitHubSyncResult, GitError> {
    if comments.is_empty() {
        return Err(GitError::CommandFailed("No comments to sync".to_string()));
    }

    let token = get_github_token()?;
    let (owner, repo_name) = get_github_repo(repo)?;
    log::info!(
        "Syncing {} comments to GitHub PR #{} in {}/{}",
        comments.len(),
        pr_number,
        owner,
        repo_name
    );
    let client = reqwest::Client::new();

    // Get current user
    let username = get_current_user(&client, &token).await?;

    // Fetch valid diff lines for each file
    let valid_lines_by_file =
        fetch_pr_diff_lines(&client, &token, &owner, &repo_name, pr_number).await?;

    // Check for existing pending review and delete it
    if let Some(existing) =
        find_pending_review(&client, &token, &owner, &repo_name, pr_number, &username).await?
    {
        log::info!("Deleting existing pending review {}", existing.id);
        delete_pending_review(&client, &token, &owner, &repo_name, pr_number, existing.id).await?;
    }

    // Convert comments to GitHub format, checking against valid lines
    let mut gh_comments: Vec<GitHubReviewComment> = Vec::new();
    let mut out_of_diff_comments: Vec<OutOfDiffComment> = Vec::new();

    for comment in comments {
        match convert_comment(comment, valid_lines_by_file.get(&comment.path)) {
            Ok(gh_comment) => gh_comments.push(gh_comment),
            Err(out_of_diff) => out_of_diff_comments.push(out_of_diff),
        }
    }

    let comment_count = gh_comments.len() + out_of_diff_comments.len();

    // Build review body from out-of-diff comments
    let review_body = if out_of_diff_comments.is_empty() {
        None
    } else {
        let mut body = String::from("### Comments on lines outside the diff\n\n");
        for ooc in &out_of_diff_comments {
            body.push_str(&format!(
                "**{}** ({})\n\n{}\n\n---\n\n",
                ooc.path, ooc.line_info, ooc.content
            ));
        }
        Some(body)
    };

    // Create new pending review
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/reviews",
        owner, repo_name, pr_number
    );

    let request = CreateReviewRequest {
        body: review_body,
        event: None, // None = PENDING
        comments: gh_comments,
    };

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&request)
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to create review: {}", e)))?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(GitError::CommandFailed(format!(
            "Failed to create review: {} - {}",
            status, error_body
        )));
    }

    let review: CreateReviewResponse = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse review response: {}", e)))?;

    Ok(GitHubSyncResult {
        review_url: review.html_url,
        comment_count,
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_github_auth_returns_status() {
        // This test just verifies the function runs without panicking
        // Actual auth status depends on the environment
        let status = check_github_auth();
        // Either authenticated or has a setup hint
        assert!(status.authenticated || status.setup_hint.is_some());
    }
}
