//! Which running session a connecting process belongs to, by process
//! ancestry.
//!
//! The peer of a control-socket connection is the `stagedctl` process
//! itself. Its credential says who it claims to be; its ancestry says where
//! it actually runs. Every local agent Staged spawns registers its pid in
//! [`ControlGrants`] for the life of its session lease (see
//! `grants::SessionLease`), so walking from the peer up through its parents
//! and finding one of those pids identifies the session whose process tree
//! the caller is in — whatever credential it presented, or withheld.
//!
//! The walk is bounded ([`MAX_ANCESTRY_HOPS`]) and best effort: a parent
//! that cannot be read, a process that has re-parented to `launchd`/`init`
//! after a double fork, or a pid outside the table all read as "not a
//! session's", never as a refusal. The gate this feeds (`enrol`) therefore
//! fails open for unrelated user processes and closed only on a positive
//! match. Same-uid processes are not a hard security boundary — a
//! determined agent can escape its process tree — so this is a strong
//! default and an honest attribution, not a sandbox.

use std::sync::Arc;

use super::grants::SpawnedSession;
use super::ControlGrants;

/// How many parents the walk visits before giving up. A `stagedctl` run
/// from an agent's shell tool is typically two or three hops below the
/// agent process; sixteen leaves room for wrappers without ever walking a
/// whole system.
pub(crate) const MAX_ANCESTRY_HOPS: usize = 16;

/// The parent lookup the walk uses: the system's, or a fake table in tests.
type ParentOf = dyn Fn(i32) -> Option<i32> + Send + Sync;

/// A process-ancestry oracle. Cloning shares the lookup.
#[derive(Clone)]
pub(crate) struct Ancestry {
    parent_of: Arc<ParentOf>,
}

impl Ancestry {
    /// The kernel's process table.
    pub fn system() -> Self {
        Self {
            parent_of: Arc::new(system_parent_of),
        }
    }

    /// A fixed pid → parent table; anything absent has no readable parent.
    #[cfg(test)]
    pub fn fake(parents: std::collections::HashMap<i32, i32>) -> Self {
        Self {
            parent_of: Arc::new(move |pid| parents.get(&pid).copied()),
        }
    }

    /// The parent of `pid`, if it can be read.
    pub fn parent_of(&self, pid: i32) -> Option<i32> {
        (self.parent_of)(pid)
    }

    /// The running session whose agent process `pid` is, or descends from,
    /// within [`MAX_ANCESTRY_HOPS`]. `None` when no ancestor is a registered
    /// agent, or the walk cannot proceed.
    pub fn spawned_session_of(&self, pid: i32, grants: &ControlGrants) -> Option<SpawnedSession> {
        let own_pid = std::process::id();
        let mut current = pid;
        for _ in 0..=MAX_ANCESTRY_HOPS {
            if current <= 1 {
                return None;
            }
            if let Ok(pid) = u32::try_from(current) {
                if pid == own_pid {
                    // The app itself is nobody's agent, and nothing above it
                    // can be.
                    return None;
                }
                if let Some(session) = grants.spawned_session_of(pid) {
                    return Some(session);
                }
            }
            let parent = self.parent_of(current)?;
            if parent == current {
                return None;
            }
            current = parent;
        }
        None
    }
}

/// The parent pid of `pid` from the kernel, or `None` when the process is
/// gone or unreadable.
#[cfg(target_os = "macos")]
fn system_parent_of(pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `proc_pidinfo` writes at most `size` bytes into `info`, which
    // is exactly `size` bytes of writable memory; it dereferences nothing
    // else. A short or failed read returns a value other than `size`, and
    // `info` is only read when the full struct was written.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: the kernel filled the whole struct (checked above).
    let info = unsafe { info.assume_init() };
    i32::try_from(info.pbi_ppid).ok()
}

#[cfg(target_os = "linux")]
fn system_parent_of(pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ppid ...` — `comm` may contain spaces and
    // parentheses, so split after the last `)`.
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn system_parent_of(_pid: i32) -> Option<i32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn session(id: &str) -> SpawnedSession {
        SpawnedSession {
            session_id: id.to_string(),
            epoch: 1,
            project_id: "p".to_string(),
        }
    }

    #[test]
    fn a_descendant_of_a_registered_agent_is_classified_as_its_session() {
        let grants = ControlGrants::default();
        grants.register_agent_process(500, session("s-1"));
        // stagedctl 503 ← sh 502 ← node tool 501 ← agent 500 ← app.
        let ancestry = Ancestry::fake(HashMap::from([(503, 502), (502, 501), (501, 500)]));
        assert_eq!(
            ancestry.spawned_session_of(503, &grants),
            Some(session("s-1"))
        );
        assert_eq!(
            ancestry.spawned_session_of(500, &grants),
            Some(session("s-1")),
            "the agent process itself"
        );
    }

    #[test]
    fn unrelated_and_unreadable_ancestry_is_nobodys_session() {
        let grants = ControlGrants::default();
        grants.register_agent_process(500, session("s-1"));
        // A user shell under launchd: never reaches an agent.
        let ancestry = Ancestry::fake(HashMap::from([(900, 800), (800, 1)]));
        assert_eq!(ancestry.spawned_session_of(900, &grants), None);
        // A parent that cannot be read stops the walk without a match.
        assert_eq!(ancestry.spawned_session_of(42, &grants), None);
        // A table that loops cannot spin.
        let looping = Ancestry::fake(HashMap::from([(7, 8), (8, 7)]));
        assert_eq!(looping.spawned_session_of(7, &grants), None);
        assert_eq!(ancestry.spawned_session_of(0, &grants), None);
        assert_eq!(ancestry.spawned_session_of(-1, &grants), None);
    }

    #[test]
    fn the_walk_is_bounded() {
        let grants = ControlGrants::default();
        grants.register_agent_process(1_000, session("deep"));
        // A straight chain below the agent: pid 1_000 + d is d hops from it.
        let chain: HashMap<i32, i32> = (1..=MAX_ANCESTRY_HOPS as i32 + 1)
            .map(|d| (1_000 + d, 1_000 + d - 1))
            .collect();
        let ancestry = Ancestry::fake(chain);
        let reachable = 1_000 + MAX_ANCESTRY_HOPS as i32;
        assert_eq!(
            ancestry.spawned_session_of(reachable, &grants),
            Some(session("deep")),
            "exactly MAX_ANCESTRY_HOPS hops is reachable"
        );
        assert_eq!(
            ancestry.spawned_session_of(reachable + 1, &grants),
            None,
            "one hop further is not"
        );
    }

    #[test]
    fn the_app_itself_ends_the_walk() {
        let grants = ControlGrants::default();
        let own = std::process::id() as i32;
        // Registering the app's own pid as an agent is nonsense, but even
        // then the app is not classified as a session.
        grants.register_agent_process(own as u32, session("never"));
        let ancestry = Ancestry::fake(HashMap::from([(77, own)]));
        assert_eq!(ancestry.spawned_session_of(77, &grants), None);
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn the_system_lookup_reads_our_own_parent() {
        let own = std::process::id() as i32;
        let parent = system_parent_of(own).expect("our parent is readable");
        assert!(parent >= 1, "{parent}");
        assert_ne!(parent, own);
        assert_eq!(system_parent_of(0), None);
        assert_eq!(system_parent_of(-5), None);
        // A pid no process holds.
        assert_eq!(system_parent_of(i32::MAX - 7), None);
    }
}
