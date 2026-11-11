//! Class B enrolment: how an outside client becomes a [`ControlGrant`].
//!
//! A client that is not a Staged-spawned session has no credential. It asks
//! for one with `stagedctl connect --client <name>`, which `POST`s
//! `/api/connect` on the control socket — the one unauthenticated call the
//! server takes beyond `ping`. The request is parked here as a
//! [`PendingAuthorization`], the app shows the consent dialog (or, with no
//! window to show it in, the native alert the quit gate uses), and the CLI
//! long-polls `GET /api/connect/{id}` for the answer. Approval mints a live
//! bearer for this run of the app *and* a durable row in `control_grants`
//! keyed by a reconnect secret the client is handed exactly once; on later
//! launches the client proves it still holds the secret
//! (`POST /api/reconnect`) and gets a fresh bearer without another prompt.
//!
//! **Pending requests** are capped at [`PENDING_CAP`] and expire after
//! [`PENDING_TTL_MS`], so an unattended app cannot be made to accumulate
//! prompts. **Nonces** for the reconnect proof are single-use and expire
//! after [`NONCE_TTL_MS`].
//!
//! **The proof.** The durable row stores `hash = SHA-256(secret)` and the
//! client proves possession with `HMAC-SHA256(key = hash, msg =
//! "<grant-id>\n<nonce>")`: the stored hash is the verification key, so the
//! secret itself never crosses the socket after enrolment, and a copy of the
//! database alone does not reveal it. (A copy of the database *does* let an
//! attacker who can also reach the socket impersonate the client — as it
//! would with any stored verifier; the socket is same-user only.)

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::grants::{Access, Scope};
use super::ControlGrants;

/// Most unanswered connection requests held at once; the next is refused.
pub(crate) const PENDING_CAP: usize = 16;
/// How long a connection request waits for an answer.
pub(crate) const PENDING_TTL_MS: i64 = 5 * 60 * 1000;
/// How long a reconnect nonce stays valid.
pub(crate) const NONCE_TTL_MS: i64 = 60 * 1000;
/// Most outstanding nonces; the oldest is forgotten past this.
const NONCE_CAP: usize = 64;
/// Longest `--client` name the server accepts.
pub(crate) const MAX_CLIENT_NAME_CHARS: usize = 64;
/// `last_used_at` is written at most this often per grant.
pub(crate) const TOUCH_INTERVAL_MS: i64 = 60 * 1000;

/// What `POST /api/connect` asks for, after validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectRequest {
    pub client: String,
    pub scope: Scope,
    /// The scoped project's display name, for the dialog.
    pub project_name: Option<String>,
    pub access: Access,
    /// The connecting `stagedctl` process, as the kernel reported it.
    pub peer_pid: Option<i32>,
    /// The name of that process's parent (the shell or agent that ran the
    /// CLI), one level up. Information for the dialog only.
    pub peer_parent: Option<String>,
}

/// An approval's credentials, handed to the CLI once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApprovedGrant {
    pub grant_id: String,
    /// The live bearer for this run of the app.
    pub token: String,
    /// The durable secret; only its hash is stored.
    pub reconnect_secret: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Approved(ApprovedGrant),
    Denied,
}

/// A connection request awaiting (or holding) its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingAuthorization {
    pub id: String,
    pub request: ConnectRequest,
    pub requested_at: i64,
    pub expires_at: i64,
    pub decision: Option<Decision>,
}

impl PendingAuthorization {
    /// The consent dialog's view of the request.
    pub fn summary(&self) -> ControlAuthorizationRequest {
        ControlAuthorizationRequest {
            request_id: self.id.clone(),
            client: self.request.client.clone(),
            project_id: self.request.scope.project_id().map(str::to_string),
            project_name: self.request.project_name.clone(),
            access: self.request.access.wire(),
            peer_pid: self.request.peer_pid,
            peer_parent: self.request.peer_parent.clone(),
            requested_at: self.requested_at,
            expires_at: self.expires_at,
        }
    }
}

/// The `control-authorization-requested` payload and the
/// `list_pending_control_authorizations` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlAuthorizationRequest {
    pub request_id: String,
    pub client: String,
    /// `None` = every project.
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    /// `"read"` or `"write"`.
    pub access: &'static str,
    pub peer_pid: Option<i32>,
    pub peer_parent: Option<String>,
    pub requested_at: i64,
    pub expires_at: i64,
}

