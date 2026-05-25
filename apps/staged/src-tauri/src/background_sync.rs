//! Tiered background sync service for all locally-cloned repos.
//!
//! Periodically fetches each cloned repo and fast-forwards (or hard-resets)
//! the main checkout to match `origin/<default_branch>`. Fetch frequency
//! is tiered by how actively the repo is being used:
//!
//! | Tier | Criteria                                     | Interval |
//! |------|----------------------------------------------|----------|
//! | Hot  | Pinned, or used by a recently-viewed project | ~5 min   |
//! | Warm | Has at least one project                     | ~1 hour  |
//! | Cold | Local clone but no projects                  | ~4 hours |

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::store::Store;
use crate::web_server::emit_to_all;

// ---------------------------------------------------------------------------
// Tier intervals (milliseconds)
// ---------------------------------------------------------------------------

const HOT_INTERVAL_MS: i64 = 5 * 60 * 1000; // 5 minutes
const WARM_INTERVAL_MS: i64 = 60 * 60 * 1000; // 1 hour
const COLD_INTERVAL_MS: i64 = 4 * 60 * 60 * 1000; // 4 hours

/// How often the tick loop wakes up (seconds).
const TICK_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Tier classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncTier {
    Hot,
    Warm,
    Cold,
}

impl SyncTier {
    fn interval_ms(self) -> i64 {
        match self {
            Self::Hot => HOT_INTERVAL_MS,
            Self::Warm => WARM_INTERVAL_MS,
            Self::Cold => COLD_INTERVAL_MS,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
        }
    }
}

// ---------------------------------------------------------------------------
// Per-repo tracking
// ---------------------------------------------------------------------------

struct RepoSyncState {
    /// Next time we should fetch this repo (ms since epoch).
    next_fetch_at: i64,
    /// Tier as of the last classification.
    tier: SyncTier,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Spawn the background sync loop.
///
/// Should be called once during app setup, after the store is initialised.
/// The loop runs on the Tokio runtime and ticks every ~30 seconds, checking
/// whether any repo is due for a fetch.
pub fn spawn(store: Arc<Store>, app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        sync_loop(store, app_handle).await;
    });
}

// ---------------------------------------------------------------------------
// Core loop
// ---------------------------------------------------------------------------

async fn sync_loop(store: Arc<Store>, app_handle: tauri::AppHandle) {
    let mut repo_states: HashMap<String, RepoSyncState> = HashMap::new();

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(TICK_INTERVAL_SECS)).await;

        // Re-classify repos on every tick so tier transitions are immediate.
        let repos = match store.list_repos_for_sync() {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[background_sync] failed to list repos: {e}");
                continue;
            }
        };

        let now = now_ms();

        for (github_repo, default_branch, is_pinned, has_projects) in &repos {
            let tier = classify(*is_pinned, *has_projects);

            let state = repo_states
                .entry(github_repo.clone())
                .or_insert_with(|| RepoSyncState {
                    // Schedule the first fetch on the next tick so we don't
                    // slam all repos at startup simultaneously.
                    next_fetch_at: now,
                    tier,
                });

            // If the tier changed (e.g. repo was just pinned), adjust the
            // schedule so the new interval takes effect immediately.
            if state.tier != tier {
                let old_interval = state.tier.interval_ms();
                let new_interval = tier.interval_ms();
                // Re-anchor: keep the same elapsed fraction but with the new interval.
                let elapsed_since_last =
                    now.saturating_sub(state.next_fetch_at.saturating_sub(old_interval));
                let remaining = (new_interval - elapsed_since_last).max(0);
                state.next_fetch_at = now + remaining;
                state.tier = tier;
            }

            if now < state.next_fetch_at {
                continue;
            }

            // Check that the clone actually exists on disk.
            let clone_path = match crate::paths::clone_path_for(github_repo) {
                Some(p) if p.join(".git").exists() => p,
                _ => {
                    // No local clone yet — recheck on the tier's normal cadence
                    // so a pinned or project-backed repo picks up a fresh clone
                    // promptly instead of waiting the cold-tier 4 hours.
                    state.next_fetch_at = now + tier.interval_ms();
                    continue;
                }
            };

            let default_branch_value = default_branch.clone();
            let github_repo_clone = github_repo.clone();
            let clone_path_clone = clone_path.clone();
            let store_clone = Arc::clone(&store);
            let app_handle_clone = app_handle.clone();

            // Run the sync on a blocking thread so git commands don't block
            // the async executor.
            let result = tauri::async_runtime::spawn_blocking(move || {
                sync_repo(
                    &clone_path_clone,
                    &github_repo_clone,
                    default_branch_value.as_deref(),
                    &store_clone,
                    &app_handle_clone,
                )
            })
            .await;

            match result {
                Ok(Ok(())) => {
                    log::info!(
                        "[background_sync] synced {} (tier={})",
                        github_repo,
                        tier.label()
                    );
                }
                Ok(Err(e)) => {
                    log::warn!("[background_sync] sync failed for {}: {e}", github_repo);
                }
                Err(e) => {
                    log::warn!(
                        "[background_sync] spawn_blocking panicked for {}: {e}",
                        github_repo
                    );
                }
            }

            // Re-read `now` after the blocking work — schedule based on
            // when we finish, not when we started.
            let after = now_ms();
            if let Some(st) = repo_states.get_mut(github_repo) {
                st.next_fetch_at = after + tier.interval_ms();
            }
        }

        // Prune tracking entries for repos that no longer appear in the DB.
        let known: std::collections::HashSet<&String> =
            repos.iter().map(|(r, _, _, _)| r).collect();
        repo_states.retain(|k, _| known.contains(k));
    }
}

