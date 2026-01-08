//! GitHub integration for fetching pull requests.
//!
//! Uses the GitHub CLI (`gh`) for authentication and the GitHub REST API
//! for fetching PR data. Includes caching to minimize API calls.

use git2::Repository;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;
use std::sync::RwLock;
use std::time::{Duration, Instant};

// =============================================================================
// Types
// =============================================================================

/// A GitHub pull request with the fields we care about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub author: String,
    pub base_ref: String,
    pub head_ref: String,
    pub head_sha: String,
    pub draft: bool,
    pub additions: u32,
    pub deletions: u32,
    pub updated_at: String,
}

/// Result of checking GitHub authentication status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAuthStatus {
    pub authenticated: bool,
    /// If not authenticated, instructions for setting up.
    pub setup_hint: Option<String>,
}

/// GitHub repository identifier (owner and repo name).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitHubRepo {
    pub owner: String,
    pub name: String,
}

/// Cached PR list with expiration.
struct CachedPRList {
    prs: Vec<PullRequest>,
    fetched_at: Instant,
}

/// Error type for GitHub operations.
#[derive(Debug)]
pub struct GitHubError(pub String);

impl std::fmt::Display for GitHubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitHubError {}

type Result<T> = std::result::Result<T, GitHubError>;

// =============================================================================
// Cache
// =============================================================================

/// How long to cache PR lists before they're considered stale.
const CACHE_TTL: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// Global cache for PR lists, keyed by "owner/repo".
static PR_CACHE: RwLock<Option<HashMap<String, CachedPRList>>> = RwLock::new(None);

fn cache_key(repo: &GitHubRepo) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

fn get_cached_prs(repo: &GitHubRepo) -> Option<Vec<PullRequest>> {
    let cache = PR_CACHE.read().ok()?;
    let cache = cache.as_ref()?;
    let entry = cache.get(&cache_key(repo))?;

    // Check if cache is still valid
    if entry.fetched_at.elapsed() < CACHE_TTL {
        Some(entry.prs.clone())
    } else {
        None
    }
}

fn set_cached_prs(repo: &GitHubRepo, prs: Vec<PullRequest>) {
    let mut cache = match PR_CACHE.write() {
        Ok(c) => c,
        Err(_) => return,
    };

    let cache = cache.get_or_insert_with(HashMap::new);
    cache.insert(
        cache_key(repo),
        CachedPRList {
            prs,
            fetched_at: Instant::now(),
        },
    );
}

/// Clear the cache for a specific repo, forcing a fresh fetch.
pub fn invalidate_cache(repo: &GitHubRepo) {
    if let Ok(mut cache) = PR_CACHE.write() {
        if let Some(ref mut map) = *cache {
            map.remove(&cache_key(repo));
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
///
/// First tries the bare command (works if already in PATH), then checks common locations.
fn find_gh_command() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    // First, check if `gh` is directly available (e.g., already in PATH)
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

/// Get the GitHub token from `gh auth token`.
///
/// Returns the token if authenticated, or an error with setup instructions.
pub fn get_github_token() -> Result<String> {
    let gh_path = find_gh_command().ok_or_else(|| {
        GitHubError("GitHub CLI not found. Install it with: brew install gh".to_string())
    })?;

    let output = Command::new(&gh_path)
        .args(["auth", "token"])
        .output()
        .map_err(|e| GitHubError(format!("Failed to run gh: {}", e)))?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            Err(GitHubError(
                "GitHub CLI returned empty token. Run: gh auth login".to_string(),
            ))
        } else {
            Ok(token)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not logged in") || stderr.contains("no oauth token") {
            Err(GitHubError(
                "Not authenticated with GitHub CLI. Run: gh auth login".to_string(),
            ))
        } else {
            Err(GitHubError(format!("GitHub CLI error: {}", stderr.trim())))
        }
    }
}

/// Check if the user is authenticated with GitHub CLI.
pub fn check_github_auth() -> GitHubAuthStatus {
    match get_github_token() {
        Ok(_) => GitHubAuthStatus {
            authenticated: true,
            setup_hint: None,
        },
        Err(e) => GitHubAuthStatus {
            authenticated: false,
            setup_hint: Some(e.0),
        },
    }
}

// =============================================================================
// Repository Detection
// =============================================================================

/// Extract GitHub owner/repo from a git remote URL.
///
/// Handles formats:
/// - `git@github.com:owner/repo.git`
/// - `https://github.com/owner/repo.git`
/// - `https://github.com/owner/repo`
pub fn parse_github_url(url: &str) -> Option<GitHubRepo> {
    // SSH format: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let path = rest.strip_suffix(".git").unwrap_or(rest);
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() == 2 {
            return Some(GitHubRepo {
                owner: parts[0].to_string(),
                name: parts[1].to_string(),
            });
        }
    }

    // HTTPS format: https://github.com/owner/repo.git
    if url.contains("github.com") {
        let url = url.strip_suffix(".git").unwrap_or(url);
        // Find github.com and take the next two path segments
        if let Some(idx) = url.find("github.com") {
            let after = &url[idx + "github.com".len()..];
            let path = after.strip_prefix('/').unwrap_or(after);
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() >= 2 {
                return Some(GitHubRepo {
                    owner: parts[0].to_string(),
                    name: parts[1].to_string(),
                });
            }
        }
    }

    None
}