/// What the CLI's poll learns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PollOutcome {
    Pending {
        expires_at: i64,
    },
    Approved(ApprovedGrant),
    Denied,
    /// The request expired unanswered (or was collected already).
    Expired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestError {
    TooManyPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnswerError {
    Unknown,
    Expired,
    AlreadyAnswered,
}

#[derive(Default)]
pub(super) struct EnrolInner {
    pending: HashMap<String, PendingAuthorization>,
    /// nonce → (grant id, expiry), oldest first.
    nonces: VecDeque<(String, String, i64)>,
    /// grant id → last `last_used_at` write.
    touched: HashMap<String, i64>,
}

/// The enrolment state, shared by the control server's handlers and the
/// consent commands. Lives inside [`ControlGrants`] so one managed value
/// carries every credential-related thing.
#[derive(Clone, Default)]
pub struct Enrolment {
    inner: Arc<Mutex<EnrolInner>>,
}

impl Enrolment {
    fn prune(inner: &mut EnrolInner, now: i64) {
        // An answered request stays until the CLI collects it, but not past
        // its expiry: a CLI that gave up must not leave credentials parked.
        inner.pending.retain(|_, p| p.expires_at > now);
        inner.nonces.retain(|(_, _, expires)| *expires > now);
    }

    /// Park a request. Refused when [`PENDING_CAP`] unanswered requests are
    /// already waiting.
    pub(crate) fn request(
        &self,
        request: ConnectRequest,
        now: i64,
    ) -> Result<PendingAuthorization, RequestError> {
        let mut inner = self.inner.lock().unwrap();
        Self::prune(&mut inner, now);
        let unanswered = inner
            .pending
            .values()
            .filter(|p| p.decision.is_none())
            .count();
        if unanswered >= PENDING_CAP {
            return Err(RequestError::TooManyPending);
        }
        let pending = PendingAuthorization {
            id: uuid::Uuid::new_v4().to_string(),
            request,
            requested_at: now,
            expires_at: now + PENDING_TTL_MS,
            decision: None,
        };
        inner.pending.insert(pending.id.clone(), pending.clone());
        Ok(pending)
    }

    /// The unanswered requests, oldest first — the consent dialog's queue.
    pub(crate) fn pending(&self, now: i64) -> Vec<PendingAuthorization> {
        let mut inner = self.inner.lock().unwrap();
        Self::prune(&mut inner, now);
        let mut list: Vec<_> = inner
            .pending
            .values()
            .filter(|p| p.decision.is_none())
            .cloned()
            .collect();
        list.sort_by(|a, b| a.requested_at.cmp(&b.requested_at).then(a.id.cmp(&b.id)));
        list
    }

    /// The unanswered request `id`, for the approver to build a grant from.
    pub(crate) fn get_unanswered(
        &self,
        id: &str,
        now: i64,
    ) -> Result<PendingAuthorization, AnswerError> {
        let mut inner = self.inner.lock().unwrap();
        Self::prune(&mut inner, now);
        match inner.pending.get(id) {
            None => Err(AnswerError::Unknown),
            Some(p) if p.decision.is_some() => Err(AnswerError::AlreadyAnswered),
            Some(p) => Ok(p.clone()),
        }
    }

    /// Record the user's answer. A request that expired while the dialog
    /// was open is `Expired`; a second answer is `AlreadyAnswered`.
    pub(crate) fn answer(
        &self,
        id: &str,
        decision: Decision,
        now: i64,
    ) -> Result<PendingAuthorization, AnswerError> {
        let mut inner = self.inner.lock().unwrap();
        let expired = inner.pending.get(id).is_some_and(|p| p.expires_at <= now);
        Self::prune(&mut inner, now);
        let Some(pending) = inner.pending.get_mut(id) else {
            return Err(if expired {
                AnswerError::Expired
            } else {
                AnswerError::Unknown
            });
        };
        if pending.decision.is_some() {
            return Err(AnswerError::AlreadyAnswered);
        }
        pending.decision = Some(decision);
        Ok(pending.clone())
    }

    /// The CLI's poll. An answered request is handed over once and
    /// forgotten; an expired one reads `Expired` (and is forgotten); an id
    /// this instance never issued is `Unknown`.
    pub(crate) fn poll(&self, id: &str, now: i64) -> PollOutcome {
        let mut inner = self.inner.lock().unwrap();
        let expired = inner.pending.get(id).is_some_and(|p| p.expires_at <= now);
        Self::prune(&mut inner, now);
        if expired {
            return PollOutcome::Expired;
        }
        let Some(pending) = inner.pending.get(id) else {
            return PollOutcome::Unknown;
        };
        match &pending.decision {
            None => PollOutcome::Pending {
                expires_at: pending.expires_at,
            },
            Some(_) => match inner.pending.remove(id).and_then(|p| p.decision) {
                Some(Decision::Approved(grant)) => PollOutcome::Approved(grant),
                Some(Decision::Denied) => PollOutcome::Denied,
                None => PollOutcome::Unknown,
            },
        }
    }

    /// A fresh single-use nonce bound to `grant_id`.
    pub(crate) fn issue_nonce(&self, grant_id: &str, now: i64) -> String {
        let nonce = crate::web_server::generate_token();
        let mut inner = self.inner.lock().unwrap();
        Self::prune(&mut inner, now);
        inner
            .nonces
            .push_back((nonce.clone(), grant_id.to_string(), now + NONCE_TTL_MS));
        while inner.nonces.len() > NONCE_CAP {
            inner.nonces.pop_front();
        }
        nonce
    }

    /// Consume `nonce` if it is live and was issued for `grant_id`.
    pub(crate) fn take_nonce(&self, nonce: &str, grant_id: &str, now: i64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        Self::prune(&mut inner, now);
        let position = inner
            .nonces
            .iter()
            .position(|(n, g, _)| n == nonce && g == grant_id);
        match position {
            Some(index) => {
                inner.nonces.remove(index);
                true
            }
            None => false,
        }
    }

    /// Whether a `last_used_at` write is due for `grant_id`; records the
    /// write when it is.
    pub(crate) fn touch_due(&self, grant_id: &str, now: i64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let due = match inner.touched.get(grant_id) {
            Some(last) => now - *last >= TOUCH_INTERVAL_MS,
            None => true,
        };
        if due {
            inner.touched.insert(grant_id.to_string(), now);
        }
        due
    }

    /// Forget the throttle entry when a grant is revoked.
    pub(crate) fn forget_touch(&self, grant_id: &str) {
        self.inner.lock().unwrap().touched.remove(grant_id);
    }
}

impl ControlGrants {
    /// The enrolment state carried alongside the live grants.
    pub(crate) fn enrolment(&self) -> &Enrolment {
        &self.enrolment
    }
}

// =============================================================================
// The reconnect proof
// =============================================================================

/// Hex SHA-256 of a reconnect secret: what the `control_grants` row stores.
pub(crate) fn secret_hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// The message a reconnect proof signs.
fn proof_message(grant_id: &str, nonce: &str) -> Vec<u8> {
    format!("{grant_id}\n{nonce}").into_bytes()
}

/// HMAC-SHA256 (RFC 2104) over `message` with `key`. Spelled out rather than
/// pulled from a crate so the CLI, which cannot link this crate, can carry
/// the identical twenty lines; both sides pin RFC 4231's test vectors.
pub(crate) fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block_key = [0u8; BLOCK];
    if key.len() > BLOCK {
        block_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block_key.map(|b| b ^ 0x36));
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block_key.map(|b| b ^ 0x5c));
    outer.update(inner_hash);
    outer.finalize().into()
}

