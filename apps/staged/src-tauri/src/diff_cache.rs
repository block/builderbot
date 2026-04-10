//! On-disk cache for branch and commit diff results.
//!
//! Each branch gets a directory under the project's data directory:
//! `~/.staged/workspaces/<local|remote>/projects/<project_id>/diff-cache/<branch_id>/`
//!
//! Branch-level diffs live under `branch/<head_sha>/`:
//! ```text
//! branch/<head_sha>/
//! ├── index.json           # CachedBranchIndex (file list + metadata)
//! └── diffs/
//!     ├── <path_hash>.json # FileDiff for one file
//!     └── ...
//! ```
//!
//! Commit-level diffs live under `commits/<commit_sha>/`:
//! ```text
//! commits/<commit_sha>/
//! ├── index.json           # CachedCommitIndex (file list + metadata)
//! └── diffs/
//!     ├── <path_hash>.json # FileDiff for one file
//!     └── ...
//! ```

use crate::store::{ProjectLocation, Store};
use anyhow::Result;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

/// Result of collecting diffs for a branch: the branch index, branch-level
/// file diffs, and per-commit indices with their file diffs.
pub type CollectedDiffs = (
    CachedBranchIndex,
    HashMap<String, git_diff::FileDiff>,
    Vec<(CachedCommitIndex, HashMap<String, git_diff::FileDiff>)>,
);
use std::sync::Arc;

/// Index for a cached branch diff at a specific revision.
///
/// Contains only metadata and the file list — individual file diffs are stored
/// separately under `diffs/<path_hash>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedBranchIndex {
    pub branch_id: String,
    /// Merge-base SHA between the branch and its base.
    pub base_sha: String,
    /// Branch tip SHA.
    pub head_sha: String,
    /// Changed file list.
    pub files: Vec<git_diff::FileDiffSummary>,
    /// Unix timestamp (seconds since epoch) of when this cache entry was created.
    pub cached_at: u64,
}

/// Index for a cached commit diff.
///
/// Contains only metadata and the file list — individual file diffs are stored
/// separately under `diffs/<path_hash>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCommitIndex {
    /// The commit SHA this cache entry is for.
    pub sha: String,
    /// Parent commit SHA.
    pub parent_sha: String,
    /// Changed file list.
    pub files: Vec<git_diff::FileDiffSummary>,
    /// Unix timestamp (seconds since epoch) of when this cache entry was created.
    pub cached_at: u64,
}

/// Returns the cache directory for a given branch:
/// `~/.staged/workspaces/<local|remote>/projects/<project_id>/diff-cache/<branch_id>/`
fn cache_dir(location: ProjectLocation, project_id: &str, branch_id: &str) -> Option<PathBuf> {
    crate::paths::project_data_dir(location, project_id)
        .map(|d| d.join("diff-cache").join(branch_id))
}

/// Returns the branch-level cache directory:
/// `<cache_dir>/branch/`
fn branch_cache_dir(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
) -> Option<PathBuf> {
    cache_dir(location, project_id, branch_id).map(|d| d.join("branch"))
}

/// Returns the commit-level cache directory:
/// `<cache_dir>/commits/`
fn commits_cache_dir(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
) -> Option<PathBuf> {
    cache_dir(location, project_id, branch_id).map(|d| d.join("commits"))
}

