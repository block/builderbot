//! Moving a whole project: prepare the destination, carry each branch,
//! switch the database, tear the source down.
//!
//! Until [`Store::apply_project_transfer`] commits, the source and the
//! database are untouched and every step is undone on failure, panic or
//! cancel (checked between steps): the workspace or the worktrees, branches
//! and remotes the transfer created are removed, branch config it replaced is
//! put back, and the temporary refs at both ends are dropped (see
//! [`super::undo`]). Each is written to the manifest before it is made, so a
//! transfer Staged never finished is undone at the next start instead. The
//! bundles on this machine always stay, as the backup. After the commit
//! nothing is undone: tearing the source down is best-effort — and refused
//! for a source that changed after its snapshot — and what it leaves comes
//! back as warnings on a successful transfer (see [`teardown`]).
//!
//! Synchronous, for the blocking pool; [`super::commands`] wraps it.

mod teardown;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::store::{
    BranchLanding, BranchTransfer, ProjectLocation, ProjectTransfer, Store, Workdir,
    WorkspaceStatus,
};

use super::backups::{self, ManifestRepo, TransferManifest};
use super::carry::{
    carry_repo_with, finish, origin_tips, run_step_sized, CarryOutcome, CarryPlan, CarryStep,
    Landing,
};
use super::endpoint::TransferEndpoint;
use super::host::{TransferHost, WorkspaceState};
use super::preflight::{
    self, BranchSource, Inspection, RepoPreflight, RepoTransferState, TransferDirection,
    TransferIssue,
};
use super::registry::TransferGuard;
use super::scripts::{Snapshot, CLONE_SCRIPT};
use super::undo::{
    self, AddedRemote, AddedWorktree, CreatedBranch, ReplacedConfig, SourceCheckout, Undo,
};
use super::TransferError;

/// Where a transfer is, as `project-transfer-progress` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase {
    /// Inspecting the source (resuming a suspended workspace first).
    Checking,
    /// Creating the workspace and clones, or the local clones.
    PreparingDestination,
    Snapshotting,
    Moving,
    Applying,
    /// Switching the database over: the last point a cancel is honoured.
    Committing,
    TearingDown,
    Completed,
    RollingBack,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub project_id: String,
    pub transfer_id: String,
    pub direction: TransferDirection,
    pub phase: TransferPhase,
    /// The branch the phase is about, when it is about one.
    pub branch_id: Option<String>,
    pub project_repo_id: Option<String>,
    pub detail: Option<String>,
    /// The bundle's size, while [`TransferPhase::Moving`].
    pub bytes: Option<u64>,
}

/// One branch, after the transfer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoTransferResult {
    pub branch_id: String,
    pub project_repo_id: Option<String>,
    pub github_repo: String,
    /// Its name at the destination.
    pub branch_name: String,
    pub previous_branch_name: String,
    /// Whether there was a checkout to carry; without one the branch starts
    /// from origin at the destination.
    pub carried: bool,
    pub head: Option<String>,
    /// Uncommitted changes were restored at the destination.
    pub uncommitted_restored: bool,
    /// Uncommitted changes stayed behind, in the backup only.
    pub uncommitted_left_behind: bool,
    pub retried_without_exclusions: bool,
    /// The ref holding the source's snapshot in the shared local clone.
    pub backup_ref: Option<String>,
    pub warnings: Vec<TransferIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTransferResult {
    pub project_id: String,
    pub transfer_id: String,
    pub direction: TransferDirection,
    pub location: ProjectLocation,
    /// The workspace on the remote side: created by a transfer to remote
    /// (or only named, when there was nothing to carry), deleted by one to
    /// local.
    pub workspace_name: Option<String>,
    pub repos: Vec<RepoTransferResult>,
    /// What tearing the source down could not do.
    pub warnings: Vec<TransferIssue>,
    /// `~/.staged/transfers/<transfer id>`, when anything was carried.
    pub backup_dir: Option<String>,
}

