//! The user's side of class B enrolment: showing a connection request,
//! recording the answer, and the Settings view of approved clients.
//!
//! A request arriving on the control socket is announced to the frontend
//! with a bare `app.emit` of [`AUTHORIZATION_REQUESTED_EVENT`] — not
//! `web_server::emit_to_all`, which also feeds the browser-facing event
//! stream and would broadcast consent prompts to remote clients if that
//! server is ever re-enabled — and the main window is shown and focused so
//! the dialog is seen. When no window exists to show, the native alert the
//! quit gate uses asks instead.
//!
//! Approval records the decision on the pending request and nothing else:
//! the durable `control_grants` row, the reconnect secret and the live
//! bearer are minted by [`mint_collected_approval`] only when the CLI's poll
//! collects the answer (`enrol::poll_connect`). An approval the CLI never
//! collects — it gave up before the user answered, or the app quit first —
//! therefore persists nothing: no "connected" client can exist that nobody
//! holds the secret for, across a restart included. Collection, denial and
//! revocation end in [`GRANTS_CHANGED_EVENT`] so the Settings panel
//! refreshes.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

use super::enrol::{secret_hash, AnswerError, ApprovedGrant, Decision, PendingAuthorization};
use super::grants::{Access, Scope};
use super::ControlGrants;
use crate::store::{now_timestamp, renewed_expiry, ControlGrantRecord, Store};

/// Emitted with a [`ControlAuthorizationRequest`](super::enrol::ControlAuthorizationRequest)
/// payload when a client asks to connect.
pub const AUTHORIZATION_REQUESTED_EVENT: &str = "control-authorization-requested";
/// Emitted (no payload) whenever the set of approved clients changes.
pub const GRANTS_CHANGED_EVENT: &str = "control-grants-changed";

const APPROVE_BUTTON: &str = "Approve";
const DENY_BUTTON: &str = "Deny";

/// The Settings panel's row for one approved client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlGrantSummary {
    pub id: String,
    pub client: String,
    /// `None` = every project.
    pub project_id: Option<String>,
    /// The scoped project's current name, when it still exists.
    pub project_name: Option<String>,
    /// `"read"` or `"write"`.
    pub access: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_used_at: Option<i64>,
    /// Whether the client holds a live bearer in this run of the app.
    pub live: bool,
}

/// The store, or why a credential operation cannot proceed.
pub(crate) fn store_of(app: &AppHandle) -> Result<Arc<Store>, String> {
    app.try_state::<Mutex<Option<Arc<Store>>>>()
        .and_then(|slot| slot.lock().unwrap().clone())
        .ok_or_else(|| "Database not initialized".to_string())
}

pub(crate) fn emit_grants_changed(app: &AppHandle) {
    if let Err(e) = app.emit(GRANTS_CHANGED_EVENT, ()) {
        log::warn!("[control_server] failed to emit {GRANTS_CHANGED_EVENT}: {e}");
    }
}

/// Announce `pending` to the frontend and bring the main window forward;
/// with no window at all, ask through the native alert.
pub(crate) fn present_request(app: &AppHandle, pending: &PendingAuthorization) {
    let summary = pending.summary();
    if let Err(e) = app.emit(AUTHORIZATION_REQUESTED_EVENT, &summary) {
        log::warn!("[control_server] failed to emit {AUTHORIZATION_REQUESTED_EVENT}: {e}");
    }
    if !show_main_window(app) {
        ask_natively(app, pending);
    }
}

/// Show, unminimize and focus the main window (or any window if `main` is
/// gone). `false` when the app has no window left to show — which no close
/// path produces, since closing the last window hides it.
fn show_main_window(app: &AppHandle) -> bool {
    let windows = app.webview_windows();
    let Some(window) = windows
        .get(crate::app_lifecycle::MAIN_WINDOW_LABEL)
        .or_else(|| windows.values().next())
    else {
        return false;
    };
    for (what, result) in [
        ("show", window.show()),
        ("unminimize", window.unminimize()),
        ("focus", window.set_focus()),
    ] {
        if let Err(e) = result {
            log::warn!("[control_server] failed to {what} the window for a consent prompt: {e}");
        }
    }
    true
}

