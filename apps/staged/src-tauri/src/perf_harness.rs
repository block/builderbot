//! Hooks for the load-test harness in `apps/staged/scripts/perf-harness`.
//!
//! Compiled only with the `perf-harness` cargo feature, which no release or
//! dev recipe enables, and inert in a binary that has it until
//! [`PORT_ENV`] is set. With it set the app:
//!
//! - refuses to run unless its data directory is inside [`ROOT_ENV`], so the
//!   hooks can never be pointed at a real `~/.staged`;
//! - never activates (no Dock icon, no window on screen, no focus taken);
//! - serves, on `127.0.0.1` only, the web API and the built frontend (the
//!   web server proper is stubbed, see `web_server::start`) plus the
//!   `/perf/*` routes in [`server`]: evaluate a script in the desktop
//!   webviews, hand a result back, read and reset the counters;
//! - records the counters in [`metrics`]: commands served over that port,
//!   session stream frames, how long the store's connection lock was waited
//!   for and held, and how long after a fake agent stamped a piece of output
//!   its row was written.
//!
//! The touch points outside this module are one line each: the call in
//! `lib.rs`'s setup, the lock's type in `store/mod.rs`, the two message
//! writers in `store/messages.rs` and the send in `session_stream/hub.rs`.
//! Only the last, with [`stream_frames`], needs the session stream; a tree
//! without the stream takes the rest unchanged.

mod guard;
mod histogram;
pub(crate) mod metrics;
mod server;
pub(crate) mod store_lock;
pub(crate) mod stream_frames;

/// The loopback port to serve on. Unset, nothing here runs.
const PORT_ENV: &str = "STAGED_PERF_HARNESS_PORT";
/// The harness's throwaway directory, which must contain the data directory.
const ROOT_ENV: &str = "STAGED_PERF_HARNESS_ROOT";
/// The built frontend to serve.
const DIST_ENV: &str = "STAGED_PERF_HARNESS_DIST";
/// What a caller of `/perf/*` must send as `x-perf-token`.
const TOKEN_ENV: &str = "STAGED_PERF_HARNESS_TOKEN";

/// Called first thing in the app's setup, before the data directory is
/// created or the database opened.
pub(crate) fn start(app: &mut tauri::App) {
    let Ok(port) = std::env::var(PORT_ENV) else {
        return;
    };
    let config = match guard::check(&port) {
        Ok(config) => config,
        Err(reason) => {
            // Not a panic: nothing may run against a data directory the
            // harness does not own, and the reason has to reach its log.
            eprintln!("[perf-harness] refusing to start: {reason}");
            std::process::exit(78);
        }
    };
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Prohibited);
    metrics::activate();
    tauri::async_runtime::spawn(server::serve(app.handle().clone(), config));
}