/// Produce a filesystem-safe key from a file path using SHA-256.
fn path_to_cache_key(path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Write an index JSON file and per-file diffs into a cache directory.
///
/// Creates `sha_dir/index.json` and `sha_dir/diffs/<path_hash>.json` atomically
/// (via temp file + rename).
fn write_cache_dir(
    sha_dir: &std::path::Path,
    index_json: &str,
    file_diffs: &HashMap<String, git_diff::FileDiff>,
) -> Result<()> {
    let diffs_dir = sha_dir.join("diffs");
    std::fs::create_dir_all(&diffs_dir)?;

    // Write index.json atomically.
    let index_path = sha_dir.join("index.json");
    let tmp_index = sha_dir.join("index.json.tmp");
    std::fs::write(&tmp_index, index_json)?;
    std::fs::rename(&tmp_index, &index_path)?;

    // Write each file diff atomically.
    for (path, diff) in file_diffs {
        let key = path_to_cache_key(path);
        let diff_path = diffs_dir.join(format!("{key}.json"));
        let tmp_diff = diffs_dir.join(format!("{key}.json.tmp"));
        std::fs::write(&tmp_diff, serde_json::to_string(diff)?)?;
        std::fs::rename(&tmp_diff, &diff_path)?;
    }

    Ok(())
}

/// Persist a branch cache entry to disk under `<cache_dir>/branch/<head_sha>/`.
pub fn save_cache(
    location: ProjectLocation,
    project_id: &str,
    index: &CachedBranchIndex,
    file_diffs: &HashMap<String, git_diff::FileDiff>,
) -> Result<()> {
    let sha_dir = branch_cache_dir(location, project_id, &index.branch_id)
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory"))?
        .join(&index.head_sha);
    write_cache_dir(&sha_dir, &serde_json::to_string(index)?, file_diffs)
}

/// Load the cache index for a branch at a given head SHA.
/// Returns `None` if the index file is missing or unreadable.
pub fn load_cache_index(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    head_sha: &str,
) -> Option<CachedBranchIndex> {
    let path = branch_cache_dir(location, project_id, branch_id)?
        .join(head_sha)
        .join("index.json");
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Load a single cached file diff by path.
/// Returns `None` if the diff file is missing or unreadable.
pub fn load_cached_file_diff(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    head_sha: &str,
    file_path: &str,
) -> Option<git_diff::FileDiff> {
    let key = path_to_cache_key(file_path);
    let path = branch_cache_dir(location, project_id, branch_id)?
        .join(head_sha)
        .join("diffs")
        .join(format!("{key}.json"));
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Remove all cached SHA directories for a branch except the one for `keep_sha`.
pub fn cleanup_old_caches(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    keep_sha: &str,
) -> Result<()> {
    let dir = branch_cache_dir(location, project_id, branch_id)
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory"))?;
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log::warn!("Diff cache: failed to read cache directory entry: {e}");
                continue;
            }
        };
        let is_dir = match entry.file_type() {
            Ok(ft) => ft.is_dir(),
            Err(e) => {
                log::warn!(
                    "Diff cache: failed to read file type for {:?}: {e}",
                    entry.path()
                );
                continue;
            }
        };
        if is_dir && entry.file_name() != *keep_sha {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                log::warn!(
                    "Diff cache: failed to remove stale cache {:?}: {e}",
                    entry.path()
                );
            }
        }
    }
    Ok(())
}

