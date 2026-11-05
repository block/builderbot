//! Transfers Staged never finished — it quit, crashed or was killed part way
//! — dealt with at the next start.
//!
//! A transfer's manifest records what it makes before making it (see
//! [`super::undo`]), so one still `carrying` or `committed` when no running
//! Staged owns it says what is left. One still `carrying` whose project is
//! at its source never switched the database, and is rolled back: the
//! orphan workspace, the worktrees, branches and remotes it created, its
//! temporary refs. Anything else is only marked, with what may be left, for
//! the user. A `carrying` one whose project is elsewhere (or gone) switched
//! before it could say so. A `committed` one switched, wherever the project
//! is now: back at its source means a later transfer moved it back, and the
//! undo list names what the project then used, perhaps the workspace or
//! worktree that transfer kept because it changed.

use std::path::Path;

use crate::store::Store;

use super::backups::{self, TransferManifest};
use super::host::TransferHost;
use super::is_valid_transfer_id;
use super::preflight::TransferDirection;
use super::registry;
use super::undo;

/// What [`recover`] did with an unfinished transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovered {
    /// The project was still at its source: rolled back, leaving these.
    RolledBack { left: Vec<String> },
    /// It had switched (wherever it is now), or is gone: left for the user,
    /// as this says.
    Marked(String),
}

/// Find every unfinished transfer under `host`'s transfers directory that
/// `is_running` says no one is still running, and roll it back or mark it.
/// Returns each, by transfer id.
pub fn recover(
    store: &Store,
    host: &dyn TransferHost,
    is_running: &dyn Fn(&TransferManifest) -> bool,
) -> Vec<(String, Recovered)> {
    let Ok(transfers_dir) = host.transfers_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&transfers_dir) else {
        return Vec::new();
    };
    let mut recovered = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !entry.file_name().to_str().is_some_and(is_valid_transfer_id) {
            continue;
        }
        let Some(manifest) = backups::read_manifest(&dir) else {
            continue;
        };
        if manifest.is_finished() || is_running(&manifest) {
            continue;
        }
        let outcome = recover_one(store, host, &dir, manifest.clone());
        recovered.push((manifest.transfer_id, outcome));
    }
    recovered
}

fn recover_one(
    store: &Store,
    host: &dyn TransferHost,
    dir: &Path,
    mut manifest: TransferManifest,
) -> Recovered {
    let tid = manifest.transfer_id.clone();
    let source = match manifest.direction.as_str() {
        "toRemote" => Some(TransferDirection::ToRemote.source()),
        "toLocal" => Some(TransferDirection::ToLocal.source()),
        _ => None,
    };
    let location = store
        .get_project(&manifest.project_id)
        .ok()
        .flatten()
        .map(|project| project.location);
    let outcome = if manifest.status == backups::CARRYING && source.is_some() && location == source
    {
        let left = undo::roll_back(&manifest.undo, host, &tid);
        log::warn!(
            "[project_transfer] transfer {tid} of {} was cut off before the project switched; \
             rolled it back{}",
            manifest.project_id,
            if left.is_empty() {
                String::new()
            } else {
                format!(", leaving: {}", left.join("; "))
            }
        );
        manifest.status = backups::ROLLED_BACK.to_string();
        manifest
            .warnings
            .push("Staged stopped during this transfer; it was undone at the next start.".into());
        manifest.warnings.extend(left.iter().cloned());
        Recovered::RolledBack { left }
    } else {
        let warning = format!(
            "Staged stopped during this transfer, {}; nothing was undone. The source may \
             still be there: {}.",
            if location.is_some() {
                "after the project had switched over"
            } else {
                "and the project is gone"
            },
            source_left(&manifest)
        );
        log::warn!("[project_transfer] transfer {tid}: {warning}");
        manifest.status = backups::COMPLETED.to_string();
        manifest.warnings.push(warning.clone());
        Recovered::Marked(warning)
    };
    manifest.keep_until_removed = manifest.holds_left_behind_files();
    if let Err(e) = backups::write_manifest(dir, &manifest) {
        log::warn!("[project_transfer] {e}");
    }
    outcome
}

/// What a transfer's source was, for a person: the workspace it came from,
/// or the worktrees.
fn source_left(manifest: &TransferManifest) -> String {
    let parts: Vec<String> = match manifest.direction.as_str() {
        "toLocal" => manifest
            .workspace_name
            .iter()
            .map(|w| format!("workspace {w}"))
            .collect(),
        _ => manifest
            .undo
            .sources
            .iter()
            .map(|s| s.dir.clone())
            .collect(),
    };
    if parts.is_empty() {
        "nothing was recorded".to_string()
    } else {
        parts.join(", ")
    }
}

/// Whether a running Staged owns `manifest`'s transfer: this one, if it has
/// the project registered, or another whose process is still alive.
pub fn is_running(manifest: &TransferManifest) -> bool {
    if registry::is_project_transferring(&manifest.project_id) {
        return true;
    }
    match manifest.owner_pid {
        Some(pid) if pid != std::process::id() => crate::session_runner::is_process_alive(pid),
        _ => false,
    }
}