// ---------------------------------------------------------------------------
// Tier classification
// ---------------------------------------------------------------------------

fn classify(is_pinned: bool, has_projects: bool) -> SyncTier {
    if is_pinned {
        SyncTier::Hot
    } else if has_projects {
        SyncTier::Warm
    } else {
        SyncTier::Cold
    }
}

// ---------------------------------------------------------------------------
// Per-repo sync
// ---------------------------------------------------------------------------

fn sync_repo(
    clone_path: &Path,
    github_repo: &str,
    stored_default_branch: Option<&str>,
    store: &Store,
    app_handle: &tauri::AppHandle,
) -> Result<(), String> {
    // 0. Handle bare-clone auto-fix.
    fix_bare_clone_if_needed(clone_path, github_repo)?;

    // 1. Resolve default branch — detect if missing.
    let default_branch = match stored_default_branch {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => {
            let detected = crate::git::detect_default_branch_from_remote(clone_path)
                .map_err(|e| format!("detect default branch: {e}"))?;
            // Best-effort persist. The subpath is "" for the repo-level badge
            // which is what the background sync operates on.
            let _ = store.set_default_branch(github_repo, "", &detected);
            detected
        }
    };

    let origin_ref = format!("origin/{default_branch}");

    // 2. Fetch.
    crate::git::cli_run(clone_path, &["fetch", "origin", &default_branch])
        .map_err(|e| format!("git fetch: {e}"))?;

    // Update the shared fetch cache so FetchMode::Ttl sees this repo as fresh.
    crate::git::update_repo_fetch_cache(clone_path);

    // 3. Compute upstream relation.
    let relation = compute_upstream_relation(clone_path, &origin_ref)?;

    // 4-6. Act based on relation + worktree state.
    match relation {
        UpstreamAction::InSync => {
            // Nothing to do.
        }
        UpstreamAction::OriginAhead => {
            let dirty = is_worktree_dirty(clone_path);
            if dirty {
                log::info!(
                    "[background_sync] {} has dirty worktree, skipping ff",
                    github_repo
                );
                emit_repo_sync_event(app_handle, github_repo, true);
                return Ok(());
            }
            // Fast-forward.
            crate::git::cli_run(clone_path, &["merge", "--ff-only", &origin_ref])
                .map_err(|e| format!("ff-only merge: {e}"))?;
        }
        UpstreamAction::Diverged => {
            log::warn!(
                "[background_sync] {} diverged from origin, resetting to {}",
                github_repo,
                origin_ref
            );
            crate::git::cli_run(clone_path, &["reset", "--hard", &origin_ref])
                .map_err(|e| format!("reset --hard: {e}"))?;
        }
        UpstreamAction::LocalAhead => {
            // Unexpected for a main clone. Log but don't force-reset —
            // the user may have intentionally committed.
            log::warn!(
                "[background_sync] {} is ahead of origin; skipping",
                github_repo
            );
        }
        UpstreamAction::NoUpstream => {
            // origin ref doesn't exist yet — nothing to sync.
        }
    }

    // 7. Emit refresh event.
    let dirty = is_worktree_dirty(clone_path);
    emit_repo_sync_event(app_handle, github_repo, dirty);

    Ok(())
}