/// The native fallback: the same window-independent alert as the quit
/// gate. Anything but the Approve button — Deny, or the system dismissing
/// the alert — denies.
fn ask_natively(app: &AppHandle, pending: &PendingAuthorization) {
    let app = app.clone();
    let grants = app.state::<ControlGrants>().inner().clone();
    let request_id = pending.id.clone();
    app.dialog()
        .message(native_prompt_message(pending))
        .title(format!(
            "Allow “{}” to control Staged?",
            pending.request.client
        ))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            APPROVE_BUTTON.to_string(),
            DENY_BUTTON.to_string(),
        ))
        .show_with_result(move |result| {
            let approved =
                matches!(result, MessageDialogResult::Custom(label) if label == APPROVE_BUTTON);
            let outcome = if approved {
                approve(&grants, &request_id)
            } else {
                deny(&grants, &request_id)
            };
            if let Err(e) = outcome {
                log::warn!("[control_server] native consent answer not recorded: {e}");
            }
        });
}

fn native_prompt_message(pending: &PendingAuthorization) -> String {
    let request = &pending.request;
    let scope = match (&request.scope, &request.project_name) {
        (Scope::AllProjects, _) => "every project".to_string(),
        (Scope::Project(_), Some(name)) => format!("the project “{name}”"),
        (Scope::Project(id), None) => format!("project {id}"),
    };
    let access = match request.access {
        Access::ReadOnly => "read it",
        Access::ReadWrite => "read it and start, cancel and attach to work",
    };
    let peer = match (
        request.peer_pid,
        &request.peer_parent,
        request.peer_orphaned,
    ) {
        (Some(pid), Some(parent), _) => format!("\n\nAsked by process {pid} (run from {parent})."),
        (Some(pid), None, true) => format!(
            "\n\nAsked by process {pid}, whose parent process has already exited — it may have \
             been backgrounded from an agent session. Approve only if you ran this command \
             yourself."
        ),
        (Some(pid), None, false) => format!("\n\nAsked by process {pid}."),
        _ => String::new(),
    };
    format!(
        "A program calling itself “{}” wants to drive Staged through stagedctl, over {scope}, \
         and to {access}. Approving lets it reconnect without asking for thirty days; you can \
         revoke it any time under Settings ▸ Connected apps.{peer}",
        request.client
    )
}

/// Approve `request_id`: record the decision for the waiting CLI to
/// collect. Nothing is minted here — see [`mint_collected_approval`] — so
/// an approval nobody collects leaves no trace.
pub(crate) fn approve(grants: &ControlGrants, request_id: &str) -> Result<(), String> {
    let pending = grants
        .enrolment()
        .answer(request_id, Decision::Approved, now_timestamp())
        .map_err(answer_error_message)?;
    log::info!(
        "[control_server] approved connected app {:?} (request {request_id}); credentials are \
         minted when the client collects them",
        pending.request.client
    );
    Ok(())
}

/// Mint the credentials for an approval the CLI is collecting right now:
/// the durable row (storing only the secret's hash), the reconnect secret
/// the client is handed once, and this run's live bearer. Runs when
/// `poll_connect` hands the answer over, never at approval time, so the row
/// exists only once a client holds its secret. The caller emits
/// [`GRANTS_CHANGED_EVENT`].
pub(crate) fn mint_collected_approval(
    store: &Store,
    grants: &ControlGrants,
    pending: &PendingAuthorization,
    now: i64,
) -> Result<ApprovedGrant, String> {
    let request = &pending.request;
    let grant_id = uuid::Uuid::new_v4().to_string();
    let reconnect_secret = crate::web_server::generate_token();
    let record = ControlGrantRecord {
        id: grant_id.clone(),
        client: request.client.clone(),
        scope_project_id: request.scope.project_id().map(str::to_string),
        access: request.access.wire().to_string(),
        reconnect_secret_hash: secret_hash(&reconnect_secret),
        created_at: now,
        expires_at: renewed_expiry(now),
        last_used_at: Some(now),
        revoked_at: None,
    };
    store
        .create_control_grant(&record)
        .map_err(|e| format!("record the approval: {e}"))?;
    let token = grants.mint_app_bearer(
        &grant_id,
        &request.client,
        request.scope.clone(),
        request.access,
        record.expires_at,
    );
    log::info!(
        "[control_server] connected app {:?} collected its approval as grant {grant_id}",
        request.client
    );
    Ok(ApprovedGrant {
        grant_id,
        token,
        reconnect_secret,
        expires_at: record.expires_at,
    })
}

