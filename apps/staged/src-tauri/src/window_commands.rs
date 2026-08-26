//! Multi-window support: opening peer app windows.
//!
//! Every window is a full copy of the app — its own sidebar, navigation stack,
//! and selected project; there is no privileged "main" window beyond being the
//! one restored on cold start. New windows are built from the `main` window's
//! own `tauri.conf.json` entry, so the conf stays the single source of truth for
//! chrome — including `visible: false`, which lets the frontend show each window
//! once the theme is applied, exactly like the main window.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use tauri::Manager;

/// Offset of each new window from the opener (focused) window, in logical px.
const CASCADE_OFFSET: f64 = 24.0;

/// The currently focused window, if any. (`Manager::get_focused_window` is
/// behind tauri's `unstable` feature; this is its stable equivalent.)
///
/// Generic over the runtime — the standard tauri-plugin idiom — so the tests
/// below can drive this module with tauri's `MockRuntime`. `generate_handler!`
/// and the menu handler both instantiate it with `Wry`, as before.
pub fn focused_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Option<tauri::WebviewWindow<R>> {
    app.webview_windows()
        .into_values()
        .find(|w| w.is_focused().unwrap_or(false))
}

/// Managed state for window creation.
pub struct NewWindowState {
    /// Suffix for the next `win-N` label. Starts at 2 (the first window is
    /// `main`) and is never reused within a process, so two rapid `new_window`
    /// calls cannot race to the same label.
    next_index: AtomicUsize,
    /// Project each not-yet-initialized window should open on, keyed by window
    /// label. Written by [`open_new_window`], consumed once by
    /// [`take_window_seed`], and cleared on window destruction for windows that
    /// never initialize — or, when the window never got built at all, in
    /// [`open_new_window`]'s own error arm.
    seeds: Mutex<HashMap<String, String>>,
}

impl NewWindowState {
    pub fn new() -> Self {
        Self {
            next_index: AtomicUsize::new(2),
            seeds: Mutex::new(HashMap::new()),
        }
    }

    /// Drop a window's unconsumed seed (called on window destruction).
    pub fn discard_seed(&self, label: &str) {
        self.seeds.lock().unwrap().remove(label);
    }
}

impl Default for NewWindowState {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide ownership of the frontend updater loop.
///
/// The updater UI still belongs to a webview, but exactly one live window may
/// check, prompt, and install at a time. Ownership is released by the native
/// `Destroyed` hook so a surviving peer can take over even when frontend
/// teardown does not run.
#[derive(Default)]
pub struct UpdaterWindowState {
    inner: Mutex<UpdaterWindowInner>,
}

#[derive(Default)]
struct UpdaterWindowInner {
    owner: Option<String>,
    /// Window labels are never reused within a process. Remembering destroyed
    /// ones rejects an IPC claim that was queued before destruction but did not
    /// reach the backend until after the native hook ran.
    destroyed: HashSet<String>,
}

impl UpdaterWindowState {
    fn try_claim(&self, label: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.owner.is_some() || inner.destroyed.contains(label) {
            return false;
        }
        inner.owner = Some(label.to_string());
        true
    }

