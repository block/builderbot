//! Control credentials: who may call the control server, and with what
//! reach.
//!
//! A [`ControlGrant`] is the authorization a bearer token stands for —
//! the actor it names, the [`Scope`] of projects it may touch and whether
//! it may write. The [`ControlGrants`] registry holds the live grants keyed
//! by the SHA-256 of their token (the token itself is never stored), hands
//! them to the server's bearer middleware, and forgets them on revoke.
//!
//! Today the only minter is a local session start (class A of the two-caller
//! model): `session_runner` mints a read-write grant scoped to the session's
//! own project, writes it to an owner-only per-session file whose path rides
//! into the agent as `STAGEDCTL_LOCK`, and holds a [`SessionLease`] for the
//! run's life. The lease's `Drop` revokes the grant by token — never by
//! session id, since a resume or queued follow-up can register a successor
//! under the same id before this run's thread exits — and unlinks its file,
//! which carries the registration epoch in its name so two runs of one
//! session id never share a path.
//!
//! The other minter is consent (class B, see `enrol`): an outside client the
//! user approved by name gets a live bearer carrying [`Actor::App`], bounded
//! per grant by [`MAX_LIVE_BEARERS_PER_GRANT`] and lasting until the app
//! exits, the grant is revoked (which drops every bearer of that grant), or
//! the durable row's expiry passes — each bearer carries that expiry, so a
//! long-running app refuses it as `authorization_expired` the moment the
//! row would.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use acp_client::AgentProcessEvent;
use sha2::{Digest, Sha256};

use super::enrol::Enrolment;
use super::files::{self, ControlTransport};

/// Who a grant was issued to. Recorded so mutations can name their actor.
/// The authorization decision is the scope and access alone, with one
/// exception: `open_project` moves the user's window, so the scope table
/// refuses it to a [`Session`](Self::Session) whatever its scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Actor {
    /// A session this instance spawned. `epoch` is the registry
    /// registration the grant belongs to, so two runs of the same session
    /// id are told apart.
    Session { session_id: String, epoch: u64 },
    /// An outside client the user approved by name (class B). `grant_id`
    /// is the durable `control_grants` row id, `client` the name the user
    /// approved.
    App { grant_id: String, client: String },
}

impl Actor {
    /// The value recorded in `sessions.started_by` for a session this actor
    /// starts: `session:<session-id>` or `app:<grant-id>`. Stable identifiers
    /// only — never the client's display name, which the grant row keeps.
    pub fn started_by(&self) -> String {
        match self {
            Self::Session { session_id, .. } => format!("session:{session_id}"),
            Self::App { grant_id, .. } => format!("app:{grant_id}"),
        }
    }
}

/// A running session this instance spawned, as the session runner
/// registered its agent process: what a connecting process's ancestry
/// resolves to (see `ancestry`). The registration lives exactly as long as
/// the session's [`SessionLease`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnedSession {
    pub session_id: String,
    pub epoch: u64,
    pub project_id: String,
}

/// Which projects a grant reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Exactly one project: commands addressing another project's ids are
    /// refused ahead of the dispatch arm, and listing commands return only
    /// this project's rows.
    Project(String),
    /// Every project (an approved class B grant without `--project`).
    AllProjects,
}

impl Scope {
    /// The confined project's id, or `None` for every project.
    pub fn project_id(&self) -> Option<&str> {
        match self {
            Self::Project(project_id) => Some(project_id),
            Self::AllProjects => None,
        }
    }

    /// From a `control_grants.scope_project_id` column.
    pub fn from_project_id(project_id: Option<String>) -> Self {
        match project_id {
            Some(project_id) => Self::Project(project_id),
            None => Self::AllProjects,
        }
    }
}

/// Whether a grant may run the commands that change the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    ReadOnly,
    ReadWrite,
}

impl Access {
    /// The `control_grants.access` column and wire spelling.
    pub fn wire(self) -> &'static str {
        match self {
            Self::ReadOnly => "read",
            Self::ReadWrite => "write",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::ReadOnly),
            "write" => Some(Self::ReadWrite),
            _ => None,
        }
    }
}

/// What a bearer token authorizes. Carried on the request into
/// `web_server::dispatch` as `DispatchCaller::Control(grant)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ControlGrant {
    pub actor: Actor,
    pub scope: Scope,
    /// The branch the granted session runs on, when it has one. Informational
    /// for now: scope is per project, not per branch.
    pub branch_id: Option<String>,
    pub access: Access,
}

impl ControlGrant {
    /// The project a [`Scope::Project`] grant is confined to; `None` for
    /// [`Scope::AllProjects`].
    pub fn project_scope(&self) -> Option<&str> {
        match &self.scope {
            Scope::Project(project_id) => Some(project_id),
            Scope::AllProjects => None,
        }
    }