/// The proof the CLI sends for `grant_id` and `nonce`, given the stored
/// hash (the verification key). Hex.
pub(crate) fn expected_proof(secret_hash_hex: &str, grant_id: &str, nonce: &str) -> Option<String> {
    let key = hex::decode(secret_hash_hex).ok()?;
    Some(hex::encode(hmac_sha256(
        &key,
        &proof_message(grant_id, nonce),
    )))
}

/// Constant-time check of a presented proof against the stored hash.
pub(crate) fn proof_matches(
    secret_hash_hex: &str,
    grant_id: &str,
    nonce: &str,
    proof: &str,
) -> bool {
    match expected_proof(secret_hash_hex, grant_id, nonce) {
        Some(expected) => expected.as_bytes().ct_eq(proof.as_bytes()).into(),
        None => false,
    }
}

/// Validation of a `--client` name: non-empty, bounded, printable.
pub(crate) fn validate_client_name(client: &str) -> Result<String, String> {
    let trimmed = client.trim();
    if trimmed.is_empty() {
        return Err("client name must not be empty".to_string());
    }
    if trimmed.chars().count() > MAX_CLIENT_NAME_CHARS {
        return Err(format!(
            "client name must be at most {MAX_CLIENT_NAME_CHARS} characters"
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("client name must not contain control characters".to_string());
    }
    Ok(trimmed.to_string())
}

// =============================================================================
// HTTP: the enrolment routes
// =============================================================================

use axum::extract::{Json, Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::consent;
use super::grants::AuthorizationFailure;
use super::{authorization_response, ControlState};
use crate::store::{now_timestamp, ControlGrantRecord, Store};

/// How long `GET /api/connect/{id}` holds a pending request before
/// answering `pending` and letting the CLI ask again.
const LONG_POLL_MS: u64 = 25_000;
const LONG_POLL_STEP_MS: u64 = 250;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConnectBody {
    client: String,
    /// A project name or id to confine the grant to; absent = every project.
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    read_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChallengeBody {
    grant_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProofBody {
    grant_id: String,
    nonce: String,
    proof: String,
}

fn bad_request(message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": message.into() })),
    )
        .into_response()
}

fn refusal(failure: AuthorizationFailure) -> Response {
    authorization_response(StatusCode::UNAUTHORIZED, failure.code(), failure.message())
}

/// Run a store operation off the async runtime.
async fn with_store<T, F>(state: &ControlState, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Store) -> Result<T, String> + Send + 'static,
{
    let store = consent::store_of(&state.app_handle)?;
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| format!("store task failed: {e}"))?
}

/// The project `needle` names: its id, or its name (case-insensitively,
/// and only when exactly one project has it).
pub(crate) fn resolve_project(store: &Store, needle: &str) -> Result<(String, String), String> {
    let needle = needle.trim();
    if let Some(project) = store.get_project(needle).map_err(|e| e.to_string())? {
        return Ok((project.id, project.name));
    }
    let projects = store.list_projects().map_err(|e| e.to_string())?;
    let matches: Vec<_> = projects
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(needle))
        .collect();
    match matches.as_slice() {
        [project] => Ok((project.id.clone(), project.name.clone())),
        [] => Err(format!(
            "no project named {needle:?} (names and ids are accepted)"
        )),
        _ => Err(format!(
            "{} projects are named {needle:?}; pass the project id instead",
            matches.len()
        )),
    }
}

/// `POST /api/connect`: park a request and prompt the user.
pub(super) async fn connect(
    State(state): State<ControlState>,
    parts: Parts,
    Json(body): Json<ConnectBody>,
) -> Response {
    let client = match validate_client_name(&body.client) {
        Ok(client) => client,
        Err(e) => return bad_request(e),
    };
    let (scope, project_name) = match body.project.as_deref().filter(|p| !p.trim().is_empty()) {
        None => (Scope::AllProjects, None),
        Some(needle) => {
            let needle = needle.to_string();
            match with_store(&state, move |store| resolve_project(store, &needle)).await {
                Ok((id, name)) => (Scope::Project(id), Some(name)),
                Err(e) => return bad_request(e),
            }
        }
    };
    let access = if body.read_only {
        Access::ReadOnly
    } else {
        Access::ReadWrite
    };
    #[cfg(unix)]
    let peer_pid = parts
        .extensions
        .get::<super::UdsPeer>()
        .and_then(|peer| peer.pid);
    #[cfg(not(unix))]
    let peer_pid: Option<i32> = {
        let _ = parts;
        None
    };
    let peer_parent = match peer_pid {
        Some(pid) => tokio::task::spawn_blocking(move || consent::peer_parent_process(pid))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let request = ConnectRequest {
        client,
        scope,
        project_name,
        access,
        peer_pid,
        peer_parent,
    };
    let pending = match state.grants.enrolment().request(request, now_timestamp()) {
        Ok(pending) => pending,
        Err(RequestError::TooManyPending) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": format!(
                        "{PENDING_CAP} connection requests are already waiting for an answer \
                         in Staged; answer or let them expire before asking again"
                    )
                })),
            )
                .into_response();
        }
    };
    log::info!(
        "[control_server] connection request {} from {:?} (pid {:?})",
        pending.id,
        pending.request.client,
        pending.request.peer_pid
    );
    consent::present_request(&state.app_handle, &pending);
    Json(serde_json::json!({
        "requestId": pending.id,
        "expiresAt": pending.expires_at,
    }))
    .into_response()
}