    /// Record native destruction and release ownership if `label` held it.
    /// Returns whether peers should be notified that ownership is available.
    pub fn window_destroyed(&self, label: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        inner.destroyed.insert(label.to_string());
        if inner.owner.as_deref() != Some(label) {
            return false;
        }
        inner.owner = None;
        true
    }
}

/// Open a new full-peer app window, optionally seeded with the opener's
/// selected project. Returns the new window's label.
///
/// Labels are `win-N` to match the `win-*` glob in `capabilities/default.json`;
/// any other label would get no permissions, `invoke` would fail, and the
/// window would never show (the frontend only shows it after init).
///
/// Callable directly (not just through the [`new_window`] command) so the menu
/// handler can open a window when no window is focused and there is therefore
/// no frontend to round-trip through.
pub fn open_new_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    seed_project_id: Option<String>,
) -> Result<String, String> {
    let state = app.state::<NewWindowState>();
    let label = format!("win-{}", state.next_index.fetch_add(1, Ordering::Relaxed));

    // Cascade from the opener so the new window doesn't cover it exactly. The
    // window-state plugin only restores the main window, so this position is
    // not overridden.
    let cascade = focused_window(app).and_then(|opener| {
        let scale = opener.scale_factor().ok()?;
        let position = opener.outer_position().ok()?.to_logical::<f64>(scale);
        Some((position.x + CASCADE_OFFSET, position.y + CASCADE_OFFSET))
    });

    // Build from the `main` window's own conf entry rather than restating its
    // chrome here — one source of truth, so a conf tweak can't un-sync secondary
    // windows. The entry has no explicit `label`, so it parses as "main".
    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .ok_or("no `main` window entry in tauri.conf.json")?
        .clone();
    config.label = label.clone();

    let mut builder = tauri::WebviewWindowBuilder::from_config(app, &config)
        .map_err(|e| format!("Failed to build window from config: {e}"))?;

    // The conf sets no `x`/`y` (the window-state plugin positions `main`), so the
    // cascade is the only position setter — and it comes after `from_config`, so
    // it would still win if the conf ever gained one.
    if let Some((x, y)) = cascade {
        builder = builder.position(x, y);
    }

    // Seed before `build()`, not after: the consuming side (`take_window_seed`)
    // can only be invoked from a webview with this label, and no such webview
    // exists until `build()` creates it — so the seed is present before anyone
    // can ask for it, without a lock spanning window creation. Everything
    // fallible is deliberately above this line, leaving `build()` as the only
    // error exit that has to clean up.
    if let Some(project_id) = seed_project_id {
        state
            .seeds
            .lock()
            .unwrap()
            .insert(label.clone(), project_id);
    }

    let window = builder.build().map_err(|e| {
        // No window with this label ever existed, so the `Destroyed` hook that
        // normally discards an unconsumed seed can never fire for it — and
        // labels are never reused within a process, so nothing else would ever
        // remove the entry. Drop it here or it leaks until app exit.
        state.discard_seed(&label);
        format!("Failed to create window: {e}")
    })?;

    Ok(window.label().to_string())
}

/// Frontend entry point for [`open_new_window`]. Stays `async` — the convention
/// for window-creating commands, which keeps them off wry's synchronous-command
/// path — but is await-free, so there is no cancellation point between
/// [`open_new_window`]'s seed insert and the cleanup in its error arm.
#[tauri::command]
pub async fn new_window<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    seed_project_id: Option<String>,
) -> Result<String, String> {
    open_new_window(&app, seed_project_id)
}

/// One-shot read of the project this window was seeded with by its opener.
/// Consumes the seed; only `win-*` windows ever have one.
#[tauri::command]
pub fn take_window_seed(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, NewWindowState>,
) -> Option<String> {
    state.seeds.lock().unwrap().remove(window.label())
}

