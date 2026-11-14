//! The combined lock for npm writers outside the managed installer.

use std::path::{Path, PathBuf};

use crate::managed_node::{self, ManagedNodeError, PackagesDirLock};

use super::cache::{prune_npm_cache_on_blocking_pool, NPM_CACHE_MAX_BYTES};
use super::install::tool_install_lock;

#[cfg(test)]
#[path = "tests/lock.rs"]
mod tests;

/// Held while something other than the managed installer writes the shared
/// packages tree through npm — Doctor's `npm install -g` fixes and updates.
/// It is the same pair the installer and the reconcile epilogue hold (the
/// in-process tool-install mutex, then the cross-process packages flock), so
/// such a fix can neither overlap a bridge install nor have the npm cache
/// pruned out from under it (see `cache::prune_npm_cache_at`), in this or any
/// other Staged process sharing the tree.
///
/// Because the holder is by construction the only Staged npm writer, it is
/// also in a position to cap the cache it just wrote: see
/// [`NpmWriteLock::prune_npm_cache`].
pub(crate) struct NpmWriteLock {
    /// The packages tree the two locks below cover — the one whose npm cache
    /// [`NpmWriteLock::prune_npm_cache`] caps.
    packages_root: PathBuf,
    // Field order is drop order: the flock is released before the mutex, the
    // reverse of acquisition.
    _packages_lock: PackagesDirLock,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl NpmWriteLock {
    /// Wipe the private npm cache if it has outgrown its size cap, while this
    /// lock still excludes every other Staged npm writer. Call it after the
    /// fix's command has finished, whatever its outcome, and before dropping
    /// the lock.
    ///
    /// This is what caps the cache on a build that manages no bridges
    /// (`no-managed-acp-tools`, the `STAGED_ACP_TOOLS_DIR` override, an
    /// unsupported target): the reconciler there never runs a pass, so its
    /// epilogue never prunes, yet Doctor's `npm install -g` fixes and updates
    /// still fill the same cache. On a managed build it is a second, cheap
    /// chance at the same cap.
    ///
    /// Best-effort, like the epilogue's prune: it returns nothing and logs
    /// its failures, so it can never fail the fix it follows.
    pub(crate) async fn prune_npm_cache(&self) {
        self.prune_npm_cache_over(NPM_CACHE_MAX_BYTES).await;
    }

    /// [`NpmWriteLock::prune_npm_cache`] with the cap as a parameter, so
    /// tests can exceed it with a few bytes.
    pub(super) async fn prune_npm_cache_over(&self, max_bytes: u64) {
        prune_npm_cache_on_blocking_pool(&self.packages_root, max_bytes).await;
    }
}

/// Take the [`NpmWriteLock`] for the real `~/.staged/packages`. Waits for
/// whatever holds either lock — worst case a whole bridge install in another
/// Staged instance. Never call while a [`PackagesDirLock`] is held, and only
/// after `managed_node::ensure_managed_node_runtime` has returned: that call
/// takes the same flock, and nesting it deadlocks (see
/// `managed_node::lock_packages_dir`).
pub(crate) async fn lock_npm_writes() -> Result<NpmWriteLock, ManagedNodeError> {
    let packages_root = crate::paths::packages_dir()
        .ok_or_else(|| ManagedNodeError::DataDir("home directory is unavailable".to_string()))?;
    lock_npm_writes_at(&packages_root).await
}

/// [`lock_npm_writes`] for a given packages root.
pub(crate) async fn lock_npm_writes_at(
    packages_root: &Path,
) -> Result<NpmWriteLock, ManagedNodeError> {
    let guard = tool_install_lock().lock().await;
    let packages_lock = managed_node::lock_packages_dir(packages_root).await?;
    Ok(NpmWriteLock {
        packages_root: packages_root.to_path_buf(),
        _packages_lock: packages_lock,
        _guard: guard,
    })
}
