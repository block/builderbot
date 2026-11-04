//! Moving a whole project: prepare the destination, carry each branch,
//! switch the database, tear the source down.
//!
//! Until [`Store::apply_project_transfer`] commits, the source and the
//! database are untouched and every step is undone on failure (or on
//! cancel, checked between steps): the workspace or the worktrees, branches
//! and remotes the transfer created are removed, and the temporary refs at
//! both ends. The bundles on this machine always stay, as the backup. After
//! the commit nothing is undone: tearing the source down is best-effort, and
//! what fails there comes back as warnings on a successful transfer.
//!
//! Synchronous, for the blocking pool; [`super::commands`] wraps it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::store::{
    BranchLanding, BranchTransfer, ProjectLocation, ProjectTransfer, Store, Workdir,
    WorkspaceStatus,
};

use super::backups::{self, ManifestRepo, TransferManifest};
use super::carry::{
    carry_repo_with, cleanup, finish, origin_tips, run_step, CarryOutcome, CarryPlan, CarryStep,
    Landing,
};
use super::endpoint::TransferEndpoint;
use super::host::{TransferHost, WorkspaceState};
use super::preflight::{
    self, BranchSource, Inspection, RepoPreflight, RepoTransferState, TransferDirection,
    TransferIssue,
};
use super::registry::TransferGuard;
use super::scripts::CLONE_SCRIPT;
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
    /// The ref holding the source's snapshot in the local clone, for a
    /// transfer to remote.
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
    let mut run = Run {
        store,
        host,
        guard,
        include_uncommitted,
        on_progress,
        direction: TransferDirection::from_source(project.location),
        transfer_dir: host.transfers_dir()?.join(guard.transfer_id()),
        workspace_name: None,
        undo: Undo::default(),
    };
    match run.execute() {
        Ok(result) => {
            run.emit(TransferPhase::Completed, None, None, None);
            Ok(result)
        }
        // Only the steps up to the commit can fail.
        Err(Failure { message }) => {
            run.roll_back();
            log::warn!(
                "[project_transfer] transfer {} of {} failed: {message}",
                guard.transfer_id(),
                guard.project_id()
            );
            run.emit(TransferPhase::Failed, None, Some(message.clone()), None);
            Err(message)
        }
    }
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

/// Everything the transfer made that a rollback removes.
#[derive(Default)]
struct Undo {
    /// The workspace created for a transfer to remote.
    workspace: Option<String>,
    /// Checkouts snapshotted at the source: their temporary refs, and a
    /// workspace's scratch directory.
    sources: Vec<SourceCheckout>,
    /// Local clones a transfer to local fetched bundles into.
    clones: Vec<PathBuf>,
    /// Local branches it created, `(clone, branch)`.
    branches: Vec<(PathBuf, String)>,
    /// Worktrees it added, `(clone, worktree)`.
    worktrees: Vec<(PathBuf, PathBuf)>,
    /// Remotes the apply added to a local clone, `(clone, remote)`.
    remotes: Vec<(PathBuf, String)>,
}