/// Delete the entire cache directory for a branch.
pub fn delete_branch_cache(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
) -> Result<()> {
    let dir = cache_dir(location, project_id, branch_id)
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory"))?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Persist a commit cache entry to disk under `<cache_dir>/commits/<commit_sha>/`.
pub fn save_commit_cache(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    index: &CachedCommitIndex,
    file_diffs: &HashMap<String, git_diff::FileDiff>,
) -> Result<()> {
    let sha_dir = commits_cache_dir(location, project_id, branch_id)
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory"))?
        .join(&index.sha);
    write_cache_dir(&sha_dir, &serde_json::to_string(index)?, file_diffs)
}

/// Check whether a commit cache entry exists on disk.
pub fn has_commit_cache(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    commit_sha: &str,
) -> bool {
    commits_cache_dir(location, project_id, branch_id)
        .map(|d| d.join(commit_sha).join("index.json").exists())
        .unwrap_or(false)
}

/// Load the cache index for a specific commit.
/// Returns `None` if the index file is missing or unreadable.
pub fn load_commit_index(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    commit_sha: &str,
) -> Option<CachedCommitIndex> {
    let path = commits_cache_dir(location, project_id, branch_id)?
        .join(commit_sha)
        .join("index.json");
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Load a single cached file diff from a commit cache.
/// Returns `None` if the diff file is missing or unreadable.
pub fn load_commit_file_diff(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    commit_sha: &str,
    file_path: &str,
) -> Option<git_diff::FileDiff> {
    let key = path_to_cache_key(file_path);
    let path = commits_cache_dir(location, project_id, branch_id)?
        .join(commit_sha)
        .join("diffs")
        .join(format!("{key}.json"));
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Remove commit cache directories whose SHA is not in `keep_shas`.
///
/// This keeps only commits that are currently on the branch.
pub fn cleanup_old_commit_caches(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    keep_shas: &[String],
) -> Result<()> {
    let dir = commits_cache_dir(location, project_id, branch_id)
        .ok_or_else(|| anyhow::anyhow!("could not determine cache directory"))?;
    if !dir.exists() {
        return Ok(());
    }
    let keep_set: std::collections::HashSet<&str> = keep_shas.iter().map(|s| s.as_str()).collect();
    for entry in std::fs::read_dir(&dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log::warn!("Diff cache: failed to read commit cache directory entry: {e}");
                continue;
            }
        };
        let is_dir = match entry.file_type() {
            Ok(ft) => ft.is_dir(),
            Err(e) => {
                log::warn!(
                    "Diff cache: failed to read file type for {:?}: {e}",
                    entry.path()
                );
                continue;
            }
        };
        if is_dir {
            let name = entry.file_name();
            if !keep_set.contains(name.to_string_lossy().as_ref()) {
                if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                    log::warn!(
                        "Diff cache: failed to remove stale commit cache {:?}: {e}",
                        entry.path()
                    );
                }
            }
        }
    }
    Ok(())
}

/// JSON output from the remote collection script.
#[derive(Deserialize)]
struct CollectScriptOutput {
    head: String,
    base: String,
    files: Vec<CollectScriptFile>,
    #[serde(default)]
    commits: Vec<CollectScriptCommit>,
}

/// A single file entry in the collection script output.
#[derive(Deserialize)]
#[allow(dead_code)]
struct CollectScriptFile {
    status: String,
    before_path: String,
    after_path: String,
    before_content_b64: Option<String>,
    after_content_b64: Option<String>,
    patch: String,
}

/// A single commit entry in the collection script output.
#[derive(Deserialize)]
struct CollectScriptCommit {
    sha: String,
    parent: String,
    files: Vec<CollectScriptFile>,
}