/// Deny `request_id`; the waiting CLI reads `authorization_denied`.
pub(crate) fn deny(grants: &ControlGrants, request_id: &str) -> Result<(), String> {
    let pending = grants
        .enrolment()
        .answer(request_id, Decision::Denied, now_timestamp())
        .map_err(answer_error_message)?;
    log::info!(
        "[control_server] denied connected app {:?}",
        pending.request.client
    );
    Ok(())
}

fn answer_error_message(error: AnswerError) -> String {
    match error {
        AnswerError::Unknown => "that connection request is no longer pending".to_string(),
        AnswerError::Expired => {
            "that connection request expired before it was answered".to_string()
        }
        AnswerError::AlreadyAnswered => "that connection request was already answered".to_string(),
    }
}

/// Revoke grant `grant_id` from the Settings panel: stamp the row (inline;
/// the Tauri command calls this on the blocking pool), then retire it.
/// Returns whether the row was live.
pub(crate) fn revoke_grant(
    app: &AppHandle,
    grants: &ControlGrants,
    grant_id: &str,
) -> Result<bool, String> {
    let was_live = store_of(app)?
        .revoke_control_grant(grant_id)
        .map_err(|e| e.to_string())?;
    retire_revoked_grant(app, grants, grant_id, was_live);
    Ok(was_live)
}

/// The in-memory half of a revocation, after the row has been stamped
/// (`was_live` is what that write reported): drop every live bearer, forget
/// the touch throttle, and tell the Settings panel when anything changed.
pub(crate) fn retire_revoked_grant(
    app: &AppHandle,
    grants: &ControlGrants,
    grant_id: &str,
    was_live: bool,
) {
    let bearers = grants.revoke_app_grant(grant_id);
    grants.enrolment().forget_touch(grant_id);
    if was_live || bearers > 0 {
        log::info!(
            "[control_server] revoked connected app grant {grant_id} ({bearers} live bearers)"
        );
        emit_grants_changed(app);
    }
}

/// The Settings rows: every unrevoked grant with its project's current name
/// and whether it is live in this run.
pub(crate) fn grant_summaries(
    store: &Store,
    grants: &ControlGrants,
) -> Result<Vec<ControlGrantSummary>, String> {
    let records = store.list_control_grants().map_err(|e| e.to_string())?;
    records
        .into_iter()
        .map(|record| {
            let project_name = match &record.scope_project_id {
                Some(project_id) => store
                    .get_project(project_id)
                    .map_err(|e| e.to_string())?
                    .map(|project| project.name),
                None => None,
            };
            Ok(ControlGrantSummary {
                live: grants.app_grant_is_live(&record.id),
                id: record.id,
                client: record.client,
                project_id: record.scope_project_id,
                project_name,
                access: record.access,
                created_at: record.created_at,
                expires_at: record.expires_at,
                last_used_at: record.last_used_at,
            })
        })
        .collect()
}

/// The name of the parent of process `pid`, one level up: the shell or
/// agent that ran `stagedctl`. Best effort, for the dialog only.
#[cfg(unix)]
pub(crate) fn peer_parent_process(pid: i32) -> Option<String> {
    fn ps(args: &[&str]) -> Option<String> {
        let output = std::process::Command::new("ps").args(args).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!text.is_empty()).then_some(text)
    }
    let ppid = ps(&["-o", "ppid=", "-p", &pid.to_string()])?;
    let ppid: i32 = ppid.trim().parse().ok()?;
    if ppid <= 1 {
        return None;
    }
    let command = ps(&["-o", "comm=", "-p", &ppid.to_string()])?;
    Some(
        std::path::Path::new(&command)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or(command),
    )
}

