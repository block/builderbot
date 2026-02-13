//! Centralized storage paths following XDG Base Directory conventions.
//!
//! All app-managed data lives under `$XDG_DATA_HOME/staged/`
//! (defaulting to `~/.local/share/staged/`).

use std::path::PathBuf;

/// Return the root data directory for Staged.
///
/// Resolves to `$XDG_DATA_HOME/staged` if the env var is set,
/// otherwise `~/.local/share/staged`.
pub fn data_dir() -> Result<PathBuf, String> {
    let base = match std::env::var("XDG_DATA_HOME") {
        Ok(val) if !val.is_empty() => PathBuf::from(val),
        _ => dirs::home_dir()
            .ok_or_else(|| "Cannot determine home directory".to_string())?
            .join(".local")
            .join("share"),
    };
    Ok(base.join("staged"))
}