/// Move the project `guard` registered to the other location.
pub(super) fn run_transfer(
    store: &Store,
    host: &dyn TransferHost,
    guard: &TransferGuard,
    include_uncommitted: bool,
    on_progress: &dyn Fn(&TransferProgress),
) -> Result<ProjectTransferResult, String> {
    let project = store
        .get_project(guard.project_id())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", guard.project_id()))?;
    let direction = TransferDirection::from_source(project.location);
    let mut run = Run {
        store,
        host,
        guard,
        include_uncommitted,
        on_progress,
        direction,
        transfer_dir: host.transfers_dir()?.join(guard.transfer_id()),
        workspace_name: None,
        journal: RefCell::new(TransferManifest {
            transfer_id: guard.transfer_id().to_string(),
            project_id: project.id.clone(),
            direction: direction.as_str().to_string(),
            created_at: crate::store::now_timestamp(),
            include_uncommitted,
            status: backups::CARRYING.to_string(),
            owner_pid: Some(std::process::id()),
            ..TransferManifest::default()
        }),
        journaled: Cell::new(false),
        committed: Cell::new(false),
    };
    match catch_unwind(AssertUnwindSafe(|| run.execute())) {
        Ok(Ok(result)) => {
            run.emit(TransferPhase::Completed, None, None, None);
            Ok(result)
        }
        // Only the steps up to the commit can fail.
        Ok(Err(Failure { message })) => Err(run.fail(message)),
        Err(panic) => {
            let what = panic_message(panic.as_ref());
            run.clear_poisoned_launch_locks();
            if !run.committed.get() {
                return Err(run.fail(format!("The transfer stopped unexpectedly: {what}")));
            }
            // The database has switched: the destination is the project now,
            // so there is nothing to roll back to.
            let message = format!(
                "The project moved, but the transfer stopped unexpectedly while removing \
                 the source ({what}); what it hadn't removed yet is still there."
            );
            log::error!(
                "[project_transfer] transfer {} of {}: {message}",
                guard.transfer_id(),
                guard.project_id()
            );
            run.close_journal(backups::COMPLETED, None, Some(message.clone()));
            run.emit(TransferPhase::Failed, None, Some(message.clone()), None);
            Err(message)
        }
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("a panic")
}

/// Why the transfer stopped, for `?` on every error type it meets.
struct Failure {
    message: String,
}

impl<E: ToString> From<E> for Failure {
    fn from(e: E) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}

/// One branch's progress through the transfer.
struct Leg {
    source: BranchSource,
    row: RepoPreflight,
    carry: bool,
    dest_branch: String,
    /// The destination: the workspace clone, or the local clone.
    dest_dir: Option<String>,
    exclude_tips: Vec<String>,
    plan: Option<CarryPlan>,
    outcome: Option<CarryOutcome>,
    /// The worktree added for it locally.
    worktree: Option<PathBuf>,
    backup_ref: Option<String>,
    warnings: Vec<TransferIssue>,
}

struct Run<'a> {
    store: &'a Store,
    host: &'a dyn TransferHost,
    guard: &'a TransferGuard,
    include_uncommitted: bool,
    on_progress: &'a dyn Fn(&TransferProgress),
    direction: TransferDirection,
    transfer_dir: PathBuf,
    workspace_name: Option<String>,
    /// The manifest as it stands, [`Undo`] list included: written before
    /// the transfer first makes anything, and again with each change.
    journal: RefCell<TransferManifest>,
    /// Whether the manifest has been written.
    journaled: Cell<bool>,
    /// Whether the database has switched.
    committed: Cell<bool>,
}

fn path_str(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("not a UTF-8 path: {}", path.display()))
}

/// What a destination fetches from origin before the source bundles, so the
/// bundle can leave those commits out: the base, the branch, and the
/// branch's upstream when it is on origin.
fn origin_tip_names(leg: &Leg) -> Vec<String> {
    let mut names = vec![
        crate::branches::normalize_branch_ref(&leg.source.branch.base_branch),
        leg.row.branch_name.clone(),
    ];
    if let Some(upstream) = leg
        .row
        .upstream
        .as_deref()
        .and_then(|u| u.strip_prefix("origin/"))
    {
        names.push(upstream.to_string());
    }
    let mut seen = HashSet::new();
    names.retain(|n| !n.is_empty() && seen.insert(n.clone()));
    names
}