#[cfg(not(unix))]
pub(crate) fn peer_parent_process(_pid: i32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_server::enrol::ConnectRequest;

    fn pending(scope: Scope, project_name: Option<&str>, access: Access) -> PendingAuthorization {
        PendingAuthorization {
            id: "r1".to_string(),
            request: ConnectRequest {
                client: "my-agent".to_string(),
                scope,
                project_name: project_name.map(str::to_string),
                access,
                peer_pid: Some(4242),
                peer_parent: Some("zsh".to_string()),
                peer_orphaned: false,
            },
            requested_at: 0,
            expires_at: 1,
            decision: None,
        }
    }

    #[test]
    fn native_prompt_names_the_client_scope_access_and_peer() {
        let text = native_prompt_message(&pending(
            Scope::Project("p1".to_string()),
            Some("Widgets"),
            Access::ReadOnly,
        ));
        assert!(text.contains("“my-agent”"), "{text}");
        assert!(text.contains("the project “Widgets”"), "{text}");
        assert!(text.contains("to read it."), "{text}");
        assert!(text.contains("process 4242 (run from zsh)"), "{text}");

        let text = native_prompt_message(&pending(Scope::AllProjects, None, Access::ReadWrite));
        assert!(text.contains("every project"), "{text}");
        assert!(text.contains("start, cancel"), "{text}");
    }

    /// The native fallback says plainly when the asker's parent is gone —
    /// the shape a `stagedctl connect` backgrounded from an agent session
    /// takes — and says nothing extra for a peer that is merely unknown.
    #[test]
    fn native_prompt_flags_an_orphaned_asker() {
        let mut orphaned = pending(Scope::AllProjects, None, Access::ReadWrite);
        orphaned.request.peer_parent = None;
        orphaned.request.peer_orphaned = true;
        let text = native_prompt_message(&orphaned);
        assert!(
            text.contains("process 4242, whose parent process has already exited"),
            "{text}"
        );
        assert!(
            text.contains("backgrounded from an agent session"),
            "{text}"
        );

        let mut unknown_parent = pending(Scope::AllProjects, None, Access::ReadWrite);
        unknown_parent.request.peer_parent = None;
        let text = native_prompt_message(&unknown_parent);
        assert!(text.ends_with("Asked by process 4242."), "{text}");
        assert!(!text.contains("already exited"), "{text}");
    }

    #[test]
    fn summaries_name_the_project_and_report_liveness() {
        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::named("Widgets");
        store.create_project(&project).unwrap();
        let grants = ControlGrants::default();
        for (id, scope) in [("g-scoped", Some(project.id.clone())), ("g-all", None)] {
            store
                .create_control_grant(&ControlGrantRecord {
                    id: id.to_string(),
                    client: "agent".to_string(),
                    scope_project_id: scope,
                    access: "write".to_string(),
                    reconnect_secret_hash: format!("{id}-hash"),
                    created_at: 1,
                    expires_at: 2,
                    last_used_at: None,
                    revoked_at: None,
                })
                .unwrap();
        }
        grants.mint_app_bearer("g-all", "agent", Scope::AllProjects, Access::ReadWrite, 2);
        let summaries = grant_summaries(&store, &grants).unwrap();
        let by_id = |id: &str| summaries.iter().find(|s| s.id == id).unwrap().clone();
        assert_eq!(by_id("g-scoped").project_name.as_deref(), Some("Widgets"));
        assert!(!by_id("g-scoped").live);
        assert_eq!(by_id("g-all").project_name, None);
        assert!(by_id("g-all").live);
    }

    /// An approval is only a parked decision until the CLI collects it:
    /// if the CLI never polls, the request expires with no row, no secret
    /// and no bearer ever having existed — nothing to retire, and nothing
    /// for a restart to resurrect.
    #[test]
    fn an_uncollected_approval_persists_nothing() {
        use crate::control_server::enrol::PollOutcome;

        let store = Store::in_memory().unwrap();
        let grants = ControlGrants::default();
        let enrol = grants.enrolment();
        let now = 1_000;
        let parked = enrol
            .request(
                pending(Scope::AllProjects, None, Access::ReadWrite).request,
                now,
            )
            .unwrap();
        enrol
            .answer(&parked.id, Decision::Approved, now + 1)
            .unwrap();

        // Approved, but not collected: still nothing anywhere but the
        // pending map.
        assert!(store.list_control_grants().unwrap().is_empty());
        assert_eq!(grants.live_count(), 0);
        assert!(
            enrol.pending(now + 2).is_empty(),
            "answered: no longer prompts"
        );

        // The CLI gave up; the request expires unanswered-for-collection.
        assert_eq!(
            enrol.poll(&parked.id, parked.expires_at),
            PollOutcome::Expired
        );
        assert!(store.list_control_grants().unwrap().is_empty());
        assert_eq!(grants.live_count(), 0);
        assert_eq!(
            enrol.poll(&parked.id, parked.expires_at + 1),
            PollOutcome::Unknown
        );
    }

    /// Collection mints exactly what the CLI is handed: a row keyed by the
    /// secret's hash, one lifetime out, and a live class B bearer.
    #[test]
    fn collecting_an_approval_mints_the_row_and_the_bearer() {
        use crate::control_server::enrol::PollOutcome;
        use crate::store::renewed_expiry;

        let store = Store::in_memory().unwrap();
        let project = crate::store::Project::named("Widgets");
        store.create_project(&project).unwrap();
        let grants = ControlGrants::default();
        let enrol = grants.enrolment();
        let now = 1_000;
        let parked = enrol
            .request(
                pending(
                    Scope::Project(project.id.clone()),
                    Some("Widgets"),
                    Access::ReadOnly,
                )
                .request,
                now,
            )
            .unwrap();
        enrol
            .answer(&parked.id, Decision::Approved, now + 1)
            .unwrap();
        let PollOutcome::Approved(collected) = enrol.poll(&parked.id, now + 2) else {
            panic!("approved request is handed over");
        };
        let approved = mint_collected_approval(&store, &grants, &collected, now + 2).unwrap();

        let record = store
            .get_control_grant(&approved.grant_id)
            .unwrap()
            .expect("row written at collection");
        assert_eq!(record.client, "my-agent");
        assert_eq!(
            record.scope_project_id.as_deref(),
            Some(project.id.as_str())
        );
        assert_eq!(record.access, "read");
        assert_eq!(
            record.reconnect_secret_hash,
            secret_hash(&approved.reconnect_secret),
            "only the hash is stored"
        );
        assert_eq!(record.expires_at, renewed_expiry(now + 2));
        assert_eq!(approved.expires_at, record.expires_at);
        assert_eq!(record.last_used_at, Some(now + 2));
        assert!(!record.is_revoked());

        let grant = grants.authorize_at(&approved.token, now + 3).unwrap();
        assert_eq!(
            grant.actor,
            crate::control_server::Actor::App {
                grant_id: approved.grant_id.clone(),
                client: "my-agent".to_string(),
            }
        );
        assert_eq!(grant.scope, Scope::Project(project.id.clone()));
        assert_eq!(grant.access, Access::ReadOnly);
        assert!(grants.app_grant_is_live(&approved.grant_id));
        assert_eq!(
            enrol.poll(&parked.id, now + 3),
            PollOutcome::Unknown,
            "handed over once"
        );
    }

    #[test]
    #[cfg(unix)]
    fn peer_parent_process_names_our_own_parent() {
        // Whatever runs the test binary (cargo, a shell, an IDE) is a real
        // process with a name; pid 1's parent is nothing.
        let ours = peer_parent_process(std::process::id() as i32);
        assert!(ours.is_some_and(|name| !name.is_empty()));
        assert_eq!(peer_parent_process(1), None);
    }
}
