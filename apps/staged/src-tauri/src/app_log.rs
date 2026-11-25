//! The app's logger: `tauri-plugin-log`, in every build.
//!
//! The plugin's defaults (a 40 KB file, deleted when full) held about two
//! minutes of a busy session, with every agent's stderr going in at `warn`,
//! so the log of a failure was gone before anyone could read it. Release
//! builds registered no logger at all, making every `log::` call a no-op.

use tauri::plugin::TauriPlugin;
use tauri::Runtime;
use tauri_plugin_log::RotationStrategy;

/// Size at which the active log file is rotated.
const MAX_LOG_FILE_BYTES: u128 = 5_000_000;

/// Rotated files kept beside the active one. With [`MAX_LOG_FILE_BYTES`]
/// that bounds the log at about 25 MB, many hours of a busy session.
const KEPT_LOG_FILES: usize = 4;

/// The log plugin, registered in debug and release builds alike.
pub(crate) fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_log::Builder::default()
        .level(log::LevelFilter::Info)
        .max_file_size(MAX_LOG_FILE_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(KEPT_LOG_FILES))
        .build()
}