struct SourceCheckout {
    /// `None`: this machine.
    workspace: Option<String>,
    dir: String,
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
    undo: Undo,
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
        (self.on_progress)(&TransferProgress {
            project_id: self.guard.project_id().to_string(),
            transfer_id: self.guard.transfer_id().to_string(),
            direction: self.direction,
            phase,
            branch_id: leg.map(|l| l.source.branch.id.clone()),
            project_repo_id: leg.and_then(|l| l.source.branch.project_repo_id.clone()),
            detail,
            bytes,
        });
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
        self.check_sessions(&inspection)?;

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
            }
        }
        if legs.iter().any(|l| l.outcome.is_some()) {
            backups::write_manifest(
                &self.transfer_dir,
                &self.manifest(&inspection, &legs, "carrying"),
            )?;
        }

        self.checkpoint(TransferPhase::Committing, None, None)?;
        self.commit(&inspection, &legs)?;

        let warnings = self.tear_down(&inspection, &mut legs);
        if legs.iter().any(|l| l.outcome.is_some()) {
            if let Err(e) = backups::write_manifest(
                &self.transfer_dir,
                &self.manifest(&inspection, &legs, "completed"),
            ) {
                log::warn!("[project_transfer] {e}");
            }
        }
        Ok(self.result(&inspection, legs, warnings))
    }

    /// The preflight, again, now that the transfer is registered — with any
    /// suspended source workspace resumed first, so its checkouts can be read.
    fn inspect(&self) -> Result<Inspection, Failure> {
        let mut inspection = preflight::inspect(self.store, self.host, self.guard.project_id())?;
        let suspended: BTreeSet<String> = inspection
            .branches
            .iter()
            .zip(&inspection.preflight.repos)
            .filter(|(_, row)| row.state == RepoTransferState::Unknown)
            .filter_map(|(source, _)| source.workspace.clone())
            .collect();
        if !suspended.is_empty() {
            for workspace in &suspended {
                self.checkpoint(
                    TransferPhase::Checking,
                    None,
                    Some(format!("Resuming workspace {workspace}")),
                )?;
                self.host.resume_workspace(workspace)?;
            }
            inspection = preflight::inspect(self.store, self.host, self.guard.project_id())?;
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
        self.undo.workspace = Some(workspace.clone());
        let base = crate::branches::normalize_branch_ref(&legs[primary].source.branch.base_branch);
        self.host.start_workspace(
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
        )?;
        self.guard_not_cancelled()?;

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
            run_step(
                endpoint.as_ref(),
                "clone",
                CLONE_SCRIPT,
                &[
                    dir.clone(),
                    self.host.workspace_clone_url(&leg.source.github_repo),
                ],
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
            let (name, created) = preflight::local_branch_name(
                &clone,
                &leg.row.branch_name,
                leg.row.head.as_deref(),
                &taken,
            )?;
            if created {
                self.undo.branches.push((clone.clone(), name.clone()));
            }
            if !self.undo.clones.contains(&clone) {
                self.undo.clones.push(clone.clone());
            }
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
        };

        self.undo.sources.push(SourceCheckout {
            workspace: match self.direction {
                TransferDirection::ToRemote => None,
                TransferDirection::ToLocal => leg.source.workspace.clone(),
            },
            dir: source_dir,
        });
        let outcome = carry_repo_with(source.as_ref(), dest.as_ref(), &plan, &mut |step| {
            let (phase, bytes) = match step {
                CarryStep::Snapshotting => (TransferPhase::Snapshotting, None),
                CarryStep::Moving { bytes } => (TransferPhase::Moving, Some(bytes)),
                CarryStep::Applying => (TransferPhase::Applying, None),
            };
            self.emit(phase, Some(leg), None, bytes);
            self.guard_not_cancelled()
        })?;
        if self.direction == TransferDirection::ToLocal {
            for remote in &outcome.applied.added_remotes {
                self.undo
                    .remotes
                    .push((PathBuf::from(&plan.dest_dir), remote.clone()));
            }

            self.checkpoint(
                TransferPhase::Applying,
                Some(leg),
                Some("Adding the worktree".to_string()),
            )?;
            let clone = PathBuf::from(&plan.dest_dir);
            let worktree = self.free_worktree_path(leg)?;
            self.undo.worktrees.push((clone.clone(), worktree.clone()));
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

    /// Remove the source: the worktrees (after keeping each snapshot under
    /// a backup ref) or the workspace. Never fails; returns what went wrong.
    fn tear_down(&self, inspection: &Inspection, legs: &mut [Leg]) -> Vec<TransferIssue> {
        self.emit(TransferPhase::TearingDown, None, None, None);
        let mut warnings = Vec::new();
        let tid = self.guard.transfer_id();
        match self.direction {
            TransferDirection::ToRemote => {
                for leg in legs.iter_mut() {
                    let (Some(outcome), Some(worktree)) = (&leg.outcome, &leg.source.source_dir)
                    else {
                        continue;
                    };
                    let worktree_path = PathBuf::from(worktree);
                    let removed = self.remove_local_source(leg, outcome, &worktree_path);
                    match removed {
                        Ok(backup_ref) => leg.backup_ref = Some(backup_ref),
                        Err(e) => warnings.push(TransferIssue::new(
                            "worktree_left",
                            format!(
                                "Transferred, but couldn't remove the old worktree {worktree}: {e}"
                            ),
                        )),
                    }
                }
                // The bundles at the destination have served their purpose.
                if let (Some(workspace), Some(dir)) = (
                    self.workspace_name.as_deref(),
                    legs.iter().find_map(|l| l.dest_dir.as_deref()),
                ) {
                    let endpoint = self.host.workspace_endpoint(workspace);
                    if let Err(e) = cleanup(endpoint.as_ref(), dir, tid, true) {
                        log::warn!("[project_transfer] cannot clear {workspace}'s scratch: {e}");
                    }
                }
            }
            TransferDirection::ToLocal => {
                for workspace in &inspection.running_workspaces {
                    if let Err(e) = self.host.delete_workspace(workspace) {
                        warnings.push(TransferIssue::new(
                            "workspace_left",
                            format!("Transferred, but couldn't delete workspace {workspace}: {e}"),
                        ));
                        let endpoint = self.host.workspace_endpoint(workspace);
                        for leg in legs
                            .iter()
                            .filter(|l| l.source.workspace.as_deref() == Some(workspace))
                        {
                            if let Some(dir) = &leg.source.source_dir {
                                let _ = cleanup(endpoint.as_ref(), dir, tid, true);
                            }
                        }
                    }
                }
            }
        }
        warnings
    }

    /// Keep the snapshot under a backup ref in the shared clone, drop the
    /// temporary ref, and remove the worktree (and its branch).
    fn remove_local_source(
        &self,
        leg: &Leg,
        outcome: &CarryOutcome,
        worktree: &Path,
    ) -> Result<String, String> {
        let clone = self.host.local_clone_path(&leg.source.github_repo)?;
        if clone.canonicalize().ok() == worktree.canonicalize().ok() {
            return Err("it is the shared clone itself".to_string());
        }
        let backup_ref = backups::backup_ref(self.guard.transfer_id(), &leg.row.branch_name);
        crate::git::cli_run(
            &clone,
            &["update-ref", &backup_ref, &outcome.snapshot.snapshot],
        )
        .map_err(|e| format!("cannot keep the backup ref: {e}"))?;
        let endpoint = self.host.local_endpoint(worktree)?;
        if let Err(e) = cleanup(
            endpoint.as_ref(),
            &path_str(worktree)?,
            self.guard.transfer_id(),
            false,
        ) {
            log::warn!("[project_transfer] cannot drop the temporary ref: {e}");
        }
        crate::git::remove_worktree(&clone, worktree).map_err(|e| e.to_string())?;
        Ok(backup_ref)
    }

    /// Undo everything in [`Undo`], best-effort: destination first, then the
    /// source's temporary refs. The bundles on this machine stay.
    fn roll_back(&self) {
        self.emit(TransferPhase::RollingBack, None, None, None);
        let tid = self.guard.transfer_id();
        if let Some(workspace) = &self.undo.workspace {
            if let Err(e) = self.host.delete_workspace(workspace) {
                log::warn!("[project_transfer] rollback: cannot delete workspace {workspace}: {e}");
            }
        }
        for (clone, worktree) in self.undo.worktrees.iter().rev() {
            remove_worktree_keeping_branch(clone, worktree);
        }
        for (clone, branch) in &self.undo.branches {
            if crate::git::branch_exists(clone, branch).unwrap_or(false) {
                if let Err(e) = crate::git::cli_run(clone, &["branch", "-D", branch]) {
                    log::warn!("[project_transfer] rollback: cannot delete {branch}: {e}");
                }
            }
        }
        for (clone, remote) in &self.undo.remotes {
            let _ = crate::git::cli_run(clone, &["remote", "remove", remote]);
        }
        for clone in &self.undo.clones {
            let result = self
                .host
                .local_endpoint(clone)
                .map_err(|e| e.to_string())
                .and_then(|endpoint| {
                    cleanup(endpoint.as_ref(), &path_str(clone)?, tid, false)
                        .map_err(|e| e.to_string())
                });
            if let Err(e) = result {
                log::warn!("[project_transfer] rollback: {e}");
            }
        }
        for source in &self.undo.sources {
            let result = match &source.workspace {
                Some(workspace) => cleanup(
                    self.host.workspace_endpoint(workspace).as_ref(),
                    &source.dir,
                    tid,
                    true,
                )
                .map_err(|e| e.to_string()),
                None => self
                    .host
                    .local_endpoint(Path::new(&source.dir))
                    .and_then(|endpoint| {
                        cleanup(endpoint.as_ref(), &source.dir, tid, false)
                            .map_err(|e| e.to_string())
                    }),
            };
            if let Err(e) = result {
                log::warn!("[project_transfer] rollback: {e}");
            }
        }
        if self.transfer_dir.join(backups::MANIFEST_FILE).exists() {
            if let Some(mut manifest) = backups::read_manifest(&self.transfer_dir) {
                manifest.status = "rolledBack".to_string();
                let _ = backups::write_manifest(&self.transfer_dir, &manifest);
            }
        }
    }

    fn manifest(&self, inspection: &Inspection, legs: &[Leg], status: &str) -> TransferManifest {
        let repos = legs
            .iter()
            .filter_map(|leg| {
                let (plan, outcome) = (leg.plan.as_ref()?, leg.outcome.as_ref()?);
                Some(ManifestRepo {
                    github_repo: leg.source.github_repo.clone(),
                    branch: leg.row.branch_name.clone(),
                    dest_branch: leg.dest_branch.clone(),
                    bundle: plan.bundle_name.clone(),
                    head: outcome.snapshot.head.clone(),
                    snapshot: outcome.snapshot.snapshot.clone(),
                    has_uncommitted_changes: outcome.snapshot.has_uncommitted_changes(),
                    local_clone: leg.backup_ref.as_ref().and_then(|_| {
                        self.host
                            .local_clone_path(&leg.source.github_repo)
                            .ok()
                            .map(|p| p.to_string_lossy().into_owned())
                    }),
                    backup_ref: leg.backup_ref.clone(),
                })
            })
            .collect();
        TransferManifest {
            transfer_id: self.guard.transfer_id().to_string(),
            project_id: inspection.project.id.clone(),
            direction: self.direction.as_str().to_string(),
            created_at: crate::store::now_timestamp(),
            include_uncommitted: self.include_uncommitted,
            status: status.to_string(),
            workspace_name: self
                .workspace_name
                .clone()
                .or_else(|| inspection.preflight.workspace_name.clone()),
            repos,
        }
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

/// Remove a worktree the transfer added, without `remove_worktree`'s branch
/// deletion: the branch is the rollback's to decide about.
fn remove_worktree_keeping_branch(clone: &Path, worktree: &Path) {
    let Some(path) = worktree.to_str() else {
        return;
    };
    if crate::git::cli_run(clone, &["worktree", "remove", "--force", path]).is_err() {
        if worktree.exists() {
            if let Err(e) = std::fs::remove_dir_all(worktree) {
                log::warn!("[project_transfer] rollback: cannot remove {path}: {e}");
            }
        }
        let _ = crate::git::cli_run(clone, &["worktree", "prune"]);
    }
}