/// `GET /api/connect/{id}`: long-poll for the answer.
pub(super) async fn poll_connect(
    Path(request_id): Path<String>,
    State(state): State<ControlState>,
) -> Response {
    let started = std::time::Instant::now();
    loop {
        match state.grants.enrolment().poll(&request_id, now_timestamp()) {
            PollOutcome::Pending { expires_at } => {
                if started.elapsed() >= std::time::Duration::from_millis(LONG_POLL_MS) {
                    return (
                        StatusCode::ACCEPTED,
                        Json(serde_json::json!({ "status": "pending", "expiresAt": expires_at })),
                    )
                        .into_response();
                }
                tokio::time::sleep(std::time::Duration::from_millis(LONG_POLL_STEP_MS)).await;
            }
            PollOutcome::Approved(grant) => {
                return Json(approved_payload(&state, &grant).await).into_response();
            }
            PollOutcome::Denied => return refusal(AuthorizationFailure::Denied),
            PollOutcome::Expired => {
                return authorization_response(
                    StatusCode::UNAUTHORIZED,
                    AuthorizationFailure::Missing.code(),
                    "the connection request expired before it was answered; run \
                     `stagedctl connect` again",
                );
            }
            PollOutcome::Unknown => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "unknown connection request (already collected, or from \
                                  another instance)"
                    })),
                )
                    .into_response();
            }
        }
    }
}

