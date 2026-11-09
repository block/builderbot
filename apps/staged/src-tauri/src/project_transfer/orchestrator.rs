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
//! bundles on this machine always stay, as the backup, and so do the source's
//! ignored files, archived there before it comes down. After the commit
//! nothing is undone: tearing the source down is best-effort — and refused
//! for a source that changed after its snapshot — and what it leaves comes
//! back as warnings on a successful transfer (see [`teardown`]), or on the
//! result [`TransferStopped::AfterSwitch`] carries when it panics part way.
//!
//! Synchronous, for the blocking pool; [`super::commands`] wraps it.
//!
//! The legs — one per branch, each on its own repository — are independent
//! of each other once the destination exists: the per-leg steps (cloning or
//! fetching at the destination, snapshotting, moving, applying, backing up)
//! run up to [`MAX_PARALLEL_LEGS`] at a time, legs on the same repository
//! one after another since they share a clone and its index (see
//! [`Run::for_each_leg`]). What touches shared state stays sequential: the
//! inspection, starting and deleting the workspace, the database switch,
//! and the unchanged check right before the source comes down.

mod parallel;
mod teardown;

#[cfg(test)]
pub(crate) use parallel::SerialLegs;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;

use crate::store::{
    BranchLanding, BranchTransfer, Project, ProjectLocation, ProjectTransfer, Store, Workdir,
    WorkspaceStatus,
};

use super::backups::{self, ManifestIgnored, ManifestRepo, TransferManifest};
use super::carry::{
    carry_repo_with, finish, origin_tips, run_step_sized, CarryOutcome, CarryPlan, CarryStep,
    Landing,
};
use super::endpoint::TransferEndpoint;
use super::excludes::WorktreeExcludes;
use super::host::{TransferHost, WorkspaceState};
use super::preflight::{
    self, BranchSource, Inspection, RepoPreflight, RepoTransferState, TransferDirection,
    TransferIssue,
};
use super::registry::TransferGuard;
use super::scripts::{ApplyReport, IgnoredFiles, Snapshot, CLONE_SCRIPT};
use super::undo::{
    self, AddedExcludes, AddedRemote, AddedWorktree, CreatedBranch, ReplacedConfig, SourceCheckout,
    Undo,
};
use super::TransferError;

/// How many candidate workspace names a transfer tries before giving up:
/// the derived name, then as many later ordinals.
const WORKSPACE_NAME_ATTEMPTS: u32 = 20;

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

/// What a transfer carries besides each branch's commits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransferOptions {
    /// Restore the staged, unstaged and untracked changes at the destination.
    /// Off, they stay behind, in the backup only.
    pub include_uncommitted: bool,
    /// Carry the ignored files outside ignored directories (`.env`) and
    /// place them at the destination. Off, they stay behind, in the backup.
    pub include_ignored: bool,
}

/// How a transfer that [`run_transfer`] did not see through ended.
#[derive(Debug)]
pub enum TransferStopped {
    /// Before the database switched: everything was undone, and the project
    /// is where it was.
    Failed(String),
    /// After it switched, tearing the source down stopped unexpectedly. The
    /// project is at the result's `location`: the result is what the switch
    /// made, with a warning saying what may be left, and what follows a
    /// switch still has to happen.
    AfterSwitch(Box<ProjectTransferResult>),
}