/// Escape a string for embedding inside single-quoted bash strings.
fn shell_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Build a bash script that collects all diff data in a single remote execution.
///
/// The script resolves HEAD / merge-base, lists changed files, fetches content
/// (base64-encoded) for both sides of every file, and collects the unified diff
/// hunks. Everything is emitted as a single JSON object via `jq`.
fn build_collect_script(
    repo_subpath: Option<&str>,
    base_branch: &str,
    head_sha: &str,
    commit_shas: &[String],
) -> String {
    let escaped_base = shell_escape(base_branch);
    let escaped_head = shell_escape(head_sha);

    let commit_shas_bash = commit_shas
        .iter()
        .map(|s| format!("'{}'", shell_escape(s)))
        .collect::<Vec<_>>()
        .join(" ");

    let git_fn = match repo_subpath.map(str::trim).filter(|s| !s.is_empty()) {
        Some(subpath) => {
            let escaped = shell_escape(subpath);
            format!(
                r#"RP='{escaped}'
case "$RP" in home:*) RP="$HOME/${{RP#home:}}" ;; esac
g() {{ git -C "$RP" "$@"; }}"#
            )
        }
        None => r#"g() { git "$@"; }"#.to_string(),
    };

    format!(
        r##"#!/bin/bash
set -e
{git_fn}
BB='{escaped_base}'
HD='{escaped_head}'
MB=$(g merge-base "$BB" "$HD")
TD=$(mktemp -d)
trap 'rm -rf "$TD"' EXIT
cf() {{
while IFS= read -r -d '' sf; do
  sc="${{sf:0:1}}"
  case "$sc" in
    A) IFS= read -r -d '' ap; bp="";;
    D) IFS= read -r -d '' bp; ap="";;
    M|T) IFS= read -r -d '' p; bp="$p"; ap="$p";;
    R|C) IFS= read -r -d '' bp; IFS= read -r -d '' ap;;
    *) IFS= read -r -d '' _; continue;;
  esac
  if [ -n "$bp" ] && g cat-file -e "$MB:$bp" 2>/dev/null; then
    g show "$MB:$bp" | base64 | tr -d '\n' > "$TD/b"
  else
    : > "$TD/b"
  fi
  if [ -n "$ap" ] && g cat-file -e "$HD:$ap" 2>/dev/null; then
    g show "$HD:$ap" | base64 | tr -d '\n' > "$TD/a"
  else
    : > "$TD/a"
  fi
  if [ -n "$ap" ] && [ "$ap" != "$bp" ] && [ -n "$bp" ]; then
    g -c color.ui=never diff --unified=0 "$MB" "$HD" -- "$bp" "$ap" > "$TD/p" 2>/dev/null || true
  else
    dp="${{ap:-$bp}}"
    g -c color.ui=never diff --unified=0 "$MB" "$HD" -- "$dp" > "$TD/p" 2>/dev/null || true
  fi
  jq -nc \
    --arg s "$sc" --arg bp "$bp" --arg ap "$ap" \
    --rawfile bb "$TD/b" --rawfile ab "$TD/a" --rawfile p "$TD/p" \
    '{{status:$s,before_path:$bp,after_path:$ap,before_content_b64:(if($bb|length)==0 then null else $bb end),after_content_b64:(if($ab|length)==0 then null else $ab end),patch:$p}}'
done < <(g diff --name-status -z "$MB" "$HD")
}}
cf | jq -s '.' > "$TD/bf.json"
BR_HD="$HD" BR_MB="$MB"
CS=({commit_shas_bash})
: > "$TD/ca_parts.json"
for cs in "${{CS[@]}}"; do
  pp=$(g rev-parse "$cs^" 2>/dev/null) || continue
  MB="$pp" HD="$cs" cf | jq -s '.' > "$TD/cf.json"
  jq -nc --arg s "$cs" --arg p "$pp" --slurpfile f "$TD/cf.json" '{{sha:$s,parent:$p,files:$f[0]}}' >> "$TD/ca_parts.json"
done
jq -s '.' "$TD/ca_parts.json" > "$TD/ca.json"
jq -nc --arg head "$BR_HD" --arg base "$BR_MB" --slurpfile files "$TD/bf.json" --slurpfile commits "$TD/ca.json" '{{head:$head,base:$base,files:$files[0],commits:$commits[0]}}'
"##
    )
}

/// Decode base64-encoded file content into a `git_diff::File`.
fn decode_file(b64: &str, path: &str) -> git_diff::File {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("Diff cache: base64 decode failed for {path}: {e}");
            Vec::new()
        }
    };
    let content = crate::diff_commands::file_content_from_bytes(&bytes, path);
    git_diff::File {
        path: path.to_string(),
        content,
    }
}