    /// An unconfined read-write grant, for tests that need a caller with
    /// no scope restriction.
    #[cfg(test)]
    pub(crate) fn test_all_projects() -> Self {
        Self {
            actor: Actor::App {
                grant_id: "test-grant".to_string(),
                client: "test".to_string(),
            },
            scope: Scope::AllProjects,
            branch_id: None,
            access: Access::ReadWrite,
        }
    }
}

/// Why a bearer did not resolve to a grant. Each maps to an error code the
/// CLI reports under exit code 4 (report, don't retry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthorizationFailure {
    /// No `Authorization: Bearer` on the request.
    Missing,
    /// A bearer this instance never minted (another instance's, an earlier
    /// start's, or made up).
    Unknown,
    /// A bearer this instance minted and has since revoked — the session it
    /// belonged to has ended, or the user revoked the connected app.
    Revoked,
    /// A durable (class B) grant whose thirty days ran out unused.
    Expired,
    /// The user answered the consent prompt with Deny.
    Denied,
    /// A reconnect proof that did not verify against the grant's stored
    /// hash: the client's grant file no longer matches the row (a database
    /// restored from elsewhere, or a file copied between machines).
    ProofMismatch,
}

impl AuthorizationFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Missing | Self::Unknown => "authorization_required",
            Self::Revoked => "authorization_revoked",
            Self::Expired => "authorization_expired",
            Self::Denied => "authorization_denied",
            Self::ProofMismatch => "authorization_proof_mismatch",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Missing => {
                "no credential: inside a Staged-spawned session the session's $STAGEDCTL_LOCK \
                 file carries one; from anywhere else run `stagedctl connect --client <name>` \
                 and approve it in the Staged window"
            }
            Self::Unknown => {
                "the credential is not one this Staged instance issued (it may belong to \
                 another instance, or to an earlier start of this one); from outside a \
                 session, `stagedctl connect` enrols a new one"
            }
            Self::Revoked => {
                "the credential has been revoked: the session it was issued to has ended, or \
                 the connected app was revoked in Settings"
            }
            Self::Expired => {
                "the connected app's approval expired (thirty days without use); run \
                 `stagedctl connect` to ask again"
            }
            Self::Denied => "the user denied the connection request in the Staged window",
            Self::ProofMismatch => {
                "the reconnect proof did not verify: the local grant file does not match this \
                 grant — run `stagedctl disconnect` then `stagedctl connect` to enrol afresh"
            }
        }
    }
}

/// How many retired token hashes are remembered, with why they were
/// retired, so a late call from a finished session's lingering subprocess
/// reads as "revoked" (or "expired") rather than "unknown". Bounded: it is
/// a nicety for error text, not a security property (every outcome refuses).
const REVOKED_MEMORY: usize = 1024;

/// Most live bearers one class B grant may hold at once: each `stagedctl`
/// invocation from an approved client reconnects for a fresh one, so the
/// oldest is revoked when the next would exceed this.
pub(crate) const MAX_LIVE_BEARERS_PER_GRANT: usize = 8;

#[derive(Default)]
struct GrantsInner {
    /// Live grants by token hash.
    live: HashMap<String, ControlGrant>,
    /// Class B bearers' expiry (the durable row's `expires_at` at mint),
    /// by token hash. Session grants have none: the lease is their life.
    expiries: HashMap<String, i64>,
    /// Recently retired token hashes, oldest first, and why each went.
    retired: VecDeque<String>,
    retired_reason: HashMap<String, AuthorizationFailure>,
    /// Class B: grant id → its live token hashes, oldest first.
    app_bearers: HashMap<String, VecDeque<String>>,
    /// Agent process pid → the running session it belongs to, kept in step
    /// with the session leases: registered when the driver spawns the
    /// process, removed when the lease drops.
    agents: HashMap<u32, SpawnedSession>,
}

impl GrantsInner {
    fn revoke_hash(&mut self, hash: &str) -> bool {
        self.retire_hash(hash, AuthorizationFailure::Revoked)
    }

    /// Drop the live grant behind `hash`, remembering `reason` for later
    /// callers. `false` if nothing was live.
    fn retire_hash(&mut self, hash: &str, reason: AuthorizationFailure) -> bool {
        let Some(grant) = self.live.remove(hash) else {
            return false;
        };
        self.expiries.remove(hash);
        if let Actor::App { grant_id, .. } = &grant.actor {
            if let Some(hashes) = self.app_bearers.get_mut(grant_id) {
                hashes.retain(|h| h != hash);
                if hashes.is_empty() {
                    self.app_bearers.remove(grant_id);
                }
            }
        }
        if self
            .retired_reason
            .insert(hash.to_string(), reason)
            .is_none()
        {
            self.retired.push_back(hash.to_string());
            if self.retired.len() > REVOKED_MEMORY {
                if let Some(oldest) = self.retired.pop_front() {
                    self.retired_reason.remove(&oldest);
                }
            }
        }
        true
    }
}

