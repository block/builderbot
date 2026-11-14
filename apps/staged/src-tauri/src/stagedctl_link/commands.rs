//! The Tauri command behind the Settings action that puts `stagedctl` on
//! PATH. Desktop only: `web_server::dispatch` refuses it, and it stays out
//! of `control_server::ALLOWED_COMMANDS`, so no browser client and no
//! stagedctl caller, an agent session included, can change what the user's
//! shell runs.
//!
//! This is the one place the real inputs are read: the running executable,
//! the home directory and the user's PATH. Everything it decides with is in
//! [`super`], and everything it writes goes through [`super::fs`], which are
//! tested there with paths of their own.

use std::ffi::OsString;
use std::time::Duration;

use super::{fs, link_candidates, link_source, Installed, LinkOutcome, ShownLink};

/// How long the answer waits for the user's shell environment. It is
/// captured at startup and cached, so this only bites when the cache has
/// lapsed and the login shell is slow. The link is already made by then;
/// the answer just goes without the PATH check.
const PATH_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Link this app's bundled `stagedctl` into `/usr/local/bin`, or into
/// `~/.local/bin` when that cannot be written.
///
/// Takes no path. `replace` is only a confirmation: the `link` and `target`
/// of a conflict the user was shown, repeated back. A link is replaced only
/// when it is still that one, and a file never is.
///
/// An `Err` is a sentence for the user. Nothing was linked.
#[tauri::command]
pub async fn link_stagedctl_on_path(replace: Option<ShownLink>) -> Result<LinkOutcome, String> {
    let executable = std::env::current_exe()
        .map_err(|e| format!("Staged could not find its own executable: {e}."))?;
    let source = link_source(&executable).map_err(|refusal| refusal.to_string())?;
    let links = link_candidates(dirs::home_dir().as_deref());

    let installed = fs::install(&source, &links, replace.as_ref())?;

    let path_var = match installed {
        Installed::Conflict { .. } => None,
        Installed::Linked(_) | Installed::AlreadyLinked(_) => user_path().await,
    };
    Ok(LinkOutcome::new(installed, path_var.as_deref()))
}

/// The PATH of the user's login shell, which is what a new terminal gets.
/// The app's own PATH is launchd's and says nothing about it.
async fn user_path() -> Option<OsString> {
    let cache = crate::session_runner::shell_env_cache();
    let capture = crate::shell_env::capture_home_interactive_env(cache.as_ref());
    let mut env = tokio::time::timeout(PATH_LOOKUP_TIMEOUT, capture)
        .await
        .ok()?;
    env.remove("PATH").map(OsString::from)
}