impl Run<'_> {
    fn emit(
        &self,
        phase: TransferPhase,
        leg: Option<&Leg>,
        detail: Option<String>,
        bytes: Option<u64>,
    ) {
        let progress = TransferProgress {
            project_id: self.guard.project_id().to_string(),
            transfer_id: self.guard.transfer_id().to_string(),
            direction: self.direction,
            phase,
            branch_id: leg.map(|l| l.source.branch.id.clone()),
            project_repo_id: leg.and_then(|l| l.source.branch.project_repo_id.clone()),
            detail,
            bytes,
        };
        self.guard.report(&progress);
        (self.on_progress)(&progress);
    }

    /// Report `phase`, then stop here if the transfer was cancelled.
    fn checkpoint(
        &self,
        phase: TransferPhase,
        leg: Option<&Leg>,
        detail: Option<String>,
    ) -> Result<(), TransferError> {
        self.emit(phase, leg, detail, None);
        if self.guard.is_cancelled() {
            Err(TransferError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn execute(&mut self) -> Result<ProjectTransferResult, Failure> {
        self.checkpoint(TransferPhase::Checking, None, None)?;
        let inspection = self.inspect()?;
        self.guard
            .cover_branches(inspection.branches.iter().map(|b| b.branch.id.as_str()));
        self.guard.cover_workspaces(
            inspection
                .branches
                .iter()
                .filter_map(|b| b.workspace.as_deref()),
        );
        self.check_sessions(&inspection)?;
        self.journal.borrow_mut().workspace_name = inspection.preflight.workspace_name.clone();

        let mut legs: Vec<Leg> = inspection
            .branches
            .iter()
            .cloned()
            .zip(inspection.preflight.repos.iter().cloned())
            .map(|(source, row)| Leg {
                carry: row.state == RepoTransferState::Carry && source.source_dir.is_some(),
                dest_branch: row.branch_name.clone(),
                source,
                row,
                dest_dir: None,
                exclude_tips: Vec::new(),
                plan: None,
                outcome: None,
                worktree: None,
                backup_ref: None,
                warnings: Vec::new(),
            })
            .collect();

        match self.direction {
            TransferDirection::ToRemote => self.prepare_workspace(&inspection, &mut legs)?,
            TransferDirection::ToLocal => self.prepare_clones(&inspection, &mut legs)?,
        }
        for (i, leg) in legs.iter_mut().enumerate() {
            if leg.carry {
                self.carry(i, leg)?;
                let repo = self.manifest_repo(leg);
                self.journal.borrow_mut().repos.extend(repo);
                self.save_journal()?;
            }
        }

        self.checkpoint(TransferPhase::Committing, None, None)?;
        self.commit(&inspection, &legs)?;
        self.committed.set(true);
        self.close_journal(backups::COMMITTED, None, None);
        // The rows describe the destination now, and setup may provision them
        // while the source comes down.
        self.guard.release_branches();

        let warnings = self.tear_down(&inspection, &mut legs);
        self.close_journal(backups::COMPLETED, Some(&legs), None);
        Ok(self.result(&inspection, legs, warnings))
    }

    /// The preflight, again, now that the transfer is registered — with any
    /// suspended, stopped or failed source workspace resumed first, so its
    /// checkouts can be read. One that won't come back blocks the transfer.
    fn inspect(&self) -> Result<Inspection, Failure> {
        let project_id = self.guard.project_id();
        let mut inspection = preflight::inspect(self.store, self.host, project_id)?;
        let to_resume: BTreeSet<String> = inspection
            .branches
            .iter()
            .zip(&inspection.preflight.repos)
            .filter(|(_, row)| row.state == RepoTransferState::Unknown)
            .filter_map(|(source, _)| source.workspace.clone())
            .collect();
        if !to_resume.is_empty() {
            let mut resumed = BTreeMap::new();
            for workspace in to_resume {
                self.checkpoint(
                    TransferPhase::Checking,
                    None,
                    Some(format!("Resuming workspace {workspace}")),
                )?;
                let error = self
                    .host
                    .resume_workspace(&workspace, &|| self.guard.is_cancelled())
                    .err();
                self.guard_not_cancelled()?;
                let failed = error.is_some();
                match &error {
                    Some(e) => log::warn!("[project_transfer] cannot resume {workspace}: {e}"),
                    // Running now, whatever the transfer comes to: the rows
                    // say so, rather than the state it was resumed from.
                    None => {
                        self.store.update_workspace_status_by_workspace_name(
                            &workspace,
                            &WorkspaceStatus::Running,
                        )?;
                    }
                }
                resumed.insert(workspace, error);
                // One is enough to block the transfer: spare the others.
                if failed {
                    break;
                }
            }
            inspection =
                preflight::inspect_after_resuming(self.store, self.host, project_id, &resumed)?;
        }
        let preflight = &inspection.preflight;
        if !preflight.can_transfer {
            let reasons: Vec<&str> = preflight
                .blockers
                .iter()
                .chain(preflight.repos.iter().flat_map(|r| &r.blockers))
                .map(|issue| issue.message.as_str())
                .collect();
            return Err(format!("Can't transfer the project: {}", reasons.join(" ")).into());
        }
        Ok(inspection)
    }

    /// No session may be running or queued on a branch. Taking each launch
    /// lock waits out a start that was already past its registry check when
    /// the transfer registered; every later one is refused.
    fn check_sessions(&self, inspection: &Inspection) -> Result<(), Failure> {
        for source in &inspection.branches {
            let lock = crate::session_commands::branch_session_launch_lock_for(&source.branch.id);
            let _guard = lock.lock().unwrap();
            let running = self
                .store
                .has_running_session_for_branch(&source.branch.id)?;
            let queued = !self
                .store
                .get_queued_sessions_for_branch(&source.branch.id)?
                .is_empty();
            if running || queued {
                return Err(format!(
                    "A session started on {}; stop it before transferring.",
                    source.branch.branch_name
                )
                .into());
            }
        }
        Ok(())
    }

    /// A free workspace name, from the primary branch's.
    fn pick_workspace_name(&self, seed: &str) -> Result<String, Failure> {
        const MAX: usize = 32;
        let base = crate::branches::infer_workspace_name(seed);
        for n in 1..=20 {
            let name = if n == 1 {
                base.clone()
            } else {
                let suffix = format!("-{n}");
                let keep = base.len().min(MAX - suffix.len());
                format!("{}{suffix}", base[..keep].trim_end_matches('-'))
            };
            if self.host.workspace_state(&name)? == WorkspaceState::NotFound {
                return Ok(name);
            }
        }
        Err(format!("No free workspace name like {base}").into())
    }

    /// To remote: name the workspace and, when there is anything to carry,
    /// create it, clone every repo to carry into it, and fetch what origin
    /// has. Branches with nothing to carry are cloned later by workspace
    /// setup.
    fn prepare_workspace(
        &mut self,
        inspection: &Inspection,
        legs: &mut [Leg],
    ) -> Result<(), Failure> {
        self.checkpoint(TransferPhase::PreparingDestination, None, None)?;
        let primary_repo = inspection.project.primary_repo();
        let primary = legs
            .iter()
            .position(|l| l.carry && Some(l.source.github_repo.as_str()) == primary_repo)
            .or_else(|| legs.iter().position(|l| l.carry));
        let seed = primary
            .or((!legs.is_empty()).then_some(0))
            .map(|i| legs[i].row.branch_name.clone())
            .unwrap_or_else(|| crate::branches::infer_branch_name(&inspection.project.name));
        let workspace = self.pick_workspace_name(&seed)?;
        self.workspace_name = Some(workspace.clone());
        let Some(primary) = primary else {
            return Ok(());
        };

        // Recorded before the start: a start that fails half way can still
        // have created it.
        self.journal.borrow_mut().workspace_name = Some(workspace.clone());
        self.record(|undo| undo.workspace = Some(workspace.clone()))?;
        let base = crate::branches::normalize_branch_ref(&legs[primary].source.branch.base_branch);
        let started = self.host.start_workspace(
            &workspace,
            &legs[primary].source.github_repo,
            &base,
            &|line: &str| {
                self.emit(
                    TransferPhase::PreparingDestination,
                    None,
                    Some(line.to_string()),
                    None,
                )
            },
            &|| self.guard.is_cancelled(),
        );
        self.guard_not_cancelled()?;
        started?;

        let endpoint = self.host.workspace_endpoint(&workspace);
        for leg in legs.iter_mut().filter(|l| l.carry) {
            self.checkpoint(
                TransferPhase::PreparingDestination,
                Some(leg),
                Some(format!("Cloning {}", leg.source.github_repo)),
            )?;
            let dir = self
                .host
                .workspace_repo_dir(&workspace, &leg.source.github_repo)?;
            run_step_sized(
                endpoint.as_ref(),
                "clone",
                CLONE_SCRIPT,
                &[
                    dir.clone(),
                    self.host.workspace_clone_url(&leg.source.github_repo),
                ],
                leg.source.repo_bytes,
            )?;
            self.host.configure_workspace_clone(&workspace, &dir);
            let names = origin_tip_names(leg);
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            leg.exclude_tips = origin_tips(endpoint.as_ref(), &dir, &names)?.tips;
            leg.dest_dir = Some(dir);
        }
        Ok(())
    }

    /// To local: make sure each repo to carry has its shared clone, fetch
    /// what origin has, and pick the branch's local name.
    fn prepare_clones(&mut self, inspection: &Inspection, legs: &mut [Leg]) -> Result<(), Failure> {
        let mut chosen: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        for leg in legs.iter_mut().filter(|l| l.carry) {
            self.checkpoint(
                TransferPhase::PreparingDestination,
                Some(leg),
                Some(format!(
                    "Preparing the local clone of {}",
                    leg.source.github_repo
                )),
            )?;
            let clone = self.host.ensure_local_clone(&leg.source.github_repo)?;
            let clone_dir = path_str(&clone)?;
            let endpoint = self.host.local_endpoint(&clone)?;
            let names = origin_tip_names(leg);
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let tips = origin_tips(endpoint.as_ref(), &clone_dir, &names)?.tips;

            // Names the project's other branches hold on the same repo, or
            // that this transfer already gave out in the same clone.
            let mut taken: HashSet<String> = inspection
                .branches
                .iter()
                .filter(|b| {
                    b.branch.id != leg.source.branch.id
                        && b.branch.project_repo_id == leg.source.branch.project_repo_id
                })
                .map(|b| b.branch.branch_name.clone())
                .collect();
            taken.extend(chosen.get(&clone).into_iter().flatten().cloned());
            // Whether the name is new or reused, the apply says which (it is
            // what rolls back); the name is all that is needed here.
            let (name, _) = preflight::local_branch_name(
                &clone,
                &leg.row.branch_name,
                leg.row.head.as_deref(),
                &taken,
            )?;
            self.record(|undo| {
                if !undo.clones.contains(&clone) {
                    undo.clones.push(clone.clone());
                }
            })?;
            chosen.entry(clone).or_default().insert(name.clone());

            leg.exclude_tips = tips;
            leg.dest_branch = name;
            leg.dest_dir = Some(clone_dir);
        }
        Ok(())
    }

    fn guard_not_cancelled(&self) -> Result<(), TransferError> {
        if self.guard.is_cancelled() {
            Err(TransferError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Snapshot one branch, move it, land it — and, locally, give it its
    /// worktree and finish there.
    fn carry(&mut self, index: usize, leg: &mut Leg) -> Result<(), Failure> {
        let source_dir = leg
            .source
            .source_dir
            .clone()
            .ok_or("nothing to carry from")?;
        let dest_dir = leg.dest_dir.clone().ok_or("no destination prepared")?;
        let (source, dest, landing): (Box<dyn TransferEndpoint>, Box<dyn TransferEndpoint>, _) =
            match self.direction {
                TransferDirection::ToRemote => (
                    self.host.local_endpoint(Path::new(&source_dir))?,
                    self.host
                        .workspace_endpoint(self.workspace_name.as_deref().unwrap_or_default()),
                    Landing::Checkout,
                ),
                TransferDirection::ToLocal => (
                    self.host
                        .workspace_endpoint(leg.source.workspace.as_deref().unwrap_or_default()),
                    self.host.local_endpoint(Path::new(&dest_dir))?,
                    Landing::Branch,
                ),
            };
        let bundle_name = format!(
            "{index}-{}.bundle",
            crate::branches::repo_name_from_github_repo(&leg.source.github_repo)
        );
        let plan = CarryPlan {
            transfer_id: self.guard.transfer_id().to_string(),
            source_dir: source_dir.clone(),
            source_branch: leg.row.branch_name.clone(),
            dest_dir,
            dest_branch: leg.dest_branch.clone(),
            landing,
            include_uncommitted: self.include_uncommitted,
            exclude_tips: leg.exclude_tips.clone(),
            local_bundle: self.transfer_dir.join(&bundle_name),
            bundle_name,
            repo_bytes: leg.source.repo_bytes,
            untracked_bytes: leg.row.untracked_bytes,
        };

        self.record(|undo| {
            undo.sources.push(SourceCheckout {
                workspace: match self.direction {
                    TransferDirection::ToRemote => None,
                    TransferDirection::ToLocal => leg.source.workspace.clone(),
                },
                dir: source_dir,
            })
        })?;
        // Why `on_step` stopped the carry, when it was not a cancel.
        let mut not_recorded = None;
        let carried = carry_repo_with(source.as_ref(), dest.as_ref(), &plan, &mut |step| {
            let (phase, bytes) = match step {
                CarryStep::Snapshotting => (TransferPhase::Snapshotting, None),
                CarryStep::Moving { bytes } => (TransferPhase::Moving, Some(bytes)),
                CarryStep::Applying { .. } => (TransferPhase::Applying, None),
            };
            self.emit(phase, Some(leg), None, bytes);
            self.guard_not_cancelled()?;
            if let (CarryStep::Applying { snapshot }, Landing::Branch) = (step, plan.landing) {
                if let Err(e) = self.record_landing(&plan, snapshot) {
                    not_recorded = Some(e);
                    return Err(TransferError::Cancelled);
                }
            }
            Ok(())
        });
        let outcome = match (carried, not_recorded) {
            (Err(_), Some(e)) => return Err(e.into()),
            (carried, _) => carried?,
        };
        if self.direction == TransferDirection::ToLocal {
            self.checkpoint(
                TransferPhase::Applying,
                Some(leg),
                Some("Adding the worktree".to_string()),
            )?;
            let clone = PathBuf::from(&plan.dest_dir);
            let worktree = self.free_worktree_path(leg)?;
            self.record(|undo| {
                undo.worktrees.push(AddedWorktree {
                    clone: clone.clone(),
                    worktree: worktree.clone(),
                })
            })?;
            crate::git::create_worktree_for_existing_branch_at_path(
                &clone,
                &leg.dest_branch,
                &worktree,
            )
            .map_err(|e| format!("Cannot add the worktree for {}: {e}", leg.dest_branch))?;
            let endpoint = self.host.local_endpoint(&worktree)?;
            finish(endpoint.as_ref(), &plan, &outcome, &path_str(&worktree)?)?;
            leg.worktree = Some(worktree);
        }
        leg.plan = Some(plan);
        leg.outcome = Some(outcome);
        Ok(())
    }

    /// Before `plan`'s branch lands in a local clone: record its config as it
    /// is, and whether the apply will create the branch and which remotes it
    /// will add — exactly those missing now.
    fn record_landing(&self, plan: &CarryPlan, snapshot: &Snapshot) -> Result<(), String> {
        let clone = PathBuf::from(&plan.dest_dir);
        let branch = &plan.dest_branch;
        // A retry lands what the first attempt recorded.
        if self
            .journal
            .borrow()
            .undo
            .branch_configs
            .iter()
            .any(|c| c.clone == clone && &c.branch == branch)
        {
            return Ok(());
        }
        let cannot_read = |e: String| format!("Cannot read {branch} in {}: {e}", plan.dest_dir);
        let entries = undo::branch_config(&clone, branch).map_err(cannot_read)?;
        let created = undo::branch_tip(&clone, branch)
            .map_err(cannot_read)?
            .is_none();
        let remotes: Vec<AddedRemote> = snapshot
            .remotes
            .iter()
            .filter(|remote| undo::remote_url(&clone, &remote.name).is_none())
            .map(|remote| AddedRemote {
                clone: clone.clone(),
                remote: remote.name.clone(),
                url: remote
                    .entries
                    .iter()
                    .find(|e| e.var == "url")
                    .map(|e| e.value.clone())
                    .unwrap_or_default(),
            })
            .collect();
        self.record(|undo| {
            undo.branch_configs.push(ReplacedConfig {
                clone: clone.clone(),
                branch: branch.clone(),
                head: snapshot.head.clone(),
                entries,
            });
            if created {
                undo.branches.push(CreatedBranch {
                    clone: clone.clone(),
                    branch: branch.clone(),
                    tip: snapshot.head.clone(),
                });
            }
            undo.remotes.extend(remotes);
        })
    }

    /// Change the undo list and write it to the manifest, before making what
    /// the change describes.
    fn record(&self, change: impl FnOnce(&mut Undo)) -> Result<(), String> {
        change(&mut self.journal.borrow_mut().undo);
        self.save_journal()
    }

    fn save_journal(&self) -> Result<(), String> {
        let mut manifest = self.journal.borrow_mut();
        manifest.keep_until_removed = manifest.holds_left_behind_changes();
        backups::write_manifest(&self.transfer_dir, &manifest)?;
        self.journaled.set(true);
        Ok(())
    }

    /// Bring the manifest to `status` — with the repos as `legs` end up, and
    /// `warning` — if one was written. Past the point a failure to write it
    /// could stop anything, so that is only logged.
    pub(super) fn close_journal(
        &self,
        status: &str,
        legs: Option<&[Leg]>,
        warning: Option<String>,
    ) {
        if !self.journaled.get() {
            return;
        }
        {
            let mut manifest = self.journal.borrow_mut();
            manifest.status = status.to_string();
            if let Some(legs) = legs {
                manifest.repos = legs.iter().filter_map(|l| self.manifest_repo(l)).collect();
            }
            manifest.warnings.extend(warning);
        }
        if let Err(e) = self.save_journal() {
            log::warn!("[project_transfer] {e}");
        }
    }

    /// After a panic: one that came while a launch lock was held poisoned
    /// it, and every later session start on the branch would panic too.
    fn clear_poisoned_launch_locks(&self) {
        let Ok(branches) = self
            .store
            .list_branches_for_project(self.guard.project_id())
        else {
            return;
        };
        for branch in branches {
            crate::session_commands::branch_session_launch_lock_for(&branch.id).clear_poison();
        }
    }

    /// Roll back, report the failure, and hand back its message.
    fn fail(&self, message: String) -> String {
        self.roll_back();
        log::warn!(
            "[project_transfer] transfer {} of {} failed: {message}",
            self.guard.transfer_id(),
            self.guard.project_id()
        );
        self.emit(TransferPhase::Failed, None, Some(message.clone()), None);
        message
    }

    /// The project's usual worktree path for the branch, or the first free
    /// `-N` beside it.
    fn free_worktree_path(&self, leg: &Leg) -> Result<PathBuf, String> {
        let base = self.host.local_worktree_path(
            self.guard.project_id(),
            &leg.source.github_repo,
            &leg.dest_branch,
        )?;
        if !base.exists() {
            return Ok(base);
        }
        (2..100)
            .map(|n| PathBuf::from(format!("{}-{n}", base.display())))
            .find(|p| !p.exists())
            .ok_or_else(|| format!("No free worktree path beside {}", base.display()))
    }

    /// Switch the database over, holding every branch's launch lock.
    fn commit(&self, inspection: &Inspection, legs: &[Leg]) -> Result<(), Failure> {
        let project_id = self.guard.project_id();
        let mut branches = Vec::new();
        for leg in legs {
            let (landing, session_working_dir) = match self.direction {
                TransferDirection::ToRemote => (
                    BranchLanding::Remote {
                        workspace_name: self.workspace_name.clone().unwrap_or_default(),
                        status: if leg.carry {
                            WorkspaceStatus::Running
                        } else {
                            WorkspaceStatus::Starting
                        },
                    },
                    self.host
                        .remote_session_working_dir(&leg.source.github_repo),
                ),
                TransferDirection::ToLocal => {
                    // Without a worktree, where setup will put one.
                    let worktree = match &leg.worktree {
                        Some(path) => path.clone(),
                        None => self.host.local_worktree_path(
                            project_id,
                            &leg.source.github_repo,
                            &leg.dest_branch,
                        )?,
                    };
                    let session_dir = crate::branches::local_branch_session_working_dir(
                        self.store,
                        &inspection.project,
                        &leg.source.branch,
                        &worktree,
                    );
                    let workdir = match &leg.worktree {
                        Some(path) => Some(
                            Workdir::new(project_id, &path_str(path)?)
                                .with_branch(&leg.source.branch.id),
                        ),
                        None => None,
                    };
                    (
                        BranchLanding::Local { workdir },
                        session_dir.to_string_lossy().into_owned(),
                    )
                }
            };
            branches.push(BranchTransfer {
                branch_id: leg.source.branch.id.clone(),
                branch_name: leg.dest_branch.clone(),
                landing,
                session_working_dir,
            });
        }
        let transfer = ProjectTransfer {
            project_id: project_id.to_string(),
            location: self.direction.destination(),
            branches,
        };

        let mut ids: Vec<&str> = legs.iter().map(|l| l.source.branch.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        let locks: Vec<_> = ids
            .iter()
            .map(|id| crate::session_commands::branch_session_launch_lock_for(id))
            .collect();
        let _held: Vec<_> = locks.iter().map(|lock| lock.lock().unwrap()).collect();
        self.store.apply_project_transfer(&transfer)?;
        log::info!(
            "[project_transfer] {} switched to {} by transfer {}",
            project_id,
            self.direction.destination().as_str(),
            self.guard.transfer_id()
        );
        Ok(())
    }

    /// A carried leg, as the manifest describes it.
    fn manifest_repo(&self, leg: &Leg) -> Option<ManifestRepo> {
        let (plan, outcome) = (leg.plan.as_ref()?, leg.outcome.as_ref()?);
        Some(ManifestRepo {
            github_repo: leg.source.github_repo.clone(),
            branch: leg.row.branch_name.clone(),
            dest_branch: leg.dest_branch.clone(),
            bundle: plan.bundle_name.clone(),
            head: outcome.snapshot.head.clone(),
            snapshot: outcome.snapshot.snapshot.clone(),
            has_uncommitted_changes: outcome.snapshot.has_uncommitted_changes(),
            local_clone: self
                .host
                .local_clone_path(&leg.source.github_repo)
                .ok()
                .map(|p| p.to_string_lossy().into_owned()),
            backup_ref: leg.backup_ref.clone(),
        })
    }

    fn result(
        &self,
        inspection: &Inspection,
        legs: Vec<Leg>,
        warnings: Vec<TransferIssue>,
    ) -> ProjectTransferResult {
        let carried_any = legs.iter().any(|l| l.outcome.is_some());
        let repos = legs
            .into_iter()
            .map(|leg| {
                let uncommitted = leg
                    .outcome
                    .as_ref()
                    .is_some_and(|o| o.snapshot.has_uncommitted_changes());
                let mut warnings = leg.row.warnings;
                warnings.extend(leg.warnings);
                if let Some(outcome) = &leg.outcome {
                    warnings.extend(
                        outcome
                            .applied
                            .warnings
                            .iter()
                            .map(|w| TransferIssue::new("apply", w.clone())),
                    );
                }
                RepoTransferResult {
                    branch_id: leg.source.branch.id,
                    project_repo_id: leg.source.branch.project_repo_id,
                    github_repo: leg.source.github_repo,
                    branch_name: leg.dest_branch,
                    previous_branch_name: leg.row.branch_name,
                    carried: leg.outcome.is_some(),
                    head: leg.outcome.as_ref().map(|o| o.snapshot.head.clone()),
                    uncommitted_restored: uncommitted && self.include_uncommitted,
                    uncommitted_left_behind: uncommitted && !self.include_uncommitted,
                    retried_without_exclusions: leg
                        .outcome
                        .as_ref()
                        .is_some_and(|o| o.retried_without_exclusions),
                    backup_ref: leg.backup_ref,
                    warnings,
                }
            })
            .collect();
        ProjectTransferResult {
            project_id: self.guard.project_id().to_string(),
            transfer_id: self.guard.transfer_id().to_string(),
            direction: self.direction,
            location: self.direction.destination(),
            workspace_name: self
                .workspace_name
                .clone()
                .or_else(|| inspection.preflight.workspace_name.clone()),
            repos,
            warnings,
            backup_dir: carried_any.then(|| self.transfer_dir.to_string_lossy().into_owned()),
        }
    }
}