/// The live control grants, shared between the control server (which
/// resolves bearers against it), session starts (which mint into it) and
/// the consent flow (which mints and revokes class B bearers). Managed as
/// Tauri state ahead of `control_server::start`; cloning shares the
/// registry. Carries the class B [`Enrolment`] state too.
#[derive(Clone, Default)]
pub struct ControlGrants {
    inner: Arc<Mutex<GrantsInner>>,
    pub(super) enrolment: Enrolment,
}

impl ControlGrants {
    /// Register `grant` under a fresh 256-bit token and return the token.
    /// Only the token's hash is kept.
    pub(crate) fn mint(&self, grant: ControlGrant) -> String {
        let token = crate::web_server::generate_token();
        let hash = token_hash(&token);
        let mut inner = self.inner.lock().unwrap();
        inner.live.insert(hash, grant);
        token
    }

    /// The grant `token` stands for, or why it does not.
    pub(crate) fn authorize(&self, token: &str) -> Result<ControlGrant, AuthorizationFailure> {
        self.authorize_at(token, crate::store::now_timestamp())
    }

    /// [`authorize`](Self::authorize) at a given instant. A class B bearer
    /// whose row expiry has passed is retired here and refused `Expired`,
    /// however long the app has been running.
    pub(crate) fn authorize_at(
        &self,
        token: &str,
        now: i64,
    ) -> Result<ControlGrant, AuthorizationFailure> {
        let hash = token_hash(token);
        let mut inner = self.inner.lock().unwrap();
        if let Some(grant) = inner.live.get(&hash) {
            if inner
                .expiries
                .get(&hash)
                .is_some_and(|expires| *expires <= now)
            {
                inner.retire_hash(&hash, AuthorizationFailure::Expired);
                return Err(AuthorizationFailure::Expired);
            }
            return Ok(grant.clone());
        }
        if let Some(reason) = inner.retired_reason.get(&hash) {
            return Err(*reason);
        }
        Err(AuthorizationFailure::Unknown)
    }

    /// Forget the grant behind `token`. Returns whether one was live. A
    /// second revoke of the same token is a no-op, so a lease dropping after
    /// an explicit revoke is harmless.
    pub(crate) fn revoke(&self, token: &str) -> bool {
        let hash = token_hash(token);
        self.inner.lock().unwrap().revoke_hash(&hash)
    }

    /// Mint a live bearer for an approved class B grant, good until
    /// `expires_at` (the durable row's expiry). When the grant already holds
    /// [`MAX_LIVE_BEARERS_PER_GRANT`], its oldest bearer is revoked first.
    pub(crate) fn mint_app_bearer(
        &self,
        grant_id: &str,
        client: &str,
        scope: Scope,
        access: Access,
        expires_at: i64,
    ) -> String {
        let token = crate::web_server::generate_token();
        let hash = token_hash(&token);
        let grant = ControlGrant {
            actor: Actor::App {
                grant_id: grant_id.to_string(),
                client: client.to_string(),
            },
            scope,
            branch_id: None,
            access,
        };
        let mut inner = self.inner.lock().unwrap();
        let evict: Vec<String> = {
            let hashes = inner.app_bearers.entry(grant_id.to_string()).or_default();
            let excess = (hashes.len() + 1).saturating_sub(MAX_LIVE_BEARERS_PER_GRANT);
            hashes.drain(..excess).collect()
        };
        for stale in evict {
            inner.revoke_hash(&stale);
        }
        inner.live.insert(hash.clone(), grant);
        inner.expiries.insert(hash.clone(), expires_at);
        inner
            .app_bearers
            .entry(grant_id.to_string())
            .or_default()
            .push_back(hash);
        token
    }

    /// Revoke every live bearer of class B grant `grant_id`. Returns how
    /// many were live. A later call with one of them reads `Revoked`.
    pub(crate) fn revoke_app_grant(&self, grant_id: &str) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let hashes: Vec<String> = inner
            .app_bearers
            .remove(grant_id)
            .map(Vec::from)
            .unwrap_or_default();
        let count = hashes.len();
        for hash in hashes {
            inner.revoke_hash(&hash);
        }
        count
    }

    /// Whether class B grant `grant_id` holds a live bearer right now.
    pub(crate) fn app_grant_is_live(&self, grant_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .app_bearers
            .get(grant_id)
            .is_some_and(|hashes| !hashes.is_empty())
    }

    /// Record that `pid` is an agent process of `session`. A pid already
    /// registered (reused by the kernel for a new agent) is re-pointed.
    pub(crate) fn register_agent_process(&self, pid: u32, session: SpawnedSession) {
        self.inner.lock().unwrap().agents.insert(pid, session);
    }

    /// Forget `pid` if it is still registered as an agent process of
    /// `session`'s run. A pid the kernel has since reused for another run's
    /// agent (re-pointed by `register_agent_process`) is left to that run.
    /// Returns whether it was forgotten.
    pub(crate) fn deregister_agent_process(&self, pid: u32, session: &SpawnedSession) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let ours = inner
            .agents
            .get(&pid)
            .is_some_and(|s| s.session_id == session.session_id && s.epoch == session.epoch);
        if ours {
            inner.agents.remove(&pid);
        }
        ours
    }

    /// Forget every agent process registered for the run `(session_id,
    /// epoch)`; a successor run of the same session id keeps its own.
    /// Returns how many were forgotten.
    pub(crate) fn deregister_session_processes(&self, session_id: &str, epoch: u64) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.agents.len();
        inner
            .agents
            .retain(|_, s| !(s.session_id == session_id && s.epoch == epoch));
        before - inner.agents.len()
    }

    /// The running session whose agent process is `pid`, if any.
    pub(crate) fn spawned_session_of(&self, pid: u32) -> Option<SpawnedSession> {
        self.inner.lock().unwrap().agents.get(&pid).cloned()
    }

    /// Number of live grants.
    #[cfg(test)]
    pub(crate) fn live_count(&self) -> usize {
        self.inner.lock().unwrap().live.len()
    }
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

