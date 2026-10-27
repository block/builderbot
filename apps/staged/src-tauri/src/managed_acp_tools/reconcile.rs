//! Reconcile epilogue — prune stale ids, record the outcome, gate the Node
//! prune.

use std::path::Path;

use crate::managed_node;

use super::install::tool_install_lock;
use super::{read_state, shim_bin_dir, tools_root, write_state, ManagedTool, ReconcileRecord};

#[cfg(test)]
#[path = "tests/reconcile.rs"]
mod tests;

// =============================================================================
// Reconcile epilogue — prune stale ids, record the outcome, gate the Node prune
// =============================================================================

/// Reconcile epilogue: drop installs for ids no longer in the managed set
/// (their shims, tool dirs, and state entries), record the run's outcome in
/// `state.json`, and — only when every managed bridge installed cleanly —
/// prune superseded managed Node runtimes. Takes the install mutex and the
/// cross-process flock so it cannot race an in-flight install in this or any
/// other Staged process.
pub(crate) async fn finish_reconcile(managed: &[ManagedTool], errors: Vec<String>) {
    let Some(packages_root) = crate::paths::packages_dir() else {
        return;
    };
    finish_reconcile_at(&packages_root, managed, errors).await;
}

pub(super) async fn finish_reconcile_at(
    packages_root: &Path,
    managed: &[ManagedTool],
    errors: Vec<String>,
) {
    let all_installed = errors.is_empty();
    {
        let _guard = tool_install_lock().lock().await;
        let _packages_lock = match managed_node::lock_packages_dir(packages_root).await {
            Ok(lock) => lock,
            Err(error) => {
                log::warn!("skipping ACP tools reconcile epilogue: {error}");
                return;
            }
        };
        prune_stale_managed_tools(packages_root, managed);
        record_reconcile(packages_root, errors);
    }
    // Success-gated Node prune: `errors` empty means every managed bridge
    // reinstalled this run, so every shim now embeds the pinned runtime's
    // path and superseded runtimes are unreferenced. On partial failure the
    // old runtime is kept — the failed bridge's un-rewritten shim still
    // resolves a real Node, so an offline launch never breaks a working
    // bridge. Runs outside the scope above: the prune takes the same flock
    // itself, and nesting would deadlock (see `lock_packages_dir`).
    if all_installed {
        managed_node::prune_superseded_node_runtimes(packages_root).await;
    }
}

pub(crate) fn prune_stale_managed_tools(packages_root: &Path, managed: &[ManagedTool]) {
    let managed_ids: Vec<&str> = managed.iter().map(|tool| tool.id).collect();
    let managed_binaries: Vec<&str> = managed.iter().map(|tool| tool.binary).collect();

    let mut state = read_state(packages_root);
    let stale: Vec<String> = state
        .tools
        .keys()
        .filter(|id| !managed_ids.contains(&id.as_str()))
        .cloned()
        .collect();
    for id in &stale {
        if let Some(pin) = state.tools.remove(id) {
            let _ = std::fs::remove_file(shim_bin_dir(packages_root).join(&pin.binary));
        }
    }
    if !stale.is_empty() {
        if let Err(error) = write_state(packages_root, &state) {
            log::warn!("failed to write ACP tools state after prune: {error}");
        }
    }

    // Tool dirs with no state entry (crashed installs) and shims for binaries
    // no longer managed. `<packages>/bin` holds only Staged-written shims, so
    // pruning by name is safe.
    if let Ok(entries) = std::fs::read_dir(tools_root(packages_root)) {
        for entry in entries.flatten() {
            if !managed_ids.contains(&entry.file_name().to_string_lossy().as_ref()) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(shim_bin_dir(packages_root)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && !managed_binaries.contains(&name.as_str()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

pub(crate) fn record_reconcile(packages_root: &Path, errors: Vec<String>) {
    let mut state = read_state(packages_root);
    state.last_reconcile = Some(ReconcileRecord {
        at_ms: now_ms(),
        ok: errors.is_empty(),
        errors,
    });
    if let Err(error) = write_state(packages_root, &state) {
        log::warn!("failed to record ACP tools reconcile result: {error}");
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