/// What an approved or reconnected client receives.
async fn approved_payload(state: &ControlState, grant: &ApprovedGrant) -> serde_json::Value {
    let grant_id = grant.grant_id.clone();
    let record = with_store(state, move |store| {
        store
            .get_control_grant(&grant_id)
            .map_err(|e| e.to_string())
    })
    .await
    .ok()
    .flatten();
    let mut payload = serde_json::json!({
        "status": "approved",
        "grantId": grant.grant_id,
        "token": grant.token,
        "expiresAt": grant.expires_at,
    });
    payload["reconnectSecret"] = serde_json::Value::String(grant.reconnect_secret.clone());
    if let Some(record) = record {
        payload["client"] = serde_json::Value::String(record.client);
        payload["scope"] = serde_json::json!({ "projectId": record.scope_project_id });
        payload["access"] = serde_json::Value::String(record.access);
    }
    payload
}

/// The durable row `grant_id` names, if it may still be used at `now`.
fn usable_record(
    record: Option<ControlGrantRecord>,
    now: i64,
) -> Result<ControlGrantRecord, AuthorizationFailure> {
    let record = record.ok_or(AuthorizationFailure::Unknown)?;
    if record.is_revoked() {
        return Err(AuthorizationFailure::Revoked);
    }
    if record.is_expired_at(now) {
        return Err(AuthorizationFailure::Expired);
    }
    Ok(record)
}

