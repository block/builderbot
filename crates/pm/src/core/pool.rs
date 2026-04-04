use anyhow::{Context, Result, bail};
use chrono::Utc;

use super::git;
use super::state::{Slot, State};

const DEFAULT_MAX_SLOTS: usize = 2;

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

/// Acquire a pool slot for a project+repo+branch.
///
/// Strategy:
/// 1. If this project already owns a slot for this repo, reuse it
/// 2. If there's a free (unowned) slot, take it
/// 3. Evict the least-recently-used non-pinned slot
///
/// Returns the slot index.
pub fn acquire_slot(
    state: &mut State,
    repo_name: &str,
    project_name: &str,
    branch: &str,
) -> Result<usize> {
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
            if slot.path.exists() {
                git::checkout(&slot.path, branch)?;
            } else {
                git::add_worktree(&repo.bare_path, &slot.path, branch)?;
            }
        }
        return Ok(idx);
    }

    // 2. Free slot?
    if let Some(slot) = slots.iter_mut().find(|s| s.owner.is_none()) {
        let idx = slot.index;
        slot.owner = Some(project_name.to_string());
        slot.branch = Some(branch.to_string());
        slot.last_used = Utc::now();

        let repo = state.repos.get(repo_name).unwrap();
        if slot.path.exists() {
            git::checkout(&slot.path, branch)?;
        } else {
            git::add_worktree(&repo.bare_path, &slot.path, branch)?;
        }
        return Ok(idx);
    }

    // 3. Evict LRU non-pinned slot
    let pinned_projects: Vec<String> = state
        .projects
        .values()
        .filter(|p| p.pinned)
        .map(|p| p.name.clone())
        .collect();

    let slots = state.pool.slots.get_mut(repo_name).unwrap();
    let evict_idx = slots
        .iter()
        .filter(|s| {
            s.owner
                .as_ref()
                .map(|o| !pinned_projects.contains(o))
                .unwrap_or(true)
        })
        .min_by_key(|s| s.last_used)
        .map(|s| s.index);

    let evict_idx = match evict_idx {
        Some(idx) => idx,
        None => bail!(
            "No available pool slots for '{}'. All slots are pinned. \
             Increase max_slots or unpin a project.",
            repo_name
        ),
    };

    // Remove symlink from evicted project
    let evicted_owner = slots[evict_idx].owner.clone();
    if let Some(ref owner) = evicted_owner {
        let project_dir = State::projects_dir(&state.root).join(owner);
        let link = project_dir.join(repo_name);
        if link.exists() || link.is_symlink() {
            let _ = std::fs::remove_file(&link);
        }
    }

    let slot = &mut slots[evict_idx];
    slot.owner = Some(project_name.to_string());
    slot.branch = Some(branch.to_string());
    slot.last_used = Utc::now();

    let repo = state.repos.get(repo_name).unwrap();
    if slot.path.exists() {
        git::checkout(&slot.path, branch)?;
    } else {
        git::add_worktree(&repo.bare_path, &slot.path, branch)?;
    }

    Ok(evict_idx)
}

/// Release all slots owned by a project
pub fn release_project(state: &mut State, project_name: &str) {
    for slots in state.pool.slots.values_mut() {
        for slot in slots.iter_mut() {
            if slot.owner.as_deref() == Some(project_name) {
                slot.owner = None;
            }
        }
    }
}

/// Get the default max slots for a new repo
pub fn default_max_slots() -> usize {
    DEFAULT_MAX_SLOTS
}

/// Remove a worktree slot from disk and prune
#[allow(dead_code)]
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