// ---------------------------------------------------------------------------
// Bare-clone auto-fix
// ---------------------------------------------------------------------------

fn fix_bare_clone_if_needed(clone_path: &Path, github_repo: &str) -> Result<(), String> {
    // A "bare" clone has no `.git` *directory* — the repo metadata lives
    // directly in the clone_path. But our check in the caller already
    // requires `.git` to exist, so we only get here for non-bare clones
    // *or* clones where `.git` is a file (gitdir link). Check `core.bare`.
    let is_bare = crate::git::cli_run(clone_path, &["config", "--get", "core.bare"])
        .ok()
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !is_bare {
        return Ok(());
    }

    log::warn!(
        "[background_sync] bare clone detected for {}, auto-fixing",
        github_repo
    );

    crate::git::cli_run(clone_path, &["config", "core.bare", "false"])
        .map_err(|e| format!("set core.bare=false: {e}"))?;

    // Detect default branch for checkout target.
    let branch = crate::git::detect_default_branch_from_remote(clone_path)
        .unwrap_or_else(|_| "main".to_string());

    // Restore the working tree.
    let _ = crate::git::cli_run(clone_path, &["checkout", &branch]);

    Ok(())
}

// ---------------------------------------------------------------------------
// Upstream relation helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum UpstreamAction {
    InSync,
    OriginAhead,
    LocalAhead,
    Diverged,
    NoUpstream,
}

fn compute_upstream_relation(
    clone_path: &Path,
    origin_ref: &str,
) -> Result<UpstreamAction, String> {
    // Check if the origin ref exists.
    let origin_sha = crate::git::cli_run(clone_path, &["rev-parse", "--verify", origin_ref])
        .ok()
        .and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        });
    if origin_sha.is_none() {
        return Ok(UpstreamAction::NoUpstream);
    }

    let head_sha = crate::git::cli_run(clone_path, &["rev-parse", "HEAD"])
        .ok()
        .and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        });
    if head_sha.is_none() {
        // Empty repo — no HEAD yet.
        return Ok(UpstreamAction::NoUpstream);
    }

    if head_sha == origin_sha {
        return Ok(UpstreamAction::InSync);
    }

    let counts = crate::git::cli_run(
        clone_path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{origin_ref}"),
        ],
    )
    .map_err(|e| format!("rev-list: {e}"))?;

    let mut parts = counts.split_whitespace();
    let ahead: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    Ok(match (ahead, behind) {
        (0, 0) => UpstreamAction::InSync,
        (_, 0) => UpstreamAction::LocalAhead,
        (0, _) => UpstreamAction::OriginAhead,
        _ => UpstreamAction::Diverged,
    })
}

fn is_worktree_dirty(clone_path: &Path) -> bool {
    crate::git::cli_run(clone_path, &["status", "--porcelain"])
        .ok()
        .map(|output| !output.trim().is_empty())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Event emission
// ---------------------------------------------------------------------------

fn emit_repo_sync_event(app_handle: &tauri::AppHandle, github_repo: &str, is_dirty: bool) {
    #[derive(Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RepoSyncUpdate {
        github_repo: String,
        is_dirty: bool,
    }

    emit_to_all(
        app_handle,
        "repo-sync-update",
        RepoSyncUpdate {
            github_repo: github_repo.to_string(),
            is_dirty,
        },
    );
}

// ---------------------------------------------------------------------------
// Timestamp helper
// ---------------------------------------------------------------------------

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