/// Get the GitHub repo info from a git repository's remotes.
///
/// Checks "origin" first, then falls back to any GitHub remote.
pub fn get_github_remote(repo: &Repository) -> Option<GitHubRepo> {
    // Try origin first
    if let Ok(remote) = repo.find_remote("origin") {
        if let Some(url) = remote.url() {
            if let Some(gh_repo) = parse_github_url(url) {
                return Some(gh_repo);
            }
        }
    }

    // Fall back to any GitHub remote
    if let Ok(remotes) = repo.remotes() {
        for name in remotes.iter().flatten() {
            if let Ok(remote) = repo.find_remote(name) {
                if let Some(url) = remote.url() {
                    if let Some(gh_repo) = parse_github_url(url) {
                        return Some(gh_repo);
                    }
                }
            }
        }
    }

    None
}

// =============================================================================
// GitHub API
// =============================================================================

/// Response from GitHub API for a single PR.
/// Note: additions/deletions are NOT included in the list endpoint.
#[derive(Debug, Deserialize)]
struct GitHubPRResponse {
    number: u32,
    title: String,
    user: GitHubUser,
    base: GitHubRef,
    head: GitHubRef,
    draft: bool,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRef {
    #[serde(rename = "ref")]
    ref_name: String,
    sha: String,
}

impl From<GitHubPRResponse> for PullRequest {
    fn from(pr: GitHubPRResponse) -> Self {
        PullRequest {
            number: pr.number,
            title: pr.title,
            author: pr.user.login,
            base_ref: pr.base.ref_name,
            head_ref: pr.head.ref_name,
            head_sha: pr.head.sha[..8.min(pr.head.sha.len())].to_string(),
            draft: pr.draft,
            // additions/deletions not available in list endpoint
            additions: 0,
            deletions: 0,
            updated_at: pr.updated_at,
        }
    }
}

/// Fetch open pull requests from GitHub API.
///
/// Uses caching to minimize API calls. Pass `force_refresh` to bypass cache.
pub async fn list_pull_requests(
    gh_repo: &GitHubRepo,
    token: &str,
    force_refresh: bool,
) -> Result<Vec<PullRequest>> {
    // Check cache first (unless forcing refresh)
    if !force_refresh {
        if let Some(cached) = get_cached_prs(gh_repo) {
            log::debug!(
                "Using cached PR list for {}/{}",
                gh_repo.owner,
                gh_repo.name
            );
            return Ok(cached);
        }
    }

    log::info!(
        "Fetching PRs from GitHub API for {}/{}",
        gh_repo.owner,
        gh_repo.name
    );

    let client = reqwest::Client::new();

    // Fetch first page only (50 PRs should be plenty for the selector)
    // Sorted by recently updated to show most relevant first
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls?state=open&sort=updated&direction=desc&per_page=50",
        gh_repo.owner, gh_repo.name
    );

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitHubError(format!("Failed to fetch PRs: {}", e)))?;

    let status = response.status();

    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(GitHubError(
            "Repository not found. Check that it exists and you have access.".to_string(),
        ));
    }

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(GitHubError(
            "GitHub authentication failed. Try: gh auth login".to_string(),
        ));
    }

    if status == reqwest::StatusCode::FORBIDDEN {
        // Check for rate limiting
        let remaining = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u32>().ok());

        if remaining == Some(0) {
            return Err(GitHubError(
                "GitHub API rate limit exceeded. Try again later.".to_string(),
            ));
        }

        return Err(GitHubError(
            "Access forbidden. Check your GitHub permissions.".to_string(),
        ));
    }

    if !status.is_success() {
        return Err(GitHubError(format!(
            "GitHub API error: {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown")
        )));
    }

    let prs: Vec<GitHubPRResponse> = response
        .json()
        .await
        .map_err(|e| GitHubError(format!("Failed to parse PR response: {}", e)))?;

    let prs: Vec<PullRequest> = prs.into_iter().map(Into::into).collect();

    // Cache the result
    set_cached_prs(gh_repo, prs.clone());

    Ok(prs)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_url_ssh() {
        let url = "git@github.com:owner/repo.git";
        let result = parse_github_url(url).unwrap();
        assert_eq!(result.owner, "owner");
        assert_eq!(result.name, "repo");
    }

    #[test]
    fn test_parse_github_url_ssh_no_suffix() {
        let url = "git@github.com:owner/repo";
        let result = parse_github_url(url).unwrap();
        assert_eq!(result.owner, "owner");
        assert_eq!(result.name, "repo");
    }

    #[test]
    fn test_parse_github_url_https() {
        let url = "https://github.com/owner/repo.git";
        let result = parse_github_url(url).unwrap();
        assert_eq!(result.owner, "owner");
        assert_eq!(result.name, "repo");
    }

    #[test]
    fn test_parse_github_url_https_no_suffix() {
        let url = "https://github.com/owner/repo";
        let result = parse_github_url(url).unwrap();
        assert_eq!(result.owner, "owner");
        assert_eq!(result.name, "repo");
    }

    #[test]
    fn test_parse_github_url_not_github() {
        let url = "git@gitlab.com:owner/repo.git";
        assert!(parse_github_url(url).is_none());
    }

    #[test]
    fn test_parse_github_url_invalid() {
        let url = "not a url";
        assert!(parse_github_url(url).is_none());
    }
}