// =============================================================================
// Class A: per-session leases
// =============================================================================

/// What a local session start asks a grant for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionGrantRequest {
    pub session_id: String,
    pub epoch: u64,
    pub project_id: String,
    pub branch_id: Option<String>,
}

/// Whether a session start gets a control credential: local sessions
/// (remote ones cannot reach the socket) that belong to a project (the
/// scope the grant is confined to) and are not project (MCP) sessions, whose
/// MCP tools stay their only control surface.
pub(crate) fn session_grant_applies(
    workspace_name: Option<&str>,
    project_id: Option<&str>,
    mcp_project_id: Option<&str>,
) -> bool {
    workspace_name.is_none() && project_id.is_some() && mcp_project_id.is_none()
}

/// A minted session grant plus the per-session file that delivers it.
/// Dropping the lease revokes the grant and unlinks the file; hold it for
/// the run's whole life, on the runner thread, so a panic revokes too. The
/// file name carries the registration epoch, so a successor run of the
/// same session id writes its own file and this unlink can never take the
/// successor's credential with it.
pub struct SessionLease {
    token: String,
    file: PathBuf,
    grants: ControlGrants,
    /// The run this lease belongs to; what its agent processes register as.
    session: SpawnedSession,
}

impl SessionLease {
    /// The per-session file, to inject as `STAGEDCTL_LOCK`.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// A callback for the agent driver's process hook: registers each agent
    /// process pid as this session's when it spawns, so a `stagedctl` run
    /// anywhere in that process tree is recognised as the session's whatever
    /// credential it presents (see `ancestry`), and forgets it when the
    /// process exits, so a pid the kernel reuses cannot read as this
    /// session's. Whatever is still registered goes with the lease.
    pub fn agent_process_observer(&self) -> acp_client::AgentProcessObserver {
        let grants = self.grants.clone();
        let session = self.session.clone();
        Arc::new(move |event| match event {
            AgentProcessEvent::Spawned(pid) => {
                log::info!(
                    "[control_server] session {} agent process {pid} registered",
                    session.session_id
                );
                grants.register_agent_process(pid, session.clone());
            }
            AgentProcessEvent::Exited(pid) => {
                if grants.deregister_agent_process(pid, &session) {
                    log::info!(
                        "[control_server] session {} agent process {pid} exited; forgotten",
                        session.session_id
                    );
                }
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

impl std::fmt::Debug for SessionLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token.
        f.debug_struct("SessionLease")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl Drop for SessionLease {
    fn drop(&mut self) {
        self.grants.revoke(&self.token);
        self.grants
            .deregister_session_processes(&self.session.session_id, self.session.epoch);
        if let Err(e) = std::fs::remove_file(&self.file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "[control_server] failed to remove session credential {:?}: {e}",
                    self.file
                );
            }
        }
    }
}

/// Mint a grant for `request`, write the per-session file into
/// `sessions_dir`, and return the lease. The file has the discovery file's
/// shape plus `token`, so the CLI reads it through the same hardened path.
/// A write failure revokes the grant again and returns the error: a session
/// is never granted a credential it cannot be handed.
pub(crate) fn mint_session_lease_at(
    grants: &ControlGrants,
    sessions_dir: &Path,
    transport: &ControlTransport,
    pid: u32,
    generation: u64,
    request: SessionGrantRequest,
) -> Result<SessionLease, String> {
    let file = session_file_path(sessions_dir, &request.session_id, request.epoch);
    let session = SpawnedSession {
        session_id: request.session_id.clone(),
        epoch: request.epoch,
        project_id: request.project_id.clone(),
    };
    let grant = ControlGrant {
        actor: Actor::Session {
            session_id: request.session_id,
            epoch: request.epoch,
        },
        scope: Scope::Project(request.project_id),
        branch_id: request.branch_id,
        access: Access::ReadWrite,
    };
    let token = grants.mint(grant);
    let mut payload = files::discovery_payload(transport, pid, generation);
    payload["token"] = serde_json::Value::String(token.clone());
    let contents = serde_json::to_vec_pretty(&payload)
        .map_err(|e| format!("serialize session credential: {e}"))?;
    if let Err(e) = files::write_private_file(&file, &contents) {
        grants.revoke(&token);
        return Err(e);
    }
    Ok(SessionLease {
        token,
        file,
        grants: grants.clone(),
        session,
    })
}

/// `~/.staged/control/<pid>/sessions/<session_id>-<epoch>.json`. Session
/// ids are UUIDs the app generates and the epoch is a counter, so the name
/// needs no escaping; the epoch keeps two runs of one session id — a
/// predecessor dropping while its successor mints — from sharing a path.
/// The directory is swept whole with the instance, not by name.
fn session_file_path(sessions_dir: &Path, session_id: &str, epoch: u64) -> PathBuf {
    sessions_dir.join(format!("{session_id}-{epoch}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(session_id: &str, epoch: u64) -> SessionGrantRequest {
        SessionGrantRequest {
            session_id: session_id.to_string(),
            epoch,
            project_id: "project-a".to_string(),
            branch_id: Some("branch-a".to_string()),
        }
    }

    fn unix_transport(dir: &Path) -> ControlTransport {
        ControlTransport::Unix {
            socket: dir.join("sock"),
        }
    }

    fn sessions_dir() -> (tempfile::TempDir, PathBuf) {
        let root = files::tests::private_tempdir();
        let instance = files::instance_dir_path(root.path(), 4242);
        files::create_fresh_instance_dir(&instance).unwrap();
        let sessions = files::sessions_dir_path(&instance);
        (root, sessions)
    }

    // -------------------------------------------------------------------
    // Registry
    // -------------------------------------------------------------------

    #[test]
    fn minted_tokens_authorize_their_grant_and_nothing_else() {
        let grants = ControlGrants::default();
        let token = grants.mint(ControlGrant::test_all_projects());
        assert_eq!(token.len(), 64, "a 256-bit hex token");
        assert_eq!(
            grants.authorize(&token).unwrap(),
            ControlGrant::test_all_projects()
        );
        assert_eq!(
            grants.authorize(&"0".repeat(64)).unwrap_err(),
            AuthorizationFailure::Unknown
        );
        assert_eq!(
            grants.authorize("").unwrap_err(),
            AuthorizationFailure::Unknown
        );
        // Two mints of identical grants are distinct credentials.
        let other = grants.mint(ControlGrant::test_all_projects());
        assert_ne!(token, other);
        assert_eq!(grants.live_count(), 2);
    }

    #[test]
    fn revoked_tokens_are_refused_as_revoked_and_revoke_is_idempotent() {
        let grants = ControlGrants::default();
        let token = grants.mint(ControlGrant::test_all_projects());
        assert!(grants.revoke(&token));
        assert_eq!(
            grants.authorize(&token).unwrap_err(),
            AuthorizationFailure::Revoked
        );
        assert!(!grants.revoke(&token), "second revoke is a no-op");
        assert!(!grants.revoke("never-minted"));
        assert_eq!(grants.live_count(), 0);
    }

    #[test]
    fn revoking_by_token_leaves_a_successor_under_the_same_session_id() {
        // A resume or queued follow-up can register a successor under the
        // same session id between this run's terminal write and its thread
        // exit; the revoke must not take the successor's credential with it.
        let grants = ControlGrants::default();
        let first = grants.mint(ControlGrant {
            actor: Actor::Session {
                session_id: "s1".to_string(),
                epoch: 1,
            },
            scope: Scope::Project("p".to_string()),
            branch_id: None,
            access: Access::ReadWrite,
        });
        let second = grants.mint(ControlGrant {
            actor: Actor::Session {
                session_id: "s1".to_string(),
                epoch: 2,
            },
            scope: Scope::Project("p".to_string()),
            branch_id: None,
            access: Access::ReadWrite,
        });
        assert!(grants.revoke(&first));
        let live = grants.authorize(&second).unwrap();
        assert_eq!(
            live.actor,
            Actor::Session {
                session_id: "s1".to_string(),
                epoch: 2
            }
        );
    }

    #[test]
    fn revoked_memory_is_bounded() {
        let grants = ControlGrants::default();
        let first = grants.mint(ControlGrant::test_all_projects());
        grants.revoke(&first);
        for _ in 0..REVOKED_MEMORY {
            let token = grants.mint(ControlGrant::test_all_projects());
            grants.revoke(&token);
        }
        assert_eq!(
            grants.authorize(&first).unwrap_err(),
            AuthorizationFailure::Unknown,
            "the oldest revoked hash is forgotten once the memory is full"
        );
        let inner = grants.inner.lock().unwrap();
        assert_eq!(inner.retired.len(), REVOKED_MEMORY);
        assert_eq!(inner.retired_reason.len(), REVOKED_MEMORY);
    }

    #[test]
    fn actors_record_stable_started_by_identifiers() {
        let session = Actor::Session {
            session_id: "s1".to_string(),
            epoch: 9,
        };
        assert_eq!(session.started_by(), "session:s1");
        let app = Actor::App {
            grant_id: "g1".to_string(),
            client: "My Agent".to_string(),
        };
        assert_eq!(
            app.started_by(),
            "app:g1",
            "the grant id, not the display name"
        );
    }

    #[test]
    fn authorization_failures_map_to_the_cli_codes() {
        assert_eq!(
            AuthorizationFailure::Missing.code(),
            "authorization_required"
        );
        assert_eq!(
            AuthorizationFailure::Unknown.code(),
            "authorization_required"
        );
        assert_eq!(
            AuthorizationFailure::Revoked.code(),
            "authorization_revoked"
        );
        assert_eq!(
            AuthorizationFailure::Expired.code(),
            "authorization_expired"
        );
        assert_eq!(AuthorizationFailure::Denied.code(), "authorization_denied");
        assert_eq!(
            AuthorizationFailure::ProofMismatch.code(),
            "authorization_proof_mismatch"
        );
        // Every code is one the CLI maps to exit 4 (pinned there too).
        let cli = include_str!("../../crates/stagedctl/src/client.rs");
        for failure in [
            AuthorizationFailure::Missing,
            AuthorizationFailure::Revoked,
            AuthorizationFailure::Expired,
            AuthorizationFailure::Denied,
            AuthorizationFailure::ProofMismatch,
        ] {
            assert!(
                cli.contains(&format!("\"{}\",", failure.code())),
                "stagedctl's AUTHORIZATION_CODES must list {}",
                failure.code()
            );
        }
    }

    #[test]
    fn scope_and_access_round_trip_their_column_spellings() {
        assert_eq!(Scope::from_project_id(None), Scope::AllProjects);
        assert_eq!(
            Scope::from_project_id(Some("p".to_string())),
            Scope::Project("p".to_string())
        );
        assert_eq!(Scope::Project("p".to_string()).project_id(), Some("p"));
        assert_eq!(Scope::AllProjects.project_id(), None);
        for access in [Access::ReadOnly, Access::ReadWrite] {
            assert_eq!(Access::from_wire(access.wire()), Some(access));
        }
        assert_eq!(Access::from_wire("admin"), None);
    }

    // -------------------------------------------------------------------
    // Class B bearers
    // -------------------------------------------------------------------

    /// An expiry no test clock reaches.
    const FAR: i64 = i64::MAX;

    #[test]
    fn an_app_bearer_past_its_row_expiry_is_refused_as_expired() {
        // The row's thirty days can run out while the app keeps running;
        // the bearer minted from it must not outlive the row.
        let grants = ControlGrants::default();
        let token =
            grants.mint_app_bearer("g1", "agent", Scope::AllProjects, Access::ReadWrite, 10_000);
        assert!(grants.authorize_at(&token, 9_999).is_ok());
        assert!(grants.app_grant_is_live("g1"));
        assert_eq!(
            grants.authorize_at(&token, 10_000).unwrap_err(),
            AuthorizationFailure::Expired,
            "expiry is inclusive at the instant, like the row's"
        );
        assert_eq!(
            grants.authorize_at(&token, 10_001).unwrap_err(),
            AuthorizationFailure::Expired,
            "still expired, not revoked or unknown, on the next call"
        );
        assert!(
            !grants.app_grant_is_live("g1"),
            "retired from the grant's bearers"
        );
        assert_eq!(grants.live_count(), 0);
        // A session grant has no expiry of its own.
        let session = grants.mint(ControlGrant::test_all_projects());
        assert!(grants.authorize_at(&session, i64::MAX).is_ok());
    }

    #[test]
    fn app_bearers_carry_the_grant_and_revoke_together() {
        let grants = ControlGrants::default();
        let first =
            grants.mint_app_bearer("g1", "agent", Scope::AllProjects, Access::ReadOnly, FAR);
        let second = grants.mint_app_bearer(
            "g1",
            "agent",
            Scope::Project("p".to_string()),
            Access::ReadWrite,
            FAR,
        );
        let other =
            grants.mint_app_bearer("g2", "other", Scope::AllProjects, Access::ReadWrite, FAR);
        assert_eq!(
            grants.authorize(&first).unwrap(),
            ControlGrant {
                actor: Actor::App {
                    grant_id: "g1".to_string(),
                    client: "agent".to_string()
                },
                scope: Scope::AllProjects,
                branch_id: None,
                access: Access::ReadOnly,
            }
        );
        assert!(grants.app_grant_is_live("g1"));
        assert_eq!(grants.revoke_app_grant("g1"), 2);
        assert!(!grants.app_grant_is_live("g1"));
        assert_eq!(
            grants.authorize(&first).unwrap_err(),
            AuthorizationFailure::Revoked
        );
        assert_eq!(
            grants.authorize(&second).unwrap_err(),
            AuthorizationFailure::Revoked
        );
        assert!(grants.authorize(&other).is_ok(), "other grants untouched");
        assert_eq!(grants.revoke_app_grant("g1"), 0, "idempotent");
        assert_eq!(grants.revoke_app_grant("never"), 0);
        assert_eq!(grants.live_count(), 1);
    }

    #[test]
    fn app_bearers_per_grant_are_bounded_oldest_out() {
        let grants = ControlGrants::default();
        let first =
            grants.mint_app_bearer("g1", "agent", Scope::AllProjects, Access::ReadWrite, FAR);
        let mut newest = String::new();
        for _ in 0..MAX_LIVE_BEARERS_PER_GRANT {
            newest =
                grants.mint_app_bearer("g1", "agent", Scope::AllProjects, Access::ReadWrite, FAR);
        }
        assert_eq!(
            grants.authorize(&first).unwrap_err(),
            AuthorizationFailure::Revoked,
            "the oldest bearer is revoked once the grant exceeds its cap"
        );
        assert!(grants.authorize(&newest).is_ok());
        assert_eq!(grants.live_count(), MAX_LIVE_BEARERS_PER_GRANT);
        // Revoking one by token keeps the bookkeeping straight.
        assert!(grants.revoke(&newest));
        assert_eq!(
            grants.revoke_app_grant("g1"),
            MAX_LIVE_BEARERS_PER_GRANT - 1
        );
        assert_eq!(grants.live_count(), 0);
    }

    // -------------------------------------------------------------------
    // Mint gate
    // -------------------------------------------------------------------

    #[test]
    fn grants_apply_to_local_project_bound_non_mcp_sessions_only() {
        assert!(session_grant_applies(None, Some("p"), None));
        assert!(
            !session_grant_applies(Some("ws"), Some("p"), None),
            "remote sessions cannot reach the socket"
        );
        assert!(
            !session_grant_applies(None, None, None),
            "no project means no scope to confine the grant to"
        );
        assert!(
            !session_grant_applies(None, Some("p"), Some("p")),
            "project (MCP) sessions keep their MCP tools as the only control surface"
        );
    }

    // -------------------------------------------------------------------
    // Leases
    // -------------------------------------------------------------------

    #[test]
    fn lease_writes_an_owner_only_credential_file_with_the_endpoint_and_token() {
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = unix_transport(sessions.parent().unwrap());

        let lease =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 3, request("s1", 7))
                .unwrap();

        assert_eq!(lease.file(), sessions.join("s1-7.json"));
        let contents: serde_json::Value =
            serde_json::from_slice(&std::fs::read(lease.file()).unwrap()).unwrap();
        assert_eq!(contents["socket"], transport.describe());
        assert_eq!(contents["pid"], 4242);
        assert_eq!(contents["generation"], 3);
        assert_eq!(contents["protocolVersion"], super::super::PROTOCOL_VERSION);
        assert_eq!(contents["token"], lease.token());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(lease.file())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let grant = grants.authorize(lease.token()).unwrap();
        assert_eq!(
            grant,
            ControlGrant {
                actor: Actor::Session {
                    session_id: "s1".to_string(),
                    epoch: 7
                },
                scope: Scope::Project("project-a".to_string()),
                branch_id: Some("branch-a".to_string()),
                access: Access::ReadWrite,
            }
        );
        assert!(
            !format!("{lease:?}").contains(lease.token()),
            "Debug must not print the token"
        );
    }

    #[test]
    fn tcp_fallback_lease_carries_the_port_and_the_session_token() {
        // Over the TCP fallback the CLI sends one bearer; the server accepts
        // a grant token as the transport gate, so the per-session file names
        // the session token, not the instance token.
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = ControlTransport::Tcp {
            port: 5555,
            token: "ab".repeat(32),
        };
        let lease =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 1))
                .unwrap();
        let contents: serde_json::Value =
            serde_json::from_slice(&std::fs::read(lease.file()).unwrap()).unwrap();
        assert_eq!(contents["port"], 5555);
        assert_eq!(contents["token"], lease.token());
        assert!(contents.get("socket").is_none());
    }

    #[test]
    fn dropping_the_lease_revokes_the_grant_and_removes_the_file() {
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = unix_transport(sessions.parent().unwrap());
        let lease =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 1))
                .unwrap();
        let token = lease.token().to_string();
        let file = lease.file().to_path_buf();

