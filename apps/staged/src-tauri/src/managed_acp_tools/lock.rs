//! The combined lock for npm writers outside the managed installer.

use std::path::Path;

use crate::managed_node::{self, ManagedNodeError, PackagesDirLock};

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
pub(crate) struct NpmWriteLock {
    // Field order is drop order: the flock is released before the mutex, the
    // reverse of acquisition.
    _packages_lock: PackagesDirLock,
    _guard: tokio::sync::MutexGuard<'static, ()>,
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

pub(super) async fn lock_npm_writes_at(
    packages_root: &Path,
) -> Result<NpmWriteLock, ManagedNodeError> {
    let guard = tool_install_lock().lock().await;
    let packages_lock = managed_node::lock_packages_dir(packages_root).await?;
    Ok(NpmWriteLock {
        _packages_lock: packages_lock,
        _guard: guard,
    })
}