/// The `Err` is boxed only to keep the `Result` small (clippy's
/// `result_large_err`); every caller returns it straight away.
async fn load_record(
    state: &ControlState,
    grant_id: &str,
) -> Result<Option<ControlGrantRecord>, Box<Response>> {
    let grant_id = grant_id.to_string();
    with_store(state, move |store| {
        store
            .get_control_grant(&grant_id)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| {
        Box::new(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response(),
        )
    })
}

/// `POST /api/reconnect/challenge`: a nonce for `grantId`, if the grant may
/// still reconnect.
pub(super) async fn reconnect_challenge(
    State(state): State<ControlState>,
    Json(body): Json<ChallengeBody>,
) -> Response {
    let now = now_timestamp();
    let record = match load_record(&state, &body.grant_id).await {
        Ok(record) => record,
        Err(response) => return *response,
    };
    if let Err(failure) = usable_record(record, now) {
        return refusal(failure);
    }
    let nonce = state.grants.enrolment().issue_nonce(&body.grant_id, now);
    Json(serde_json::json!({ "nonce": nonce, "expiresAt": now + NONCE_TTL_MS })).into_response()
}

/// Verify a proof body against its grant's row: the nonce is consumed
/// whether or not the proof matches.
async fn verify_proof(
    state: &ControlState,
    body: &ProofBody,
    allow_expired: bool,
) -> Result<ControlGrantRecord, Box<Response>> {
    let now = now_timestamp();
    if !state
        .grants
        .enrolment()
        .take_nonce(&body.nonce, &body.grant_id, now)
    {
        return Err(Box::new(authorization_response(
            StatusCode::UNAUTHORIZED,
            AuthorizationFailure::Missing.code(),
            "the reconnect nonce is unknown, used or expired; request a new challenge",
        )));
    }
    let record = load_record(state, &body.grant_id).await?;
    let record = match usable_record(record, now) {
        Ok(record) => record,
        Err(AuthorizationFailure::Expired) if allow_expired => {
            // Fall through with the row: a disconnect of an expired grant is
            // still the client's to make.
            load_record(state, &body.grant_id)
                .await?
                .ok_or_else(|| Box::new(refusal(AuthorizationFailure::Unknown)))?
        }
        Err(failure) => return Err(Box::new(refusal(failure))),
    };
    if !proof_matches(
        &record.reconnect_secret_hash,
        &body.grant_id,
        &body.nonce,
        &body.proof,
    ) {
        return Err(Box::new(authorization_response(
            StatusCode::UNAUTHORIZED,
            AuthorizationFailure::Unknown.code(),
            "the reconnect proof did not verify; the local grant file does not match this \
             grant — run `stagedctl disconnect` then `stagedctl connect`",
        )));
    }
    Ok(record)
}

/// `POST /api/reconnect`: prove possession of the secret, renew the grant,
/// get this run's bearer.
pub(super) async fn reconnect(
    State(state): State<ControlState>,
    Json(body): Json<ProofBody>,
) -> Response {
    let record = match verify_proof(&state, &body, false).await {
        Ok(record) => record,
        Err(response) => return *response,
    };
    let now = now_timestamp();
    let grant_id = record.id.clone();
    let renewed = with_store(&state, move |store| {
        store
            .renew_control_grant(&grant_id, now)
            .map_err(|e| e.to_string())
    })
    .await;
    let expires_at = match renewed {
        Ok(Some(expires_at)) => expires_at,
        Ok(None) => return refusal(AuthorizationFailure::Revoked),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response()
        }
    };
    let Some(access) = Access::from_wire(&record.access) else {
        return bad_request(format!("grant has unknown access {:?}", record.access));
    };
    let token = state.grants.mint_app_bearer(
        &record.id,
        &record.client,
        Scope::from_project_id(record.scope_project_id.clone()),
        access,
    );
    state.grants.enrolment().touch_due(&record.id, now);
    consent::emit_grants_changed(&state.app_handle);
    Json(serde_json::json!({
        "grantId": record.id,
        "token": token,
        "expiresAt": expires_at,
        "client": record.client,
        "scope": { "projectId": record.scope_project_id },
        "access": record.access,
    }))
    .into_response()
}