        drop(lease);

        assert_eq!(
            grants.authorize(&token).unwrap_err(),
            AuthorizationFailure::Revoked
        );
        assert!(
            !file.exists(),
            "the credential file is retired with the grant"
        );
        assert_eq!(grants.live_count(), 0);
    }

    #[test]
    fn a_lease_registers_its_agent_processes_and_forgets_them_on_drop() {
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = unix_transport(sessions.parent().unwrap());
        let lease =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 7))
                .unwrap();
        let observe = lease.agent_process_observer();
        observe(AgentProcessEvent::Spawned(500));
        // A pikchr re-connect spawns a second agent process under the same
        // lease; both belong to the run.
        observe(AgentProcessEvent::Spawned(600));
        let expected = SpawnedSession {
            session_id: "s1".to_string(),
            epoch: 7,
            project_id: "project-a".to_string(),
        };
        assert_eq!(grants.spawned_session_of(500), Some(expected.clone()));
        assert_eq!(grants.spawned_session_of(600), Some(expected));
        assert_eq!(grants.spawned_session_of(700), None);

        // A successor run of the same session id keeps its own processes
        // when the predecessor's lease drops.
        let successor =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 8))
                .unwrap();
        successor.agent_process_observer()(AgentProcessEvent::Spawned(700));
        drop(lease);
        assert_eq!(grants.spawned_session_of(500), None);
        assert_eq!(grants.spawned_session_of(600), None);
        assert_eq!(
            grants.spawned_session_of(700).map(|s| s.epoch),
            Some(8),
            "the successor's registration is untouched"
        );
        assert_eq!(grants.deregister_session_processes("s1", 8), 1);
        assert_eq!(grants.spawned_session_of(700), None);
    }

    /// An agent process that exits is forgotten at once, while its lease
    /// (and the session's other processes) live on; and an exit reported
    /// for a pid the kernel has since handed to a successor run's agent
    /// leaves the successor's registration alone.
    #[test]
    fn an_agent_exit_unregisters_its_pid_while_the_lease_still_lives() {
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = unix_transport(sessions.parent().unwrap());
        let lease =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 7))
                .unwrap();
        let observe = lease.agent_process_observer();
        observe(AgentProcessEvent::Spawned(500));
        observe(AgentProcessEvent::Spawned(600));
        observe(AgentProcessEvent::Exited(500));
        assert_eq!(
            grants.spawned_session_of(500),
            None,
            "the exited pid is gone"
        );
        assert_eq!(
            grants.spawned_session_of(600).map(|s| s.epoch),
            Some(7),
            "the session's other process stays"
        );
        assert!(
            grants.authorize(lease.token()).is_ok(),
            "the lease is untouched by a process exit"
        );

        // Pid reuse: the successor run's agent gets pid 500 before the
        // predecessor's exit for it is reported.
        let successor =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 8))
                .unwrap();
        successor.agent_process_observer()(AgentProcessEvent::Spawned(500));
        observe(AgentProcessEvent::Exited(500));
        assert_eq!(
            grants.spawned_session_of(500).map(|s| s.epoch),
            Some(8),
            "a reused pid belongs to the run that registered it last"
        );
        assert_eq!(grants.deregister_session_processes("s1", 8), 1);
    }

    #[test]
    fn dropping_the_lease_leaves_a_successors_file_in_place() {
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = unix_transport(sessions.parent().unwrap());
        let first =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 1))
                .unwrap();
        // The successor registers under the same session id with the next
        // epoch and gets its own file, so the predecessor's unconditional
        // unlink cannot race its rename (read-then-unlink was not atomic).
        let second =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 2))
                .unwrap();
        assert_ne!(first.file(), second.file());
        assert_eq!(first.file(), sessions.join("s1-1.json"));
        assert_eq!(second.file(), sessions.join("s1-2.json"));

        drop(first);

        assert!(!sessions.join("s1-1.json").exists());
        assert!(second.file().exists(), "the successor's file must survive");
        let contents: serde_json::Value =
            serde_json::from_slice(&std::fs::read(second.file()).unwrap()).unwrap();
        assert_eq!(contents["token"], second.token());
        assert!(grants.authorize(second.token()).is_ok());
        assert_eq!(grants.live_count(), 1);
    }

    #[test]
    fn a_panicking_run_still_revokes_its_lease() {
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = unix_transport(sessions.parent().unwrap());
        let lease =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 1))
                .unwrap();
        let token = lease.token().to_string();
        let file = lease.file().to_path_buf();

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _held = lease;
            panic!("the agent run blew up");
        }));
        assert!(outcome.is_err());

        assert_eq!(
            grants.authorize(&token).unwrap_err(),
            AuthorizationFailure::Revoked
        );
        assert!(!file.exists());
    }

    #[test]
    fn a_credential_that_cannot_be_written_is_not_granted() {
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = unix_transport(sessions.parent().unwrap());
        let missing = sessions.join("missing");
        let err = mint_session_lease_at(&grants, &missing, &transport, 4242, 1, request("s1", 1))
            .unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
        assert_eq!(grants.live_count(), 0, "the grant is revoked again");
    }
}
