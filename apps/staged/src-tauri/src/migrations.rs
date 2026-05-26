//! One-shot migration registry.
//!
//! Records which migrations have completed in `~/.staged/migrations.json` so
//! each runs at most once per machine. A timestamp is recorded alongside each
//! entry for support/debugging.
//!
//! Migrations are expected to be safe to re-run (idempotent): if the registry
//! file is missing or corrupt, it is treated as empty and migrations run again.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// A one-shot migration. `id` is the stable key recorded in `migrations.json`;
/// `run` performs the migration and returns an error message on failure.
pub struct Migration {
    pub id: &'static str,
    pub run: fn() -> Result<(), String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    #[serde(rename = "completedAt")]
    completed_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    completed: HashMap<String, Record>,
}

fn registry_path() -> Option<PathBuf> {
    crate::paths::data_dir().map(|d| d.join("migrations.json"))
}

fn load() -> Registry {
    let Some(path) = registry_path() else {
        return Registry::default();
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Registry::default(),
        Err(e) => {
            log::warn!(
                "Failed to read migrations registry {}: {e}; treating as empty",
                path.display()
            );
            return Registry::default();
        }
    };
    match serde_json::from_slice::<Registry>(&bytes) {
        Ok(r) => r,
        Err(e) => {
            log::warn!(
                "Corrupt migrations registry {}: {e}; treating as empty",
                path.display()
            );
            Registry::default()
        }
    }
}

fn save(registry: &Registry) {
    let Some(path) = registry_path() else {
        return;
    };
    let bytes = match serde_json::to_vec_pretty(registry) {
        Ok(b) => b,
        Err(e) => {
            log::error!("Failed to serialize migrations registry: {e}");
            return;
        }
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::error!(
                "Failed to create migrations registry dir {}: {e}",
                parent.display()
            );
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, &bytes) {
        log::error!(
            "Failed to write migrations registry {}: {e}",
            path.display()
        );
    }
}

/// Whether migration `id` has already been recorded as completed.
pub fn has_completed(id: &str) -> bool {
    load().completed.contains_key(id)
}

/// Mark migration `id` as completed at the current time. Best-effort: write
/// failures are logged but not surfaced.
pub fn mark_completed(id: &str) {
    let mut registry = load();
    registry.completed.insert(
        id.to_string(),
        Record {
            completed_at: crate::store::now_timestamp(),
        },
    );
    save(&registry);
}

/// Run each pending migration once. Failures are logged but do not halt the
/// rest — a single bad migration should not block startup.
pub fn run_pending(migrations: &[Migration]) {
    let registry = load();
    for m in migrations {
        if registry.completed.contains_key(m.id) {
            continue;
        }
        log::info!("[migrations] running '{}'", m.id);
        match (m.run)() {
            Ok(()) => {
                mark_completed(m.id);
                log::info!("[migrations] '{}' completed", m.id);
            }
            Err(e) => {
                log::error!("[migrations] '{}' failed: {e}", m.id);
            }
        }
    }
}
