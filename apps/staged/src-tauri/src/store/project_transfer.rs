//! Switching a project between local worktrees and a Blox workspace.
//!
//! By the time this runs, every branch with something to carry already exists
//! at the destination — a worktree on disk, or a running workspace — and the
//! source is untouched; see `crate::project_transfer`. A branch with nothing
//! to carry lands unprovisioned, for the usual setup to provision. What is left is the database's half: one
//! transaction that makes the rows describe the destination. Like
//! [`Store::move_branch_to_project`] it is `UPDATE`s plus the one `workdirs`
//! row, never a delete-and-reinsert of anything sessions hang off, so
//! transcripts, notes, reviews and commits stay where they are.

use rusqlite::params;

use super::branch_move::{duplicate_branch_error, duplicate_workdir_error};
use super::models::{BranchType, ProjectLocation, Workdir, WorkspaceStatus};
use super::{now_timestamp, Store, StoreChange, StoreError};

/// Where one transferred branch now lives.
#[derive(Debug, Clone)]
pub enum BranchLanding {
    /// A local branch. With a worktree, its row is inserted as given, and its
    /// `project_id` and `branch_id` must name the project and branch being
    /// transferred; without one, the branch waits for worktree setup.
    Local { workdir: Option<Workdir> },
    /// A Blox workspace: `Running` when the branch was carried into it,
    /// `Starting` for workspace setup to provision.
    Remote {
        workspace_name: String,
        status: WorkspaceStatus,
    },
}

/// One branch's part in a [`ProjectTransfer`].
#[derive(Debug, Clone)]
pub struct BranchTransfer {
    pub branch_id: String,
    /// The branch's name at the destination: its old name, or a de-duplicated
    /// one when the destination's clone already used it. A new name is also
    /// written to the branch's `project_repos` row, as a local rename is.
    pub branch_name: String,
    pub landing: BranchLanding,
    /// The `working_dir` every session on the branch now records: the
    /// worktree (or its subpath) locally, the workspace's repo placeholder
    /// remotely.
    pub session_working_dir: String,
}

/// Everything [`Store::apply_project_transfer`] rewrites, resolved by the
/// caller once the destination has been built and verified.
#[derive(Debug, Clone)]
pub struct ProjectTransfer {
    pub project_id: String,
    pub location: ProjectLocation,
    pub branches: Vec<BranchTransfer>,
}

