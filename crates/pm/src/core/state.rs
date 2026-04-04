use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Root directory name for pm state
const PM_DIR: &str = ".pm";
const STATE_FILE: &str = "state.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub root: PathBuf,
    pub repos: BTreeMap<String, RepoEntry>,
    pub projects: BTreeMap<String, Project>,
    pub pool: PoolState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    /// Remote URL
    pub url: String,
    /// Short name derived from URL (e.g. "repo1")
    pub name: String,
    /// Path to the bare clone
    pub bare_path: PathBuf,
    /// Max pool slots for this repo
    pub max_slots: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// repo_name -> branch
    pub repos: BTreeMap<String, String>,
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
    pub last_activated: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolState {
    /// repo_name -> vec of slots
    pub slots: BTreeMap<String, Vec<Slot>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slot {
    pub index: usize,
    /// Worktree directory path
    pub path: PathBuf,
    /// Which project currently owns this slot (if any)
    pub owner: Option<String>,
    /// What branch is checked out
    pub branch: Option<String>,
    /// Last time this slot was activated
    pub last_used: DateTime<Utc>,
}

impl State {
    pub fn repos_dir(base: &Path) -> PathBuf {
        base.join(PM_DIR).join("repos")
    }

    pub fn pool_dir(base: &Path) -> PathBuf {
        base.join(PM_DIR).join("pool")
    }

    pub fn projects_dir(base: &Path) -> PathBuf {
        base.to_path_buf()
    }

    pub fn state_file(base: &Path) -> PathBuf {
        base.join(PM_DIR).join(STATE_FILE)
    }

    /// Load state from disk, or return None if not initialized
    pub fn load(base: &Path) -> Result<Option<Self>> {
        let path = Self::state_file(base);
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read state from {}", path.display()))?;
        let state: State = serde_json::from_str(&data)
            .with_context(|| "Failed to parse state file")?;
        Ok(Some(state))
    }

    /// Load state, error if not initialized
    pub fn load_or_err(base: &Path) -> Result<Self> {
        Self::load(base)?
            .with_context(|| format!("pm is not initialized in {}. Run `pm init` first.", base.display()))
    }

    /// Save state to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::state_file(&self.root);
        let data = serde_json::to_string_pretty(self)
            .context("Failed to serialize state")?;
        std::fs::write(&path, data)
            .with_context(|| format!("Failed to write state to {}", path.display()))?;
        Ok(())
    }

    /// Create a fresh state for a new init
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            repos: BTreeMap::new(),
            projects: BTreeMap::new(),
            pool: PoolState {
                slots: BTreeMap::new(),
            },
        }
    }
}

/// Derive a short repo name from a URL.
/// e.g. "git@github.com:org/repo.git" -> "repo"
/// e.g. "https://github.com/org/repo" -> "repo"
pub fn repo_name_from_url(url: &str) -> String {
    let s = url.trim_end_matches('/').trim_end_matches(".git");
    s.rsplit('/').next()
        .or_else(|| s.rsplit(':').next())
        .unwrap_or(s)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_name_from_url() {
        assert_eq!(repo_name_from_url("git@github.com:org/myrepo.git"), "myrepo");
        assert_eq!(repo_name_from_url("https://github.com/org/myrepo"), "myrepo");
        assert_eq!(repo_name_from_url("https://github.com/org/myrepo.git"), "myrepo");
        assert_eq!(repo_name_from_url("git@github.com:org/myrepo"), "myrepo");
    }
}
