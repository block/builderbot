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
//! under the same id before this run's thread exits — and removes the file
//! only if it still holds this run's token. [`Actor::Client`] and
//! [`Scope::AllProjects`] exist so the consent-based class B grants can
//! reuse the registry; nothing mints them yet.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::files::{self, ControlTransport};

/// Who a grant was issued to. Recorded so mutations can name their actor;
/// never consulted for the authorization decision, which is the scope and
/// access alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Actor {
    /// A session this instance spawned. `epoch` is the registry
    /// registration the grant belongs to, so two runs of the same session
    /// id are told apart.
    Session { session_id: String, epoch: u64 },
    /// An outside client the user approved by name (class B; not yet
    /// minted). `grant_id` is the durable grant's row id, `client` the name
    /// the user approved.
    #[allow(dead_code)]
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

/// Which projects a grant reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Exactly one project: commands addressing another project's ids are
    /// refused ahead of the dispatch arm, and listing commands return only
    /// this project's rows.
    Project(String),
    /// Every project (reserved for approved class B grants; not yet
    /// minted).
    #[allow(dead_code)]
    AllProjects,
}

/// Whether a grant may run the commands that change the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    ReadOnly,
    ReadWrite,
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
    /// belonged to has ended.
    Revoked,
}

impl AuthorizationFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Missing | Self::Unknown => "authorization_required",
            Self::Revoked => "authorization_revoked",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Missing => {
                "no credential: stagedctl can only drive Staged from inside a session the app \
                 spawned (the session's $STAGEDCTL_LOCK file carries its credential)"
            }
            Self::Unknown => {
                "the credential is not one this Staged instance issued (it may belong to \
                 another instance, or to an earlier start of this one)"
            }
            Self::Revoked => {
                "the credential has been revoked: the session it was issued to has ended"
            }
        }
    }
}

/// How many revoked token hashes are remembered so a late call from a
/// finished session's lingering subprocess reads as "revoked" rather than
/// "unknown". Bounded: it is a nicety for error text, not a security
/// property (both outcomes refuse).
const REVOKED_MEMORY: usize = 1024;

#[derive(Default)]
struct GrantsInner {
    /// Live grants by token hash.
    live: HashMap<String, ControlGrant>,
    /// Recently revoked token hashes, oldest first.
    revoked: VecDeque<String>,
    revoked_set: HashSet<String>,
}

/// The live control grants, shared between the control server (which
/// resolves bearers against it) and session starts (which mint into it).
/// Managed as Tauri state ahead of `control_server::start`; cloning shares
/// the registry.
#[derive(Clone, Default)]
pub struct ControlGrants {
    inner: Arc<Mutex<GrantsInner>>,
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
        let hash = token_hash(token);
        let inner = self.inner.lock().unwrap();
        if let Some(grant) = inner.live.get(&hash) {
            return Ok(grant.clone());
        }
        if inner.revoked_set.contains(&hash) {
            return Err(AuthorizationFailure::Revoked);
        }
        Err(AuthorizationFailure::Unknown)
    }

    /// Forget the grant behind `token`. Returns whether one was live. A
    /// second revoke of the same token is a no-op, so a lease dropping after
    /// an explicit revoke is harmless.
    pub(crate) fn revoke(&self, token: &str) -> bool {
        let hash = token_hash(token);
        let mut inner = self.inner.lock().unwrap();
        let was_live = inner.live.remove(&hash).is_some();
        if was_live && inner.revoked_set.insert(hash.clone()) {
            inner.revoked.push_back(hash);
            if inner.revoked.len() > REVOKED_MEMORY {
                if let Some(oldest) = inner.revoked.pop_front() {
                    inner.revoked_set.remove(&oldest);
                }
            }
        }
        was_live
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
/// Dropping the lease revokes the grant and retires the file; hold it for
/// the run's whole life, on the runner thread, so a panic revokes too.
pub struct SessionLease {
    token: String,
    file: PathBuf,
    grants: ControlGrants,
}

impl SessionLease {
    /// The per-session file, to inject as `STAGEDCTL_LOCK`.
    pub fn file(&self) -> &Path {
        &self.file
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
        remove_session_file_if_ours(&self.file, &self.token);
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
    let file = session_file_path(sessions_dir, &request.session_id);
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
    })
}

/// `~/.staged/control/<pid>/sessions/<session_id>.json`. Session ids are
/// UUIDs the app generates, so the name needs no escaping.
fn session_file_path(sessions_dir: &Path, session_id: &str) -> PathBuf {
    sessions_dir.join(format!("{session_id}.json"))
}

/// Remove `file` if it still carries `token`. A successor run of the same
/// session id overwrites the file atomically with its own token at its
/// mint, and that file is the successor's to retire.
fn remove_session_file_if_ours(file: &Path, token: &str) {
    let holds_ours = std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| value["token"].as_str() == Some(token));
    if !holds_ours {
        return;
    }
    if let Err(e) = std::fs::remove_file(file) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("[control_server] failed to remove session credential {file:?}: {e}");
        }
    }
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
        assert_eq!(inner.revoked.len(), REVOKED_MEMORY);
        assert_eq!(inner.revoked_set.len(), REVOKED_MEMORY);
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

        assert_eq!(lease.file(), sessions.join("s1.json"));
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
    fn dropping_the_lease_leaves_a_successors_file_in_place() {
        let (_root, sessions) = sessions_dir();
        let grants = ControlGrants::default();
        let transport = unix_transport(sessions.parent().unwrap());
        let first =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 1))
                .unwrap();
        // The successor registers under the same session id and overwrites
        // the file with its own token.
        let second =
            mint_session_lease_at(&grants, &sessions, &transport, 4242, 1, request("s1", 2))
                .unwrap();
        assert_eq!(first.file(), second.file());

        drop(first);

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