impl From<String> for TransferStopped {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

/// What refusing a transfer asked for in a direction the project no longer
/// goes says, after where the project is: the frontend knows the refusal by
/// it.
pub const STALE_DIRECTION: &str =
    "it was transferred after this dialog opened, so nothing changed.";

/// The project `guard` registered, if it is still where the transfer was
/// asked to move it from. Called under the registry entry, before anything
/// else happens: a dialog left open while another window moved the project
/// would otherwise move it straight back, deleting what that transfer made.
pub(super) fn check_direction(store: &Store, guard: &TransferGuard) -> Result<Project, String> {
    let project = store
        .get_project(guard.project_id())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", guard.project_id()))?;
    if TransferDirection::from_source(project.location) != guard.direction() {
        let there = match project.location {
            ProjectLocation::Local => "local",
            ProjectLocation::Remote => "on Blox",
        };
        return Err(format!("The project is already {there}; {STALE_DIRECTION}"));
    }
    Ok(project)
}

/// Move the project `guard` registered to the other location, in the
/// direction it was registered for; refused, with nothing done, if the
/// project is no longer at that direction's source.
pub(super) fn run_transfer(
    store: &Store,
    host: &dyn TransferHost,
    guard: &TransferGuard,
    options: TransferOptions,
    on_progress: &(dyn Fn(&TransferProgress) + Sync),
) -> Result<ProjectTransferResult, TransferStopped> {
    let TransferOptions {
        include_uncommitted,
        include_ignored,
    } = options;
    let project = check_direction(store, guard)?;
    let direction = guard.direction();
    let mut run = Run {
        store,
        host,
        guard,
        include_uncommitted,
        include_ignored,
        on_progress,
        direction,
        transfer_dir: host.transfers_dir()?.join(guard.transfer_id()),
        workspace_name: None,
        journal: Mutex::new(TransferManifest {
            transfer_id: guard.transfer_id().to_string(),
            project_id: project.id.clone(),
            direction: direction.as_str().to_string(),
            created_at: crate::store::now_timestamp(),
            include_uncommitted,
            include_ignored,
            status: backups::CARRYING.to_string(),
            owner_pid: Some(std::process::id()),
            ..TransferManifest::default()
        }),
        journaled: AtomicBool::new(false),
        switched: Mutex::new(None),
        abort: AtomicBool::new(false),
        emitting: Mutex::new(()),
    };
    match catch_unwind(AssertUnwindSafe(|| run.execute())) {
        Ok(Ok(result)) => {
            run.emit(TransferPhase::Completed, None, None, None);
            Ok(result)
        }
        // Only the steps up to the commit can fail.
        Ok(Err(Failure { message })) => Err(TransferStopped::Failed(run.fail(message))),
        Err(panic) => {
            let what = panic_message(panic.as_ref());
            run.clear_poisoned_launch_locks();
            let switched = run
                .switched
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            let Some(mut result) = switched else {
                let message = format!("The transfer stopped unexpectedly: {what}");
                return Err(TransferStopped::Failed(run.fail(message)));
            };
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
            run.emit(TransferPhase::Completed, None, Some(message.clone()), None);
            result
                .warnings
                .push(TransferIssue::new("teardown_interrupted", message));
            Err(TransferStopped::AfterSwitch(Box::new(result)))
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
#[derive(Clone)]
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
    /// The entries the finish added to the local clone's `info/exclude` for
    /// that worktree, which outlive the transfer.
    added_excludes: Option<WorktreeExcludes>,
    backup_ref: Option<String>,
    /// The source's ignored files as archived into the backup right before
    /// it came down.
    ignored_backup: Option<IgnoredFiles>,
    warnings: Vec<TransferIssue>,
}

struct Run<'a> {
    store: &'a Store,
    host: &'a dyn TransferHost,
    guard: &'a TransferGuard,
    include_uncommitted: bool,
    include_ignored: bool,
    on_progress: &'a (dyn Fn(&TransferProgress) + Sync),
    direction: TransferDirection,
    transfer_dir: PathBuf,
    workspace_name: Option<String>,
    /// The manifest as it stands, [`Undo`] list included: written before
    /// the transfer first makes anything, and again with each change. Legs
    /// running at once record into it in turn.
    journal: Mutex<TransferManifest>,
    /// Whether the manifest has been written.
    journaled: AtomicBool,
    /// Once the database has switched, the result as it switched: what a
    /// panic while the source comes down reports.
    switched: Mutex<Option<ProjectTransferResult>>,
    /// Set once a leg has failed, so the legs still running stop at their
    /// next checkpoint the way a cancel stops them.
    abort: AtomicBool,
    /// Held while a progress event is reported and delivered, so the
    /// registry and the listeners see the legs' events in one order.
    emitting: Mutex<()>,
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
        let _one_at_a_time = self.emitting.lock().unwrap_or_else(|e| e.into_inner());
        self.guard.report(&progress);
        (self.on_progress)(&progress);
    }

    /// Whether to stop: the transfer was cancelled, or another leg failed.
    fn stopping(&self) -> bool {
        self.guard.is_cancelled() || self.abort.load(Ordering::SeqCst)
    }

    /// Report `phase`, then stop here if the transfer was cancelled.
    fn checkpoint(
        &self,
        phase: TransferPhase,
        leg: Option<&Leg>,
        detail: Option<String>,
    ) -> Result<(), TransferError> {
        self.emit(phase, leg, detail, None);
        if self.stopping() {
            Err(TransferError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn execute(&mut self) -> Result<ProjectTransferResult, Failure> {
        let started = Instant::now();
        let timed = |what: &str, since: Instant| {
            log::info!(
                "[project_transfer] transfer {}: {what} took {:.1?}",
                self.guard.transfer_id(),
                since.elapsed()
            );
        };
        self.checkpoint(TransferPhase::Checking, None, None)?;
        let inspection = self.inspect()?;
        timed("the inspection", started);
        self.guard
            .cover_branches(inspection.branches.iter().map(|b| b.branch.id.as_str()));
        self.guard.cover_workspaces(
            inspection
                .branches
                .iter()
                .filter_map(|b| b.workspace.as_deref()),
        );
        self.check_sessions(&inspection)?;
        self.journal.lock().unwrap().workspace_name = inspection.preflight.workspace_name.clone();

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
                added_excludes: None,
                backup_ref: None,
                ignored_backup: None,
                warnings: Vec::new(),
            })
            .collect();

        let preparing = Instant::now();
        match self.direction {
            TransferDirection::ToRemote => self.prepare_workspace(&inspection, &mut legs)?,
            TransferDirection::ToLocal => self.prepare_clones(&inspection, &mut legs)?,
        }
        timed("preparing the destination", preparing);

        // Each leg snapshots, moves and lands on its own; the legs run at
        // once, and the first to fail stops the rest.
        let carrying = Instant::now();
        self.for_each_leg(
            &mut legs,
            |l| l.carry,
            |i, leg| {
                self.carry(i, leg)?;
                let repo = self.manifest_repo(leg);
                let mut journal = self.journal.lock().unwrap();
                journal.repos.extend(repo);
                self.save_journal_locked(&mut journal)?;
                Ok(())
            },
        )?;
        timed("carrying the branches", carrying);

        self.checkpoint(TransferPhase::Committing, None, None)?;
        // Made first, so that nothing between the switch and noting it can
        // panic.
        let switched = self.result(&inspection, legs.clone(), Vec::new());
        self.commit(&inspection, &legs)?;
        *self.switched.lock().unwrap() = Some(switched);
        self.close_journal(backups::COMMITTED, None, None);
        // The rows describe the destination now, and setup may provision them
        // while the source comes down.
        self.guard.release_branches();

        let tearing_down = Instant::now();
        let warnings = self.tear_down(&inspection, &mut legs);
        timed("removing the source", tearing_down);
        self.close_journal(backups::COMPLETED, Some(&legs), None);
        timed("the whole transfer", started);
        Ok(self.result(&inspection, legs, warnings))
    }

    /// The preflight, again, now that the transfer is registered — with any
    /// suspended, stopped or failed source workspace resumed first, so its
    /// checkouts can be read. One that won't come back blocks the transfer.
    fn inspect(&self) -> Result<Inspection, Failure> {
        let project_id = self.guard.project_id();
        let cancelled = || self.guard.is_cancelled();
        let observer = preflight::PreflightObserver {
            on_partial: &|_| {},
            cancelled: &cancelled,
        };
        let mut inspection = preflight::inspect_after_resuming(
            self.store,
            self.host,
            project_id,
            &BTreeMap::new(),
            &observer,
        )?;
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
            inspection = preflight::inspect_after_resuming(
                self.store, self.host, project_id, &resumed, &observer,
            )?;
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
        // Asked to carry ignored files it can't: stop, rather than quietly
        // leave them behind.
        if self.include_ignored {
            let too_large: Vec<&str> = preflight
                .repos
                .iter()
                .flat_map(|r| &r.warnings)
                .filter(|w| w.code == preflight::IGNORED_TOO_LARGE)
                .map(|w| w.message.as_str())
                .collect();
            if !too_large.is_empty() {
                return Err(format!(
                    "Can't transfer the ignored files: {} Transfer without them, or delete \
                     some first.",
                    too_large.join(" ")
                )
                .into());
            }
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
            // Queued sessions wait for the transfer (every start on a
            // covered branch queues) and are kicked once it is over.
            let running = self
                .store
                .has_running_session_for_branch(&source.branch.id)?;
            if running {
                return Err(format!(
                    "A session started on {}; stop it before transferring.",
                    source.branch.branch_name
                )
                .into());
            }
        }
        Ok(())
    }

    /// A free workspace name, from the primary branch's: the derived name,
    /// or when that is taken the next ordinal's, which keeps the
    /// `{readable}-{hash}` shape rather than appending `-2` to a name that
    /// may already be at the length limit.
    fn pick_workspace_name(&self, project_id: &str, seed: &str) -> Result<String, Failure> {
        use crate::branches::infer_workspace_name_nth;
        for ordinal in 0..WORKSPACE_NAME_ATTEMPTS {
            let name = infer_workspace_name_nth(project_id, seed, ordinal);
            if self.host.workspace_state(&name)? == WorkspaceState::NotFound {
                return Ok(name);
            }
        }
        let base = infer_workspace_name_nth(project_id, seed, 0);
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
        let workspace = self.pick_workspace_name(&inspection.project.id, &seed)?;
        self.workspace_name = Some(workspace.clone());
        let Some(primary) = primary else {
            return Ok(());
        };

        // Recorded before the start: a start that fails half way can still
        // have created it.
        self.journal.lock().unwrap().workspace_name = Some(workspace.clone());
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

        // Each repository clones into its own directory: the clones, mostly
        // waiting on the network, go at once.
        let workspace = &workspace;
        self.for_each_leg(
            legs,
            |l| l.carry,
            |_, leg| {
                self.checkpoint(
                    TransferPhase::PreparingDestination,
                    Some(leg),
                    Some(format!("Cloning {}", leg.source.github_repo)),
                )?;
                let endpoint = self.host.workspace_endpoint(workspace);
                let dir = self
                    .host
                    .workspace_repo_dir(workspace, &leg.source.github_repo)?;
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
                self.host.configure_workspace_clone(workspace, &dir);
                let names = origin_tip_names(leg);
                let names: Vec<&str> = names.iter().map(String::as_str).collect();
                leg.exclude_tips = origin_tips(endpoint.as_ref(), &dir, &names)?.tips;
                leg.dest_dir = Some(dir);
                Ok(())
            },
        )
    }

    /// To local: make sure each repo to carry has its shared clone, fetch
    /// what origin has, and pick the branch's local name.
    fn prepare_clones(&mut self, inspection: &Inspection, legs: &mut [Leg]) -> Result<(), Failure> {
        // Each repository's clone is made and fetched on its own — the slow,
        // network-bound part — at once with the others'.
        self.for_each_leg(
            legs,
            |l| l.carry,
            |_, leg| {
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
                leg.exclude_tips = origin_tips(endpoint.as_ref(), &clone_dir, &names)?.tips;
                leg.dest_dir = Some(clone_dir);
                Ok(())
            },
        )?;

        // Then the names, one leg after another: each depends on what the
        // legs before it on the same clone were given.
        let mut chosen: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        for leg in legs.iter_mut().filter(|l| l.carry) {
            let clone = PathBuf::from(leg.dest_dir.as_deref().ok_or("no clone prepared")?);

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
            leg.dest_branch = name;
        }
        Ok(())
    }

    fn guard_not_cancelled(&self) -> Result<(), TransferError> {
        if self.stopping() {
            Err(TransferError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Snapshot one branch, move it, land it — and, locally, give it its
    /// worktree and finish there.
    fn carry(&self, index: usize, leg: &mut Leg) -> Result<(), Failure> {
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
            origin_branches: origin_tip_names(leg),
            local_bundle: self.transfer_dir.join(&bundle_name),
            bundle_name,
            repo_bytes: leg.source.repo_bytes,
            untracked_bytes: leg.row.untracked_bytes,
            lfs_bytes: leg.row.lfs_present_bytes,
            include_ignored: self.include_ignored && leg.row.ignored_files > 0,
            ignored_cap: self.host.ignored_files_cap(),
            ignored_bytes: leg.row.ignored_bytes,
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
        let mut outcome = match (carried, not_recorded) {
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
            // LFS files check out as pointers, which `finish` replaces with
            // the objects that came: a smudge filter would download each one
            // again, and fail the checkout on any it can't.
            let add_worktree = if outcome.snapshot.lfs.is_some() {
                crate::git::create_worktree_for_existing_branch_without_lfs_smudge
            } else {
                crate::git::create_worktree_for_existing_branch_at_path
            };
            add_worktree(&clone, &leg.dest_branch, &worktree)
                .map_err(|e| format!("Cannot add the worktree for {}: {e}", leg.dest_branch))?;
            let endpoint = self.host.local_endpoint(&worktree)?;
            let (finished, added) = self.finish_recording_excludes(
                endpoint.as_ref(),
                &plan,
                &outcome,
                &path_str(&worktree)?,
            )?;
            outcome.applied.warnings.extend(finished.warnings);
            outcome.applied.lfs_unavailable |= finished.lfs_unavailable;
            outcome.applied.ignored = outcome.applied.ignored.take().or(finished.ignored);
            leg.added_excludes = added.and_then(|a| WorktreeExcludes::new(worktree.clone(), &a));
            leg.worktree = Some(worktree);
        }
        leg.plan = Some(plan);
        leg.outcome = Some(outcome);
        Ok(())
    }

    /// [`finish`] in the new worktree. When the ignored files came with an
    /// excludes list, the entries it may add to the clone's `info/exclude` —
    /// which outlives the worktree, and every other worktree of the clone
    /// reads — are recorded first, by the journal the finish notes them in,
    /// and then, whatever the finish did, as the journal has them. Returns
    /// them with the report, for the manifest to keep once the transfer is
    /// over.
    fn finish_recording_excludes(
        &self,
        endpoint: &dyn TransferEndpoint,
        plan: &CarryPlan,
        outcome: &CarryOutcome,
        worktree: &str,
    ) -> Result<(ApplyReport, Option<AddedExcludes>), Failure> {
        if outcome.dest_ignored_excludes.is_none() {
            return Ok((finish(endpoint, plan, outcome, worktree)?, None));
        }
        let journal = plan.local_excludes_journal();
        self.record(|undo| undo.excludes.push(AddedExcludes::noted_in(journal.clone())))?;
        let finished = finish(endpoint, plan, outcome, worktree);
        let added = match AddedExcludes::read_journal(&journal) {
            Ok(Some(added)) => {
                let recorded = self.record(|undo| {
                    if let Some(entry) = undo.excludes.iter_mut().find(|e| e.journal == journal) {
                        *entry = added.clone();
                    }
                });
                // The entry still names the journal, which rollback reads.
                if let Err(e) = recorded {
                    log::warn!("[project_transfer] {e}");
                }
                Some(added)
            }
            Ok(None) => None,
            Err(e) => {
                log::warn!("[project_transfer] {e}");
                None
            }
        };
        Ok((finished?, added))
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
            .lock()
            .unwrap()
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
        let mut journal = self.journal.lock().unwrap();
        change(&mut journal.undo);
        self.save_journal_locked(&mut journal)
    }

    /// Write the manifest as `manifest` has it, under its lock so that two
    /// legs recording at once each write a manifest holding both records.
    fn save_journal_locked(&self, manifest: &mut TransferManifest) -> Result<(), String> {
        manifest.keep_until_removed = manifest.holds_left_behind_changes();
        backups::write_manifest(&self.transfer_dir, manifest)?;
        self.journaled.store(true, Ordering::SeqCst);
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
        if !self.journaled.load(Ordering::SeqCst) {
            return;
        }
        let mut manifest = self.journal.lock().unwrap_or_else(|e| e.into_inner());
        manifest.status = status.to_string();
        if let Some(legs) = legs {
            manifest.repos = legs.iter().filter_map(|l| self.manifest_repo(l)).collect();
        }
        manifest.warnings.extend(warning);
        if let Err(e) = self.save_journal_locked(&mut manifest) {
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

    /// Roll back, report the failure, and hand back its message — with what
    /// was queued for the transfer to finish, so the user knows it goes
    /// ahead where the project still is. The command sets it going
    /// (`session_queue::release_held_work`) once the guard has dropped,
    /// which is what the queued work waits on; it can't happen here, under
    /// the guard.
    fn fail(&self, message: String) -> String {
        self.roll_back();
        let message =
            match crate::session_queue::held_work_note(self.store, self.guard.project_id()) {
                Some(note) => format!("{message} {note}"),
                None => message,
            };
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
            ignored: manifest_ignored(plan, outcome, leg.ignored_backup.as_ref()),
            added_excludes: leg.added_excludes.clone(),
        })
    }

    /// The ignored files the destination is without, after an apply that
    /// placed them.
    fn ignored_warnings(&self, applied: &ApplyReport) -> Vec<TransferIssue> {
        let Some(placement) = &applied.ignored else {
            return Vec::new();
        };
        let there = match self.direction {
            TransferDirection::ToRemote => "the workspace",
            TransferDirection::ToLocal => "the new worktree",
        };
        // "2 ignored files weren't put in …, which doesn't ignore them (a.log,
        // for one); they're in the backup."
        let not_placed = |n: u64, why: &str, example: &Option<String>| {
            let (files, were, them, they) = match n {
                1 => ("1 ignored file".to_string(), "wasn't", "it", "it's"),
                n => (format!("{n} ignored files"), "weren't", "them", "they're"),
            };
            let why = why.replace("{them}", them);
            let example = example.as_deref().unwrap_or("?");
            let example = if n == 1 {
                example.to_string()
            } else {
                format!("{example}, for one")
            };
            TransferIssue::new(
                "ignored_not_placed",
                format!("{files} {were} put in {there}, {why} ({example}); {they} in the backup."),
            )
        };
        let mut warnings = Vec::new();
        if placement.unignored > 0 {
            warnings.push(not_placed(
                placement.unignored,
                "which doesn't ignore {them}",
                &placement.unignored_example,
            ));
        }
        if placement.in_the_way > 0 {
            warnings.push(not_placed(
                placement.in_the_way,
                "where something was already in the way",
                &placement.in_the_way_example,
            ));
        }
        if placement.elsewhere > 0 {
            warnings.push(not_placed(
                placement.elsewhere,
                "as ignoring {them} would also hide what another worktree has in the same place",
                &placement.elsewhere_example,
            ));
        }
        warnings
    }

    /// What the destination is still without, LFS-wise, after an apply.
    fn lfs_warnings(&self, applied: &ApplyReport) -> Vec<TransferIssue> {
        let here = match self.direction {
            TransferDirection::ToRemote => "in the workspace",
            TransferDirection::ToLocal => "on this machine",
        };
        let mut warnings = Vec::new();
        if applied.lfs_unavailable {
            warnings.push(TransferIssue::new(
                "lfs_unavailable",
                format!(
                    "Git LFS isn't installed {here}, so LFS files hold pointers; install \
                     git-lfs, then run git lfs checkout."
                ),
            ));
        }
        if applied.lfs_missing > 0 {
            warnings.push(TransferIssue::new(
                "lfs_not_fetched",
                format!(
                    "{} {} the source didn't have couldn't be fetched from origin, so the \
                     files that use them hold pointers until git lfs pull fetches them.",
                    applied.lfs_missing,
                    if applied.lfs_missing == 1 {
                        "LFS object"
                    } else {
                        "LFS objects"
                    }
                ),
            ));
        }
        warnings
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
                    warnings.extend(self.lfs_warnings(&outcome.applied));
                    warnings.extend(self.ignored_warnings(&outcome.applied));
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

/// What the manifest says of a leg's ignored files: the backup taken as the
/// source came down, or without one (the source stayed, or the transfer was
/// cut off) what the carry archived. `None` with none to speak of.
fn manifest_ignored(
    plan: &CarryPlan,
    outcome: &CarryOutcome,
    backup: Option<&IgnoredFiles>,
) -> Option<ManifestIgnored> {
    let records = || [backup, outcome.ignored.as_ref()].into_iter().flatten();
    // The backup's count, unless it found none left (deleted since the carry).
    let archived = records().find(|i| i.files > 0)?;
    // A carry's archive stays even when the backup found too many to take.
    let kept = records().any(|i| i.archive.is_some());
    Some(ManifestIgnored {
        archive: kept.then(|| plan.ignored_name()),
        files: archived.files,
        bytes: archived.bytes,
        over_cap: archived.over_cap,
        placed: outcome.applied.ignored.as_ref().map_or(0, |p| p.placed),
    })
}
