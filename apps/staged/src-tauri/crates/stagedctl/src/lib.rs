//! `stagedctl` — command-line control for a running Staged app.
//!
//! Library layout (the `stagedctl` binary is a thin wrapper over this):
//! - [`cli`]: the clap tree and all help prose (byte-pinned by tests)
//! - [`client`]: discovery file, ping handshake, HTTP transport, exit codes
//! - [`commands`]: verb implementations and output rendering
//!
//! The CLI is a dumb flag→JSON mapper. The app's control server
//! (`src-tauri/src/control_server.rs`) is the trust boundary: it allowlists
//! exactly the v1 command table, so nothing this crate could send reaches a
//! destructive operation.

pub mod cli;
pub mod client;
pub mod commands;