/// `POST /api/disconnect`: prove possession, then revoke the grant and its
/// live bearers (`stagedctl disconnect`).
pub(super) async fn disconnect(
    State(state): State<ControlState>,
    Json(body): Json<ProofBody>,
) -> Response {
    let record = match verify_proof(&state, &body, true).await {
        Ok(record) => record,
        Err(response) => return *response,
    };
    match consent::revoke_grant(&state.app_handle, &state.grants, &record.id) {
        Ok(revoked) => Json(serde_json::json!({ "revoked": revoked })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// Record use of a class B bearer, at most once per [`TOUCH_INTERVAL_MS`].
pub(super) fn note_app_bearer_use(state: &ControlState, grant: &super::ControlGrant) {
    let super::Actor::App { grant_id, .. } = &grant.actor else {
        return;
    };
    let now = now_timestamp();
    if !state.grants.enrolment().touch_due(grant_id, now) {
        return;
    }
    let Ok(store) = consent::store_of(&state.app_handle) else {
        return;
    };
    let grant_id = grant_id.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = store.touch_control_grant(&grant_id, now) {
            log::warn!("[control_server] failed to record use of grant {grant_id}: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(client: &str) -> ConnectRequest {
        ConnectRequest {
            client: client.to_string(),
            scope: Scope::AllProjects,
            project_name: None,
            access: Access::ReadWrite,
            peer_pid: Some(4242),
            peer_parent: Some("zsh".to_string()),
        }
    }

    fn approved(grant_id: &str) -> ApprovedGrant {
        ApprovedGrant {
            grant_id: grant_id.to_string(),
            token: "t".repeat(64),
            reconnect_secret: "s".repeat(64),
            expires_at: 99,
        }
    }

    #[test]
    fn requests_are_parked_listed_oldest_first_and_capped() {
        let enrol = Enrolment::default();
        let mut ids = Vec::new();
        for i in 0..PENDING_CAP {
            let pending = enrol
                .request(request(&format!("c{i}")), 1_000 + i as i64)
                .unwrap();
            ids.push(pending.id);
        }
        assert_eq!(
            enrol.request(request("one-too-many"), 2_000).unwrap_err(),
            RequestError::TooManyPending,
            "the {}th unanswered request is refused",
            PENDING_CAP + 1
        );
        let listed: Vec<String> = enrol.pending(2_000).into_iter().map(|p| p.id).collect();
        assert_eq!(listed, ids, "oldest first");

        // Answering one frees a slot; the answered request no longer lists.
        enrol.answer(&ids[0], Decision::Denied, 2_000).unwrap();
        assert_eq!(enrol.pending(2_000).len(), PENDING_CAP - 1);
        assert!(enrol.request(request("fits-now"), 2_000).is_ok());
    }

    #[test]
    fn approval_is_collected_once_and_denial_reads_denied() {
        let enrol = Enrolment::default();
        let pending = enrol.request(request("agent"), 1_000).unwrap();
        assert_eq!(
            enrol.poll(&pending.id, 1_001),
            PollOutcome::Pending {
                expires_at: 1_000 + PENDING_TTL_MS
            }
        );
        let answered = enrol
            .answer(&pending.id, Decision::Approved(approved("g1")), 1_002)
            .unwrap();
        assert_eq!(answered.decision, Some(Decision::Approved(approved("g1"))));
        assert_eq!(
            enrol
                .answer(&pending.id, Decision::Denied, 1_003)
                .unwrap_err(),
            AnswerError::AlreadyAnswered
        );
        assert_eq!(
            enrol.poll(&pending.id, 1_004),
            PollOutcome::Approved(approved("g1"))
        );
        assert_eq!(
            enrol.poll(&pending.id, 1_005),
            PollOutcome::Unknown,
            "credentials are handed over exactly once"
        );

        let denied = enrol.request(request("other"), 1_000).unwrap();
        enrol.answer(&denied.id, Decision::Denied, 1_001).unwrap();
        assert_eq!(enrol.poll(&denied.id, 1_002), PollOutcome::Denied);
        assert_eq!(enrol.poll("never-issued", 1_002), PollOutcome::Unknown);
    }

    #[test]
    fn unanswered_requests_expire_for_both_sides() {
        let enrol = Enrolment::default();
        let pending = enrol.request(request("slow"), 1_000).unwrap();
        let after = 1_000 + PENDING_TTL_MS;
        assert_eq!(enrol.poll(&pending.id, after), PollOutcome::Expired);
        assert_eq!(
            enrol.poll(&pending.id, after + 1),
            PollOutcome::Unknown,
            "forgotten once reported"
        );
        let late = enrol.request(request("late-dialog"), 1_000).unwrap();
        assert_eq!(
            enrol
                .answer(&late.id, Decision::Denied, 1_000 + PENDING_TTL_MS)
                .unwrap_err(),
            AnswerError::Expired,
            "an answer after expiry is refused, not recorded"
        );
        assert_eq!(
            enrol.get_unanswered(&late.id, after + 1).unwrap_err(),
            AnswerError::Unknown
        );
        // An approval the CLI never collected does not outlive the request.
        let abandoned = enrol.request(request("abandoned"), 1_000).unwrap();
        enrol
            .answer(&abandoned.id, Decision::Approved(approved("g2")), 1_001)
            .unwrap();
        assert_eq!(enrol.poll(&abandoned.id, after), PollOutcome::Expired);
    }

    #[test]
    fn nonces_are_single_use_grant_bound_and_expiring() {
        let enrol = Enrolment::default();
        let nonce = enrol.issue_nonce("g1", 1_000);
        assert_eq!(nonce.len(), 64);
        assert!(!enrol.take_nonce(&nonce, "g2", 1_001), "bound to its grant");
        assert!(enrol.take_nonce(&nonce, "g1", 1_001));
        assert!(!enrol.take_nonce(&nonce, "g1", 1_001), "single use");
        let stale = enrol.issue_nonce("g1", 1_000);
        assert!(!enrol.take_nonce(&stale, "g1", 1_000 + NONCE_TTL_MS));
        // Bounded: the oldest is forgotten past the cap.
        let first = enrol.issue_nonce("g1", 5_000);
        for _ in 0..NONCE_CAP {
            enrol.issue_nonce("g1", 5_000);
        }
        assert!(!enrol.take_nonce(&first, "g1", 5_001));
    }

    #[test]
    fn touches_are_throttled_per_grant() {
        let enrol = Enrolment::default();
        assert!(enrol.touch_due("g1", 1_000));
        assert!(!enrol.touch_due("g1", 1_000 + TOUCH_INTERVAL_MS - 1));
        assert!(enrol.touch_due("g2", 1_000), "independent per grant");
        assert!(enrol.touch_due("g1", 1_000 + TOUCH_INTERVAL_MS));
        enrol.forget_touch("g1");
        assert!(enrol.touch_due("g1", 1_000 + TOUCH_INTERVAL_MS + 1));
    }

    /// RFC 4231 test cases 1 and 2.
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex::encode(mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block is hashed first.
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            hex::encode(mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn proofs_verify_against_the_stored_hash_only() {
        let secret = "ab".repeat(32);
        let hash = secret_hash(&secret);
        assert_eq!(hash.len(), 64);
        let proof = expected_proof(&hash, "g1", "n1").unwrap();
        assert!(proof_matches(&hash, "g1", "n1", &proof));
        assert!(!proof_matches(&hash, "g1", "n2", &proof), "nonce-bound");
        assert!(!proof_matches(&hash, "g2", "n1", &proof), "grant-bound");
        assert!(!proof_matches(&secret_hash("other"), "g1", "n1", &proof));
        assert!(!proof_matches("not-hex", "g1", "n1", &proof));
        assert!(!proof_matches(&hash, "g1", "n1", ""));
    }

    #[test]
    fn client_names_are_trimmed_bounded_and_printable() {
        assert_eq!(validate_client_name("  my agent ").unwrap(), "my agent");
        assert!(validate_client_name("   ").is_err());
        assert!(validate_client_name(&"x".repeat(MAX_CLIENT_NAME_CHARS + 1)).is_err());
        assert!(validate_client_name("tab\there").is_err());
        assert!(validate_client_name(&"x".repeat(MAX_CLIENT_NAME_CHARS)).is_ok());
    }

    #[test]
    fn projects_resolve_by_id_then_unique_case_insensitive_name() {
        let store = Store::in_memory().unwrap();
        let widgets = crate::store::Project::named("Widgets");
        let gadgets_a = crate::store::Project::named("Gadgets");
        let gadgets_b = crate::store::Project::named("gadgets");
        for project in [&widgets, &gadgets_a, &gadgets_b] {
            store.create_project(project).unwrap();
        }
        assert_eq!(
            resolve_project(&store, &widgets.id).unwrap(),
            (widgets.id.clone(), "Widgets".to_string())
        );
        assert_eq!(
            resolve_project(&store, " widgets ").unwrap().0,
            widgets.id,
            "names match case-insensitively and trimmed"
        );
        assert!(resolve_project(&store, "nothing")
            .unwrap_err()
            .contains("no project"));
        assert!(
            resolve_project(&store, "GADGETS")
                .unwrap_err()
                .contains("2 projects"),
            "an ambiguous name asks for the id"
        );
        assert_eq!(
            resolve_project(&store, &gadgets_b.id).unwrap().1,
            "gadgets",
            "an id always wins"
        );
    }

    #[test]
    fn usable_record_codes_each_refusal() {
        let record = ControlGrantRecord {
            id: "g".to_string(),
            client: "c".to_string(),
            scope_project_id: None,
            access: "read".to_string(),
            reconnect_secret_hash: "h".to_string(),
            created_at: 0,
            expires_at: 100,
            last_used_at: None,
            revoked_at: None,
        };
        assert_eq!(
            usable_record(None, 1).unwrap_err(),
            AuthorizationFailure::Unknown
        );
        assert_eq!(
            usable_record(Some(record.clone()), 100).unwrap_err(),
            AuthorizationFailure::Expired,
            "expiry is inclusive at the instant"
        );
        assert!(usable_record(Some(record.clone()), 99).is_ok());
        let revoked = ControlGrantRecord {
            revoked_at: Some(5),
            ..record
        };
        assert_eq!(
            usable_record(Some(revoked), 1).unwrap_err(),
            AuthorizationFailure::Revoked,
            "revoked beats expired"
        );
    }
}
