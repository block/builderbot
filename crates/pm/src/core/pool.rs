use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::path::Path;

use super::git;
use super::state::{Slot, State};

const DEFAULT_MAX_SLOTS: usize = 2;

/// Prepare a slot's worktree for the given branch. If the worktree directory
/// already exists, checks out the branch; otherwise creates a new worktree.
fn prepare_slot(slot_path: &Path, bare_path: &Path, branch: &str) -> Result<()> {
    if slot_path.exists() {
        git::checkout(slot_path, branch)
    } else {
        git::add_worktree(bare_path, slot_path, branch)
    }
}

/// Ensure a repo has its pool slots initialized in state
pub fn ensure_slots(state: &mut State, repo_name: &str) -> Result<()> {
    let repo = state
        .repos
        .get(repo_name)
        .with_context(|| format!("Repo '{}' not found. Run `pm repo add` first.", repo_name))?
        .clone();

    let max_slots = repo.max_slots;
    let pool_dir = State::pool_dir(&state.root);

    let slots = state.pool.slots.entry(repo_name.to_string()).or_default();

    while slots.len() < max_slots {
        let index = slots.len();
        let slot_path = pool_dir.join(format!("{}--{}", repo_name, index));
        slots.push(Slot {
            index,
            path: slot_path,
            owner: None,
            branch: None,
            last_used: Utc::now(),
        });
    }

    Ok(())
}

/// Info about a slot that could be evicted
#[derive(Debug, Clone)]
pub struct EvictionCandidate {
    pub slot_index: usize,
    pub owner: String,
    pub branch: Option<String>,
    pub last_used: chrono::DateTime<Utc>,
}

/// Returned when all slots are full and eviction is needed
#[derive(Debug)]
pub struct EvictionNeeded {
    pub repo_name: String,
    pub candidates: Vec<EvictionCandidate>,
    pub current_max_slots: usize,
}

/// Result of trying to acquire a slot
pub enum AcquireResult {
    /// Slot acquired successfully
    Acquired(usize),
    /// All slots full — caller must decide what to do
    NeedsEviction(EvictionNeeded),
}

/// Acquire a pool slot for a project+repo+branch.
///
/// Strategy:
/// 1. If this project already owns a slot for this repo, reuse it
/// 2. If there's a free (unowned) slot, take it
/// 3. Return eviction candidates for the caller to decide
///
/// Returns `AcquireResult`.
pub fn acquire_slot(
    state: &mut State,
    repo_name: &str,
    project_name: &str,
    branch: &str,
) -> Result<AcquireResult> {
    ensure_slots(state, repo_name)?;

    let slots = state.pool.slots.get_mut(repo_name).unwrap();

    // 1. Already owned by this project?
    if let Some(slot) = slots
        .iter_mut()
        .find(|s| s.owner.as_deref() == Some(project_name))
    {
        let idx = slot.index;
        let needs_checkout = slot.branch.as_deref() != Some(branch);
        slot.branch = Some(branch.to_string());
        slot.last_used = Utc::now();

        if needs_checkout {
            let repo = state.repos.get(repo_name).unwrap();
            prepare_slot(&slot.path, &repo.bare_path, branch)?;
        }
        return Ok(AcquireResult::Acquired(idx));
    }

    // 2. Free slot?
    if let Some(slot) = slots.iter_mut().find(|s| s.owner.is_none()) {
        let idx = slot.index;
        slot.owner = Some(project_name.to_string());
        slot.branch = Some(branch.to_string());
        slot.last_used = Utc::now();

        let repo = state.repos.get(repo_name).unwrap();
        prepare_slot(&slot.path, &repo.bare_path, branch)?;
        return Ok(AcquireResult::Acquired(idx));
    }

    // 3. All slots full — gather eviction candidates (non-pinned)
    let pinned_projects: Vec<String> = state
        .projects
        .values()
        .filter(|p| p.pinned)
        .map(|p| p.name.clone())
        .collect();

    let slots = state.pool.slots.get(repo_name).unwrap();
    let candidates: Vec<EvictionCandidate> = slots
        .iter()
        .filter(|s| {
            s.owner
                .as_ref()
                .map(|o| !pinned_projects.contains(o))
                .unwrap_or(true)
        })
        .map(|s| EvictionCandidate {
            slot_index: s.index,
            owner: s.owner.clone().unwrap_or_default(),
            branch: s.branch.clone(),
            last_used: s.last_used,
        })
        .collect();

    if candidates.is_empty() {
        bail!(
            "No available pool slots for '{}'. All {} slots are pinned.\n\
             Increase pool size with: pm add {} --grow-pool\n\
             Or unpin a project first.",
            repo_name,
            slots.len(),
            repo_name,
        );
    }

    let current_max_slots = state
        .repos
        .get(repo_name)
        .map(|r| r.max_slots)
        .unwrap_or(DEFAULT_MAX_SLOTS);

    Ok(AcquireResult::NeedsEviction(EvictionNeeded {
        repo_name: repo_name.to_string(),
        candidates,
        current_max_slots,
    }))
}