/// Atomically claim ownership of the app-wide updater loop for this window.
#[tauri::command]
pub fn claim_updater_ownership(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, UpdaterWindowState>,
) -> bool {
    state.try_claim(window.label())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::{mock_builder, mock_context, noop_assets};

    /// A mock app with `windows` in its config — `mock_context` starts empty, so
    /// tests that need [`open_new_window`]'s `main` lookup to succeed push a
    /// default entry (whose `Default` label is `main`).
    fn mock_app(
        windows: Vec<tauri::utils::config::WindowConfig>,
    ) -> tauri::App<tauri::test::MockRuntime> {
        let mut context = mock_context(noop_assets());
        context.config_mut().app.windows = windows;
        let app = mock_builder()
            .build(context)
            .expect("failed to build mock app");
        app.manage(NewWindowState::new());
        app
    }

    fn seeds_of(app: &tauri::App<tauri::test::MockRuntime>) -> HashMap<String, String> {
        app.state::<NewWindowState>().seeds.lock().unwrap().clone()
    }

    #[test]
    fn updater_ownership_transfers_only_after_the_owner_is_released() {
        let state = UpdaterWindowState::default();

        assert!(state.try_claim("main"));
        assert!(!state.try_claim("win-2"));
        assert!(!state.window_destroyed("win-2"));
        assert!(!state.try_claim("win-2"));

        assert!(state.window_destroyed("main"));
        assert!(!state.try_claim("main"));
        assert!(state.try_claim("win-3"));
    }

    /// The one fallible step after the seed insert must undo it: no window with
    /// this label ever exists, so the `Destroyed` hook can't, and labels are
    /// never reused, so nothing else ever will either.
    #[test]
    fn a_failed_build_discards_the_seed() {
        let app = mock_app(vec![Default::default()]);
        // Occupy the label the next `win-N` will mint, so `build()` fails the
        // manager's duplicate-label check — which runs before any runtime call,
        // making the failure deterministic on the mock runtime.
        tauri::WebviewWindowBuilder::new(&app, "win-2", Default::default())
            .build()
            .expect("failed to pre-create the colliding window");
        app.state::<NewWindowState>()
            .seeds
            .lock()
            .unwrap()
            .insert("win-99".into(), "other-project".into());

        let result = open_new_window(app.handle(), Some("proj-1".into()));

        let error = result.expect_err("expected a duplicate-label build failure");
        assert!(
            error.starts_with("Failed to create window"),
            "expected the failure to come from `build()`, not an earlier step: {error}"
        );
        assert_eq!(
            seeds_of(&app),
            HashMap::from([("win-99".to_string(), "other-project".to_string())]),
            "the failed window's seed should be discarded, and only that one"
        );
    }

    /// Pins the ordering: every fallible step other than `build()` sits above
    /// the insert, so it exits with nothing to clean up. Moving the insert back
    /// to the top of the function fails this.
    #[test]
    fn a_failure_before_the_build_never_inserts_a_seed() {
        // No windows in the config, so the `main` lookup fails.
        let app = mock_app(Vec::new());

        let result = open_new_window(app.handle(), Some("proj-1".into()));

        assert!(result.is_err(), "expected the `main` config lookup to fail");
        assert!(
            seeds_of(&app).is_empty(),
            "no seed should have been inserted"
        );
    }

    /// [`new_window`] clones the conf's `main` window entry. Giving that entry an
    /// explicit non-`main` label would turn every New Window into a runtime error, so
    /// catch it here instead. (An entry with no `label` parses as `main`.)
    #[test]
    fn conf_has_a_main_window_entry_for_new_window_to_clone() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let windows = conf["app"]["windows"]
            .as_array()
            .expect("tauri.conf.json has an app.windows array");

        assert!(
            windows
                .iter()
                .any(|w| w.get("label").is_none_or(|l| l == "main")),
            "no window entry labelled `main` (explicitly or by default); \
             new_window has nothing to clone: {windows:?}"
        );
    }

    /// Per-window titles (`windowTitle.ts`) call `setTitle`, which is *not* in
    /// tauri's `core:window` default permission set — that set is getters only
    /// (`allow-title` is there, `allow-set-title` is not). Dropping the grant
    /// fails at runtime, not at build time: every title update rejects and the
    /// Window menu silently keeps saying "Staged" in every window. The `win-*`
    /// glob has to survive too, or only the first window gets titled.
    #[test]
    fn capabilities_grant_set_title_to_every_window() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();

        let permissions = capability["permissions"]
            .as_array()
            .expect("capabilities/default.json has a permissions array");
        assert!(
            permissions
                .iter()
                .any(|p| p == "core:window:allow-set-title"),
            "core:window:allow-set-title is missing; window titles will fail at \
             runtime: {permissions:?}"
        );

        let windows = capability["windows"]
            .as_array()
            .expect("capabilities/default.json has a windows array");
        assert!(
            windows.iter().any(|w| w == "win-*"),
            "the capability no longer covers secondary `win-N` windows: {windows:?}"
        );
    }
}