impl Store {
    /// Point a project, and each of the given branches, at the destination.
    ///
    /// For every branch: its type, workspace and `setup_complete` (prerun
    /// actions have not run at the destination), its name if de-duplicated,
    /// its `workdirs` row (replaced for a local landing, removed for a remote
    /// one), and its sessions' `working_dir`. Each session's `agent_id` is
    /// cleared: the agent's own session lives on the machine it ran on, so a
    /// follow-up starts a fresh agent while the transcript stays.
    ///
    /// All or nothing. Refuses a branch that is not the project's, and a
    /// branch with a running session — the caller stops those first and holds
    /// the branch's launch lock across this call.
    pub fn apply_project_transfer(&self, transfer: &ProjectTransfer) -> Result<(), StoreError> {
        let landing_type = match transfer.location {
            ProjectLocation::Local => BranchType::Local,
            ProjectLocation::Remote => BranchType::Remote,
        };
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let now = now_timestamp();

        let updated = tx.execute(
            "UPDATE projects SET location = ?1, updated_at = ?2 WHERE id = ?3",
            params![transfer.location.as_str(), now, transfer.project_id],
        )?;
        if updated == 0 {
            return Err(StoreError(format!(
                "Project not found: {}",
                transfer.project_id
            )));
        }

        for branch in &transfer.branches {
            let (branch_type, workspace_name, workspace_status) = match &branch.landing {
                BranchLanding::Local { workdir } => {
                    if workdir.as_ref().is_some_and(|workdir| {
                        workdir.project_id != transfer.project_id
                            || workdir.branch_id.as_deref() != Some(branch.branch_id.as_str())
                    }) {
                        return Err(StoreError(format!(
                            "The worktree for branch {} belongs to another branch or project.",
                            branch.branch_id
                        )));
                    }
                    (BranchType::Local, None, None)
                }
                BranchLanding::Remote {
                    workspace_name,
                    status,
                } => (
                    BranchType::Remote,
                    Some(workspace_name.as_str()),
                    Some(status.as_str()),
                ),
            };
            if branch_type != landing_type {
                return Err(StoreError(format!(
                    "Branch {} would land {} in a {} project.",
                    branch.branch_id,
                    branch_type.as_str(),
                    transfer.location.as_str()
                )));
            }

            let running: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sessions WHERE branch_id = ?1 AND status = 'running'",
                params![branch.branch_id],
                |row| row.get(0),
            )?;
            if running > 0 {
                return Err(StoreError(format!(
                    "Branch {} has a running session; stop it before transferring.",
                    branch.branch_id
                )));
            }

            let project_repo_id: Option<Option<String>> = tx
                .query_row(
                    "SELECT project_repo_id FROM branches WHERE id = ?1 AND project_id = ?2",
                    params![branch.branch_id, transfer.project_id],
                    |row| row.get(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    e => Err(e),
                })?;
            let Some(project_repo_id) = project_repo_id else {
                return Err(StoreError(format!(
                    "Branch {} is not in project {}.",
                    branch.branch_id, transfer.project_id
                )));
            };

            tx.execute(
                "UPDATE branches
                    SET branch_type = ?1, workspace_name = ?2, workspace_status = ?3,
                        setup_complete = 0, branch_name = ?4, updated_at = ?5
                  WHERE id = ?6",
                params![
                    branch_type.as_str(),
                    workspace_name,
                    workspace_status,
                    branch.branch_name,
                    now,
                    branch.branch_id,
                ],
            )
            .map_err(|e| duplicate_branch_error(&e))?;
            if let Some(repo_id) = &project_repo_id {
                tx.execute(
                    "UPDATE project_repos SET branch_name = ?1, updated_at = ?2
                      WHERE id = ?3 AND project_id = ?4 AND branch_name IS NOT ?1",
                    params![branch.branch_name, now, repo_id, transfer.project_id],
                )?;
            }

            // `workdirs` has no delete triggers: the old location's row can go.
            tx.execute(
                "DELETE FROM workdirs WHERE branch_id = ?1",
                params![branch.branch_id],
            )?;
            if let BranchLanding::Local {
                workdir: Some(workdir),
            } = &branch.landing
            {
                tx.execute(
                    "INSERT INTO workdirs (id, project_id, path, branch_id, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        workdir.id,
                        workdir.project_id,
                        workdir.path,
                        workdir.branch_id,
                        workdir.created_at,
                        now,
                    ],
                )
                .map_err(|e| duplicate_workdir_error(&e))?;
            }