/// Execute an eviction: take over a specific slot for a new project+branch.
///
/// This performs the git checkout first, then cleans up the evicted project's state.
pub fn execute_eviction(
    state: &mut State,
    repo_name: &str,
    evict_idx: usize,
    project_name: &str,
    branch: &str,
) -> Result<usize> {
    let slots = state.pool.slots.get(repo_name).unwrap();
    let evicted_owner = slots[evict_idx].owner.clone();

    // Attempt git checkout/worktree BEFORE removing anything.
    // If git fails, the evicted project's state is untouched.
    let slot_path = slots[evict_idx].path.clone();
    let repo = state.repos.get(repo_name).unwrap().clone();
    prepare_slot(&slot_path, &repo.bare_path, branch)?;

    // Git succeeded — now safe to update symlinks and state
    let slots = state.pool.slots.get_mut(repo_name).unwrap();
    if let Some(ref owner) = evicted_owner {
        let project_dir = State::projects_dir(&state.root).join(owner);
        let link = project_dir.join(repo_name);
        if link.exists() || link.is_symlink() {
            let _ = std::fs::remove_file(&link);
        }
        // Remove the repo from the evicted project's repo map
        if let Some(evicted_project) = state.projects.get_mut(owner.as_str()) {
            evicted_project.repos.remove(repo_name);
        }
    }

    let slot = &mut slots[evict_idx];
    slot.owner = Some(project_name.to_string());
    slot.branch = Some(branch.to_string());
    slot.last_used = Utc::now();

    Ok(evict_idx)
}

/// Grow the pool for a repo by 1 slot, then acquire it for the given project+branch.
pub fn grow_and_acquire(
    state: &mut State,
    repo_name: &str,
    project_name: &str,
    branch: &str,
) -> Result<usize> {
    // Bump max_slots
    let repo = state
        .repos
        .get_mut(repo_name)
        .with_context(|| format!("Repo '{}' not found", repo_name))?;
    repo.max_slots += 1;
    let new_max = repo.max_slots;

    // Re-ensure slots so the new one is created
    ensure_slots(state, repo_name)?;

    let slots = state.pool.slots.get_mut(repo_name).unwrap();
    let slot = slots
        .iter_mut()
        .find(|s| s.owner.is_none())
        .expect("Just grew pool, should have a free slot");

    let idx = slot.index;
    slot.owner = Some(project_name.to_string());
    slot.branch = Some(branch.to_string());
    slot.last_used = Utc::now();

    let repo = state.repos.get(repo_name).unwrap();
    prepare_slot(&slot.path, &repo.bare_path, branch)?;

    use colored::Colorize;
    eprintln!(
        "  {} Pool for {} grown to {} slots.",
        "↑".green().bold(),
        repo_name.bold(),
        new_max,
    );

    Ok(idx)
}

/// Release all slots owned by a project.
///
/// Removes each owned slot's worktree from disk and from git's worktree list,
/// then clears the slot's owner/branch in state. Worktree removal is best-effort:
/// a slot whose directory was already deleted manually must not cause an error.
pub fn release_project(state: &mut State, project_name: &str) {
    // Collect (repo_name, slot_index) pairs owned by this project first, so we can
    // do the git/disk work without holding a mutable borrow of state.pool.slots.
    let owned: Vec<(String, usize)> = state
        .pool
        .slots
        .iter()
        .flat_map(|(repo_name, slots)| {
            slots
                .iter()
                .filter(|s| s.owner.as_deref() == Some(project_name))
                .map(move |s| (repo_name.clone(), s.index))
        })
        .collect();

    for (repo_name, slot_index) in &owned {
        if let Some(slot) = state
            .pool
            .slots
            .get(repo_name)
            .and_then(|slots| slots.iter().find(|s| s.index == *slot_index))
            .cloned()
        {
            // Best-effort: don't let an already-removed worktree break removal.
            let _ = remove_slot_worktree(state, repo_name, &slot);
        }
    }

    for slots in state.pool.slots.values_mut() {
        for slot in slots.iter_mut() {
            if slot.owner.as_deref() == Some(project_name) {
                slot.owner = None;
                slot.branch = None;
            }
        }
    }
}

/// Get the default max slots for a new repo
pub fn default_max_slots() -> usize {
    DEFAULT_MAX_SLOTS
}

/// Remove a worktree slot from disk and prune
pub fn remove_slot_worktree(state: &State, repo_name: &str, slot: &Slot) -> Result<()> {
    if slot.path.exists() {
        if let Some(repo) = state.repos.get(repo_name) {
            git::remove_worktree(&repo.bare_path, &slot.path)?;
        }
        // If git worktree remove didn't clean it up, force remove
        if slot.path.exists() {
            std::fs::remove_dir_all(&slot.path)
                .with_context(|| format!("Failed to remove {}", slot.path.display()))?;
        }
    }
    Ok(())
}

/// Resolve the worktree path for a given project's repo
#[allow(dead_code)]
pub fn resolve_slot_path(
    state: &State,
    repo_name: &str,
    project_name: &str,
) -> Option<std::path::PathBuf> {
    state.pool.slots.get(repo_name).and_then(|slots| {
        slots
            .iter()
            .find(|s| s.owner.as_deref() == Some(project_name))
            .map(|s| s.path.clone())
    })
}
