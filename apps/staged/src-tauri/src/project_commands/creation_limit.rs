//! How many projects a control actor may hold, and whose projects count.
//!
//! The limit is there to stop a runaway: a session that keeps creating
//! projects, each of which may clone a repository or provision a workspace.
//! Counting per session id would not stop one, because a session can start
//! another session, and that one has a session id of its own.
//!
//! So the allowance belongs to a chain. `sessions.started_by` records the
//! session that started a session, and following it upward ends at the
//! chain's root: a session the user started in the app, one a connected app
//! started, or one whose row is gone. Every session under one root draws on
//! one allowance, counted from the `projects.created_by` rows of all of
//! them, so deleting any of those projects in the app frees a slot.
//!
//! Nothing new is stored for this. The root is found by walking
//! `started_by`, and the walk is bounded
//! ([`SESSION_LINEAGE_MAX_DEPTH`]).

use super::create::ProjectCreator;
use crate::store::{session_actor_id, Store};

/// How many `started_by` links the walk to a chain's root follows. A session
/// further than this from its root is refused a project: a chain that long
/// is what a runaway looks like, and treating some session part-way up as
/// the root would hand every stretch of the chain a fresh allowance.
pub(crate) const SESSION_LINEAGE_MAX_DEPTH: usize = 32;

impl ProjectCreator {
    /// Refuse when the creator's chain already holds as many projects as it
    /// may.
    ///
    /// Only a session is limited today (see `ProjectCreator::for_grant`),
    /// which is why the message speaks of one.
    pub(super) fn ensure_within_limit(&self, store: &Store) -> Result<(), String> {
        let Some(limit) = self.limit else {
            return Ok(());
        };
        let held = projects_held_by_chain(store, &self.actor)?;
        if held >= limit {
            return Err(format!(
                "This session and the sessions in its chain have created {held} projects, \
                 the limit they share. A chain is a session and every session started from \
                 it, directly or through another. Deleting one of those projects in Staged \
                 frees a slot."
            ));
        }
        Ok(())
    }
}

/// The projects that count toward `actor`'s limit: for a session, those
/// created by any session under the same root as it. An actor that is not a
/// session has no chain, and holds what it created itself.
fn projects_held_by_chain(store: &Store, actor: &str) -> Result<usize, String> {
    let Some(session_id) = session_actor_id(actor) else {
        return store
            .list_project_ids_created_by(actor)
            .map(|ids| ids.len())
            .map_err(|e| e.to_string());
    };
    let root = lineage_root(store, session_id)?.ok_or_else(|| {
        format!(
            "This session is more than {SESSION_LINEAGE_MAX_DEPTH} sessions removed from the \
             one that started its chain, and a chain that long may not create projects."
        )
    })?;

    // One walk per session that holds a project, not per session in the
    // chain: the creators are few, and a chain's sessions can be many.
    let mut held = 0;
    for (creator, projects) in store
        .count_projects_by_creator()
        .map_err(|e| e.to_string())?
    {
        let Some(creator_id) = session_actor_id(&creator) else {
            continue;
        };
        // A creator beyond the bound has no root here and counts toward no
        // chain. No session that deep is let create a project.
        if creator_id == session_id || lineage_root(store, creator_id)?.as_ref() == Some(&root) {
            held += projects;
        }
    }
    Ok(held)
}

fn lineage_root(store: &Store, session_id: &str) -> Result<Option<String>, String> {
    store
        .session_lineage_root(session_id, SESSION_LINEAGE_MAX_DEPTH)
        .map_err(|e| e.to_string())
}