            // Selected by `branch_id`, not by path: a remote session's
            // `working_dir` is a placeholder shared by every branch on the
            // same repo, so a prefix would catch other branches' sessions.
            tx.execute(
                "UPDATE sessions SET working_dir = ?1, agent_id = NULL, updated_at = ?2
                  WHERE branch_id = ?3",
                params![branch.session_working_dir, now, branch.branch_id],
            )?;
        }

        // All or nothing: a branch the transfer didn't cover (one added while
        // it ran, say) would be left describing the other location.
        let left_out: Option<String> = tx
            .query_row(
                "SELECT branch_name FROM branches WHERE project_id = ?1 AND branch_type != ?2
                  ORDER BY created_at LIMIT 1",
                params![transfer.project_id, landing_type.as_str()],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        if let Some(name) = left_out {
            return Err(StoreError(format!(
                "Branch {name} isn't part of the transfer (was it added while the transfer ran?); \
                 every branch moves with the project, so nothing was changed."
            )));
        }

        tx.commit()?;
        for branch in &transfer.branches {
            self.publish(StoreChange::Branch {
                branch_id: branch.branch_id.clone(),
                project_id: Some(transfer.project_id.clone()),
            });
        }
        self.publish(StoreChange::Project {
            project_id: Some(transfer.project_id.clone()),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::store::models::{Branch, Note, Project, ProjectRepo, Session, SessionStatus};

    struct Fixture {
        store: Store,
        project: Project,
        repo: ProjectRepo,
        branch: Branch,
    }

    /// A local project with one branch in a worktree, on its own repo row.
    fn local_fixture(store: Store) -> Fixture {
        let project = Project::named("transfer").with_primary_repo("acme/widgets");
        store.create_project(&project).unwrap();
        let repo = ProjectRepo::new(&project.id, "acme/widgets", "feature", None).primary();
        store.create_project_repo(&repo).unwrap();
        let branch = Branch::new(&project.id, "feature", "origin/main").with_project_repo(&repo.id);
        store.create_branch(&branch).unwrap();
        store
            .create_workdir(
                &Workdir::new(&project.id, "/wt/acme-widgets--feature").with_branch(&branch.id),
            )
            .unwrap();
        store.mark_branch_setup_complete(&branch.id).unwrap();
        Fixture {
            store,
            project,
            repo,
            branch,
        }
    }

    fn to_remote(f: &Fixture, workspace: &str) -> ProjectTransfer {
        ProjectTransfer {
            project_id: f.project.id.clone(),
            location: ProjectLocation::Remote,
            branches: vec![BranchTransfer {
                branch_id: f.branch.id.clone(),
                branch_name: f.branch.branch_name.clone(),
                landing: BranchLanding::Remote {
                    workspace_name: workspace.to_string(),
                    status: WorkspaceStatus::Running,
                },
                session_working_dir: "/home/me/.staged/repos/acme/widgets".to_string(),
            }],
        }
    }

    fn to_local(f: &Fixture, branch_name: &str, path: &str) -> ProjectTransfer {
        ProjectTransfer {
            project_id: f.project.id.clone(),
            location: ProjectLocation::Local,
            branches: vec![BranchTransfer {
                branch_id: f.branch.id.clone(),
                branch_name: branch_name.to_string(),
                landing: BranchLanding::Local {
                    workdir: Some(Workdir::new(&f.project.id, path).with_branch(&f.branch.id)),
                },
                session_working_dir: format!("{path}/apps/web"),
            }],
        }
    }

    fn finished_session(f: &Fixture, prompt: &str, agent: &str) -> Session {
        let mut session = Session::new_running(prompt, Path::new("/wt/acme-widgets--feature"))
            .with_branch(&f.branch.id)
            .with_agent(agent);
        session.status = SessionStatus::Completed;
        f.store.create_session(&session).unwrap();
        session
    }

    #[test]
    fn to_remote_drops_the_workdir_and_points_everything_at_the_workspace() {
        let f = local_fixture(Store::in_memory().unwrap());
        let session = finished_session(&f, "earlier work", "acp-session-1");
        let note = Note::new(&f.branch.id, "Findings", "body").with_session(&session.id);
        f.store.create_note(&note).unwrap();

        f.store
            .apply_project_transfer(&to_remote(&f, "ws-feature"))
            .unwrap();

        let project = f.store.get_project(&f.project.id).unwrap().unwrap();
        assert_eq!(project.location, ProjectLocation::Remote);
        let branch = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Remote);
        assert_eq!(branch.workspace_name.as_deref(), Some("ws-feature"));
        assert_eq!(branch.workspace_status, Some(WorkspaceStatus::Running));
        assert!(!branch.setup_complete);
        assert!(f
            .store
            .get_workdir_for_branch(&f.branch.id)
            .unwrap()
            .is_none());
        let session = f.store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(session.working_dir, "/home/me/.staged/repos/acme/widgets");
        assert_eq!(session.agent_id, None, "the agent stays behind");
        assert_eq!(
            f.store.list_notes_for_branch(&f.branch.id).unwrap().len(),
            1,
            "the transcript and what hangs off it stay"
        );
    }

    #[test]
    fn to_local_inserts_the_workdir_and_records_a_deduplicated_name() {
        let f = local_fixture(Store::in_memory().unwrap());
        f.store
            .apply_project_transfer(&to_remote(&f, "ws-feature"))
            .unwrap();
        let session = finished_session(&f, "remote work", "acp-session-2");

        f.store
            .apply_project_transfer(&to_local(&f, "feature-2", "/wt/acme-widgets--feature-2"))
            .unwrap();

        let project = f.store.get_project(&f.project.id).unwrap().unwrap();
        assert_eq!(project.location, ProjectLocation::Local);
        let branch = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Local);
        assert_eq!(branch.branch_name, "feature-2");
        assert_eq!(branch.workspace_name, None);
        assert_eq!(branch.workspace_status, None);
        assert!(
            !branch.setup_complete,
            "prerun actions run at the destination"
        );
        let repo = f.store.get_project_repo(&f.repo.id).unwrap().unwrap();
        assert_eq!(repo.branch_name, "feature-2");
        let workdir = f
            .store
            .get_workdir_for_branch(&f.branch.id)
            .unwrap()
            .unwrap();
        assert_eq!(workdir.path, "/wt/acme-widgets--feature-2");
        assert_eq!(workdir.project_id, f.project.id);
        let session = f.store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(session.working_dir, "/wt/acme-widgets--feature-2/apps/web");
        assert_eq!(session.agent_id, None);
    }

    /// Remote sessions share one placeholder `working_dir` per repo, so
    /// another branch's session there — in another project on the same repo —
    /// must not move with this branch.
    #[test]
    fn rewrites_only_the_transferred_branches_sessions() {
        let f = local_fixture(Store::in_memory().unwrap());
        let other = Project::named("other").with_primary_repo("acme/widgets");
        f.store.create_project(&other).unwrap();
        let sibling = Branch::new(&other.id, "other", "origin/main");
        f.store.create_branch(&sibling).unwrap();
        let mut theirs = Session::new_running("sibling", Path::new("/wt/acme-widgets--feature"))
            .with_branch(&sibling.id)
            .with_agent("acp-sibling");
        theirs.status = SessionStatus::Completed;
        f.store.create_session(&theirs).unwrap();
        let queued = Session::new_queued("follow-up").with_branch(&f.branch.id);
        f.store.create_session(&queued).unwrap();

        f.store
            .apply_project_transfer(&to_remote(&f, "ws-feature"))
            .unwrap();

        let theirs = f.store.get_session(&theirs.id).unwrap().unwrap();
        assert_eq!(theirs.working_dir, "/wt/acme-widgets--feature");
        assert_eq!(theirs.agent_id.as_deref(), Some("acp-sibling"));
        let queued = f.store.get_session(&queued.id).unwrap().unwrap();
        assert_eq!(queued.working_dir, "/home/me/.staged/repos/acme/widgets");
    }

    #[test]
    fn publishes_each_branch_then_the_project() {
        let (sender, mut rx) = tokio::sync::broadcast::channel(64);
        let f = local_fixture(Store::in_memory().unwrap().with_change_sender(sender));
        while rx.try_recv().is_ok() {}

        f.store
            .apply_project_transfer(&to_remote(&f, "ws-feature"))
            .unwrap();

        assert_eq!(
            rx.try_recv().unwrap(),
            StoreChange::Branch {
                branch_id: f.branch.id.clone(),
                project_id: Some(f.project.id.clone()),
            }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            StoreChange::Project {
                project_id: Some(f.project.id.clone()),
            }
        );
        assert!(rx.try_recv().is_err());
    }

    /// A failure on the last branch leaves the first one, and the project,
    /// exactly as they were — and publishes nothing.
    #[test]
    fn a_failure_part_way_through_changes_nothing() {
        let (sender, mut rx) = tokio::sync::broadcast::channel(64);
        let f = local_fixture(Store::in_memory().unwrap().with_change_sender(sender));
        let second =
            Branch::new(&f.project.id, "second", "origin/main").with_project_repo(&f.repo.id);
        f.store.create_branch(&second).unwrap();
        let running = Session::new_running("busy", Path::new("/wt/x")).with_branch(&second.id);
        f.store.create_session(&running).unwrap();
        let session = finished_session(&f, "earlier", "acp-session-1");
        while rx.try_recv().is_ok() {}

        let mut transfer = to_remote(&f, "ws-feature");
        transfer.branches.push(BranchTransfer {
            branch_id: second.id.clone(),
            branch_name: "second".to_string(),
            landing: BranchLanding::Remote {
                workspace_name: "ws-feature".to_string(),
                status: WorkspaceStatus::Running,
            },
            session_working_dir: "/home/me/.staged/repos/acme/widgets".to_string(),
        });

        let err = f.store.apply_project_transfer(&transfer).unwrap_err();

        assert!(err.0.contains("running session"), "{err}");
        let project = f.store.get_project(&f.project.id).unwrap().unwrap();
        assert_eq!(project.location, ProjectLocation::Local);
        let branch = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Local);
        assert!(branch.setup_complete);
        assert!(f
            .store
            .get_workdir_for_branch(&f.branch.id)
            .unwrap()
            .is_some());
        let session = f.store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(session.agent_id.as_deref(), Some("acp-session-1"));
        assert_eq!(session.working_dir, "/wt/acme-widgets--feature");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn refuses_a_branch_from_another_project_or_a_mismatched_landing() {
        let f = local_fixture(Store::in_memory().unwrap());
        let other = Project::named("other");
        f.store.create_project(&other).unwrap();
        let stranger = Branch::new(&other.id, "stranger", "origin/main");
        f.store.create_branch(&stranger).unwrap();

        let mut transfer = to_remote(&f, "ws-feature");
        transfer.branches[0].branch_id = stranger.id.clone();
        let err = f.store.apply_project_transfer(&transfer).unwrap_err();
        assert!(err.0.contains("is not in project"), "{err}");

        let mut transfer = to_local(&f, "feature", "/wt/x");
        transfer.location = ProjectLocation::Remote;
        assert!(f.store.apply_project_transfer(&transfer).is_err());

        let mut transfer = to_local(&f, "feature", "/wt/x");
        let BranchLanding::Local {
            workdir: Some(workdir),
        } = &mut transfer.branches[0].landing
        else {
            unreachable!()
        };
        workdir.branch_id = Some(stranger.id.clone());
        assert!(f.store.apply_project_transfer(&transfer).is_err());

        assert_eq!(
            f.store
                .get_project(&f.project.id)
                .unwrap()
                .unwrap()
                .location,
            ProjectLocation::Local
        );
        assert_eq!(
            f.store
                .get_branch(&stranger.id)
                .unwrap()
                .unwrap()
                .branch_type,
            BranchType::Local
        );
    }

    /// A branch with nothing to carry lands unprovisioned: no worktree row
    /// locally, `Starting` remotely, for the usual setup to pick up.
    #[test]
    fn lands_branches_with_nothing_to_carry_unprovisioned() {
        let f = local_fixture(Store::in_memory().unwrap());
        let mut transfer = to_remote(&f, "ws-feature");
        transfer.branches[0].landing = BranchLanding::Remote {
            workspace_name: "ws-feature".to_string(),
            status: WorkspaceStatus::Starting,
        };
        f.store.apply_project_transfer(&transfer).unwrap();
        let branch = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(branch.workspace_status, Some(WorkspaceStatus::Starting));

        let mut transfer = to_local(&f, "feature", "/wt/unused");
        transfer.branches[0].landing = BranchLanding::Local { workdir: None };
        f.store.apply_project_transfer(&transfer).unwrap();
        let branch = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Local);
        assert_eq!(branch.workspace_status, None);
        assert!(f
            .store
            .get_workdir_for_branch(&f.branch.id)
            .unwrap()
            .is_none());
    }

    /// The `branches` unique index is the backstop for a de-duplicated name
    /// that collides after all.
    #[test]
    fn a_colliding_name_is_refused_readably() {
        let f = local_fixture(Store::in_memory().unwrap());
        f.store
            .apply_project_transfer(&to_remote(&f, "ws-feature"))
            .unwrap();
        // Local, so the all-or-nothing check passes and the index decides.
        let taken =
            Branch::new(&f.project.id, "feature-2", "origin/main").with_project_repo(&f.repo.id);
        f.store.create_branch(&taken).unwrap();

        let err = f
            .store
            .apply_project_transfer(&to_local(&f, "feature-2", "/wt/acme-widgets--feature-2"))
            .unwrap_err();

        assert!(err.0.contains("already tracks a branch"), "{err}");
        let branch = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Remote);
        assert_eq!(branch.branch_name, "feature");
    }

    /// A branch that joined the project after the transfer looked fails the
    /// whole transfer rather than staying behind at the old location.
    #[test]
    fn a_branch_left_out_of_the_transfer_fails_it_and_changes_nothing() {
        let (sender, mut rx) = tokio::sync::broadcast::channel(64);
        let f = local_fixture(Store::in_memory().unwrap().with_change_sender(sender));
        let late = Branch::new(&f.project.id, "late", "origin/main").with_project_repo(&f.repo.id);
        f.store.create_branch(&late).unwrap();
        while rx.try_recv().is_ok() {}

        let err = f
            .store
            .apply_project_transfer(&to_remote(&f, "ws-feature"))
            .unwrap_err();

        assert!(
            err.0.contains("Branch late isn't part of the transfer"),
            "{err}"
        );
        let project = f.store.get_project(&f.project.id).unwrap().unwrap();
        assert_eq!(project.location, ProjectLocation::Local);
        let branch = f.store.get_branch(&f.branch.id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Local);
        assert_eq!(branch.workspace_name, None);
        assert!(f
            .store
            .get_workdir_for_branch(&f.branch.id)
            .unwrap()
            .is_some());
        assert!(rx.try_recv().is_err());
    }
}
