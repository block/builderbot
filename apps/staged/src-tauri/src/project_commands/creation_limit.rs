//! How many projects a control actor may hold, and whose projects count.
//!
//! The limit is there to stop a runaway: a session that keeps creating
//! projects, each of which may clone a repository or provision a workspace.
//! Counting per session id would not stop one, because a session can start
//! another session, and that one has a session id of its own.
//!
//! So the allowance belongs to a chain. A session row records the session
//! that started it: in `sessions.started_by` for a start made through
//! `stagedctl`, and in `sessions.parent_session_id` for one made through an
//! MCP tool, which is how a project session starts a repo session
//! (`start_repo_session`). Following those links upward ends at the chain's
//! root: a session the user started in the app, or one a connected app
//! started. Every session under one root draws on one allowance, counted
//! from the `projects.created_by` rows of all of them, so deleting any of
//! those projects in the app frees a slot.
//!
//! Both links are followed because either one alone leaves a way out. A
//! session at its limit can start a project session with `stagedctl`, and
//! that one can start a repo session with the MCP tool. The repo session has
//! no `started_by`, and read by that link alone it was a root with a fresh
//! allowance.
//!
//! Nothing new is stored for this. The root is found by walking the links
//! ([`Store::session_lineage_root`]), and the walk is bounded
//! ([`SESSION_LINEAGE_MAX_DEPTH`]).
//!
//! # A deleted session splits its chain
//!
//! The chain is rebuilt from the session rows on every count, and session
//! rows go when the user deletes a project, a branch or a timeline item.
//! When a session in the middle of a chain is deleted, the sessions below it
//! no longer lead to the root:
//!
//! - Sessions it started through `stagedctl` still name its id, and share
//!   that id as their root.
//! - Sessions it started through an MCP tool lose their parent link (the
//!   foreign key is `ON DELETE SET NULL`), and each becomes a root.
//!
//! Either way the projects those sessions hold stop counting against the
//! original root, which gets that much of its allowance back while the
//! projects still exist, and the split-off sessions count from nothing.
//! This is known and left as it is. Only the user can delete a session, so a
//! session cannot use it to get past the limit. Closing it would mean
//! storing the root on the project, which is a change to the data model.

use super::create::ProjectCreator;
use crate::store::{session_actor_id, Store};

/// How many links the walk to a chain's root follows, of either kind. A
/// session further than this from its root is refused a project: a chain
/// that long is what a runaway looks like, and treating some session
/// part-way up as the root would hand every stretch of the chain a fresh
/// allowance.
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
