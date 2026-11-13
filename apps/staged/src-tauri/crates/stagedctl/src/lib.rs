//! `stagedctl` — command-line control for a running Staged app.
//!
//! Library layout (the `stagedctl` binary is a thin wrapper over this):
//! - [`cli`]: the clap tree and all help prose (byte-pinned by tests)
//! - [`client`]: discovery file, ping handshake, HTTP transport, exit codes
//! - [`commands`]: verb implementations and output rendering
//! - [`open`]: `project open`: asks a window to show a project and words
//!   what the window did
//! - [`tags`]: the `tag` noun and `project tags`: tag wire shapes, `<tag>`
//!   and `<group>` resolution, and their renderers
//!
//! The CLI is a dumb flag→JSON mapper. The app's control server
//! (`src-tauri/src/control_server.rs`) is the trust boundary: it allowlists
//! exactly its command table, so nothing this crate could send reaches a
//! destructive operation.

pub mod cli;
pub mod client;
pub mod commands;
pub mod grants;
pub mod open;
pub mod tags;