/// Parse file entries from the collection script into summaries and full diffs.
fn parse_script_files(
    entries: &[CollectScriptFile],
) -> (
    Vec<git_diff::FileDiffSummary>,
    HashMap<String, git_diff::FileDiff>,
) {
    use crate::diff_commands::{compute_remote_alignments, parse_unified_hunks};

    let mut files = Vec::new();
    let mut file_diffs = HashMap::new();

    for entry in entries {
        let before_path = if entry.before_path.is_empty() {
            None
        } else {
            Some(entry.before_path.clone())
        };
        let after_path = if entry.after_path.is_empty() {
            None
        } else {
            Some(entry.after_path.clone())
        };

        files.push(git_diff::FileDiffSummary {
            before: before_path.as_deref().map(PathBuf::from),
            after: after_path.as_deref().map(PathBuf::from),
        });

        let canonical_path = after_path.as_deref().or(before_path.as_deref());
        let canonical_path = match canonical_path {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };

        let before = entry.before_content_b64.as_deref().map(|b64| {
            let path = before_path.as_deref().unwrap_or(canonical_path);
            decode_file(b64, path)
        });

        let after = entry.after_content_b64.as_deref().map(|b64| {
            let path = after_path.as_deref().unwrap_or(canonical_path);
            decode_file(b64, path)
        });

        let hunks = parse_unified_hunks(&entry.patch);
        let alignments = compute_remote_alignments(&hunks, &before, &after);

        file_diffs.insert(
            canonical_path.to_string(),
            git_diff::FileDiff {
                before,
                after,
                alignments,
            },
        );
    }

    (files, file_diffs)
}

/// Collect all diff data for a remote branch in a single remote round-trip.
///
/// Sends a bash script to the remote workspace that gathers HEAD, merge-base,
/// changed file list, file contents (base64), and unified diffs, returning
/// everything as a JSON payload. The result is parsed client-side into the
/// same `(CachedBranchIndex, HashMap<String, FileDiff>)` format used by the
/// on-disk cache.
pub fn collect_branch_diff(
    workspace_name: &str,
    repo_subpath: Option<&str>,
    base_branch: &str,
    branch_id: &str,
    head_sha: &str,
    commit_shas: &[String],
) -> Result<CollectedDiffs> {
    let start = std::time::Instant::now();
    log::info!(
        "collect_branch_diff: branch={branch_id} workspace={workspace_name} commits={}",
        commit_shas.len()
    );
    // Execute the collection script in a single remote call.
    let script = build_collect_script(repo_subpath, base_branch, head_sha, commit_shas);
    let t_exec = std::time::Instant::now();
    let output = crate::blox::ws_exec(workspace_name, &["bash", "-c", &script])
        .map_err(|e| anyhow::anyhow!("remote diff collection script failed: {e}"))?;
    log::info!(
        "collect_branch_diff: remote exec done in {:?}, output size={} bytes",
        t_exec.elapsed(),
        output.len()
    );

    let t_parse = std::time::Instant::now();
    let result: CollectScriptOutput = serde_json::from_str(&output)
        .map_err(|e| anyhow::anyhow!("failed to parse remote diff collection output: {e}"))?;
    log::info!(
        "collect_branch_diff: parsed JSON in {:?}, files={} commits={}",
        t_parse.elapsed(),
        result.files.len(),
        result.commits.len()
    );

    let t_process = std::time::Instant::now();
    let (files, file_diffs) = parse_script_files(&result.files);

    let cached_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let index = CachedBranchIndex {
        branch_id: branch_id.to_string(),
        base_sha: result.base,
        head_sha: result.head,
        files,
        cached_at,
    };

    log::info!(
        "collect_branch_diff: processed file diffs in {:?}",
        t_process.elapsed()
    );

    // Parse commit-level diffs.
    let mut commit_results = Vec::new();
    for commit in &result.commits {
        let (commit_files, commit_file_diffs) = parse_script_files(&commit.files);
        let commit_index = CachedCommitIndex {
            sha: commit.sha.clone(),
            parent_sha: commit.parent.clone(),
            files: commit_files,
            cached_at,
        };
        commit_results.push((commit_index, commit_file_diffs));
    }

    log::info!(
        "collect_branch_diff: total {:?}, {} file diffs, {} commits",
        start.elapsed(),
        file_diffs.len(),
        commit_results.len()
    );

    Ok((index, file_diffs, commit_results))
}

/// Collect and cache diffs for a remote branch synchronously.
///
/// Executes the collection script, saves branch and commit caches to disk,
/// cleans up stale entries, and returns the results so the caller can serve
/// them immediately.
#[allow(clippy::too_many_arguments)]
pub fn collect_and_cache(
    location: ProjectLocation,
    project_id: &str,
    branch_id: &str,
    workspace_name: &str,
    repo_subpath: Option<&str>,
    base_branch: &str,
    head_sha: &str,
    commit_shas: &[String],
) -> Result<CollectedDiffs> {
    // Filter to commits not already cached.
    let uncached_shas: Vec<String> = commit_shas
        .iter()
        .filter(|sha| !has_commit_cache(location, project_id, branch_id, sha))
        .cloned()
        .collect();

    let (index, file_diffs, commit_results) = collect_branch_diff(
        workspace_name,
        repo_subpath,
        base_branch,
        branch_id,
        head_sha,
        &uncached_shas,
    )?;

    let sha = index.head_sha.clone();
    let num_diffs = file_diffs.len();
    save_cache(location, project_id, &index, &file_diffs)?;

    if let Err(e) = cleanup_old_caches(location, project_id, branch_id, &sha) {
        log::warn!("Diff cache: cleanup failed: {e}");
    }

    // Save commit-level caches.
    let num_commits = commit_results.len();
    for (commit_index, commit_diffs) in &commit_results {
        if let Err(e) =
            save_commit_cache(location, project_id, branch_id, commit_index, commit_diffs)
        {
            log::warn!(
                "Diff cache: failed to save commit cache for {}: {e}",
                &commit_index.sha[..7.min(commit_index.sha.len())]
            );
        }
    }

    // Clean up commit caches not in the current set.
    if let Err(e) = cleanup_old_commit_caches(location, project_id, branch_id, commit_shas) {
        log::warn!("Diff cache: commit cache cleanup failed: {e}");
    }

    log::info!(
        "Diff cache: cached {} file diffs + {} commits for branch {} at {}",
        num_diffs,
        num_commits,
        branch_id,
        &sha[..7.min(sha.len())]
    );

    Ok((index, file_diffs, commit_results))
}

/// Spawn a background thread that collects and caches the full branch diff.
///
/// Looks up the branch and project from the store, collects the diff via
/// remote git calls, saves it to disk, and cleans up stale cache entries.
/// Failures are logged but never propagated — the caller can fire and forget.
pub fn spawn_cache_branch_diff(
    store: Arc<Store>,
    branch_id: String,
    workspace_name: String,
    head_sha: String,
    commit_shas: Vec<String>,
) {
    std::thread::spawn(move || {
        if let Err(e) = cache_branch_diff_background(
            &store,
            &branch_id,
            &workspace_name,
            &head_sha,
            &commit_shas,
        ) {
            log::error!("Diff cache: collection failed for branch {branch_id}: {e}");
        }
    });
}

fn cache_branch_diff_background(
    store: &Arc<Store>,
    branch_id: &str,
    workspace_name: &str,
    head_sha: &str,
    commit_shas: &[String],
) -> Result<()> {
    let branch = store
        .get_branch(branch_id)?
        .ok_or_else(|| anyhow::anyhow!("branch not found: {branch_id}"))?;

    let project = store
        .get_project(&branch.project_id)?
        .ok_or_else(|| anyhow::anyhow!("project not found: {}", branch.project_id))?;

    let repo_subpath = match crate::branches::resolve_branch_clone_dir(store, &branch) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Diff cache: failed to resolve clone dir: {e}");
            None
        }
    };

    collect_and_cache(
        project.location,
        &project.id,
        branch_id,
        workspace_name,
        repo_subpath.as_deref(),
        &branch.base_branch,
        head_sha,
        commit_shas,
    )?;

    Ok(())
}
