//! What a transfer would carry, and what stops it. Read-only: the checkouts
//! are inspected with [`PREFLIGHT_SCRIPT`], and nothing is resumed, fetched
//! or cloned.
//!
//! The checks come in three steps, each slower than the last. First the
//! store alone: the project, its repos and branches, their sessions, and
//! which of them name a worktree or a workspace there may be a checkout to
//! read in — nothing leaves this machine, so it takes milliseconds. Then
//! Blox: whether it can be reached, and the state of each source workspace
//! (one `sq blox` round trip each, about a second). Then every checkout is
//! read, in up to [`MAX_PARALLEL_SCANS`] runs of the script at once, each
//! checkout in two parts ([`PreflightPart`]) that run apart. A
//! [`PreflightObserver`] sees the preflight after each of the first two
//! steps and again as each part lands, with the rows still to be read marked
//! `scanning`, so the dialog can show every row from the first step — and
//! can ask for the reading to stop.
//!
//! The orchestrator runs the same [`inspect`] once the transfer is
//! registered, so the dialog and the transfer agree on what each branch
//! carries; the snapshot script re-checks the repo-level blockers as it
//! snapshots.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};

use serde::{Deserialize, Serialize};

use crate::store::{
    Branch, BranchType, Project, ProjectLocation, ProjectRepo, Store, WorkspaceStatus,
};

use super::host::{TransferHost, WorkspaceState};
use super::scripts::{self, LfsCounts, PreflightPart, ScriptReport, PREFLIGHT_SCRIPT};
use super::TransferError;

/// How many runs of [`PREFLIGHT_SCRIPT`] go at once: enough to overlap the
/// round trips to a workspace without flooding it.
const MAX_PARALLEL_SCANS: usize = 4;

/// Untracked content past this size gets a warning: it is carried in full.
const LARGE_PAYLOAD_BYTES: u64 = 100 * 1024 * 1024;

/// A row's warning that its ignored files are over the cap, which stops a
/// transfer asked to carry them.
pub(super) const IGNORED_TOO_LARGE: &str = "ignored_too_large";

/// A row's warning that its ignored files couldn't be listed.
const IGNORED_UNLISTED: &str = "ignored_unlisted";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferDirection {
    ToRemote,
    ToLocal,
}

impl TransferDirection {
    pub fn from_source(source: ProjectLocation) -> Self {
        match source {
            ProjectLocation::Local => Self::ToRemote,
            ProjectLocation::Remote => Self::ToLocal,
        }
    }

    pub fn source(self) -> ProjectLocation {
        match self {
            Self::ToRemote => ProjectLocation::Local,
            Self::ToLocal => ProjectLocation::Remote,
        }
    }

    pub fn destination(self) -> ProjectLocation {
        match self {
            Self::ToRemote => ProjectLocation::Remote,
            Self::ToLocal => ProjectLocation::Local,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToRemote => "toRemote",
            Self::ToLocal => "toLocal",
        }
    }
}

/// A blocker or warning: a stable `code` for the UI, and a sentence for the
/// user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferIssue {
    pub code: String,
    pub message: String,
}

impl TransferIssue {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// What a branch takes to the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepoTransferState {
    /// A checkout to snapshot and carry.
    Carry,
    /// No checkout (no worktree, no workspace, no clone there): the branch
    /// starts from origin at the destination.
    NothingToCarry,
    /// Not known until the transfer resumes the workspace (suspended,
    /// stopped or failed).
    Unknown,
}

/// One branch, on one of the project's repos.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoPreflight {
    pub branch_id: String,
    pub project_repo_id: Option<String>,
    pub github_repo: String,
    pub subpath: Option<String>,
    pub branch_name: String,
    /// The name it lands under: different when the local clone already uses
    /// `branch_name` for something else.
    pub dest_branch_name: String,
    pub state: RepoTransferState,
    pub head: Option<String>,
    /// The upstream as `git` abbreviates it, e.g. `origin/feature`.
    pub upstream: Option<String>,
    /// Commits on no remote-tracking ref.
    pub unpushed_commits: u32,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub untracked_bytes: u64,
    /// Ignored regular files outside ignored directories, each counted —
    /// including every file of an untracked directory that holds nothing
    /// else. The transfer backs them up before it removes the source, and
    /// carries them when asked to, unless together they are over the cap
    /// (an `ignored_too_large` warning).
    pub ignored_files: u32,
    pub ignored_bytes: u64,
    /// Directories an ignore pattern matches (`node_modules/`, `.venv/`),
    /// each counted once however much they hold: never carried or backed
    /// up, and deleted with the source.
    pub ignored_dirs: u32,
    /// LFS objects the branch refers to that the source has, and will carry
    /// beside the bundle; counting the modified and untracked files the LFS
    /// filter applies to, which the snapshot stores as new objects.
    pub lfs_present: u64,
    pub lfs_present_bytes: u64,
    /// LFS objects in `HEAD` or the index that the source never downloaded:
    /// the destination fetches the ones its checkout uses from origin if it
    /// can.
    pub lfs_missing: u64,
    pub lfs_missing_bytes: u64,
    pub has_uncommitted_changes: bool,
    pub blockers: Vec<TransferIssue>,
    pub warnings: Vec<TransferIssue>,
    /// The checkout is still being read: the counts above are what has been
    /// read so far (nothing, or the status part without the ignored files
    /// and LFS objects). Only ever `true` in a partial result.
    pub scanning: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTransferPreflight {
    pub project_id: String,
    pub from: ProjectLocation,
    pub to: ProjectLocation,
    pub direction: TransferDirection,
    /// The source workspace of a transfer to local; `None` to remote, where
    /// the workspace is named when it is created.
    pub workspace_name: Option<String>,
    pub repos: Vec<RepoPreflight>,
    pub blockers: Vec<TransferIssue>,
    pub warnings: Vec<TransferIssue>,
    /// No blockers, project-level or on any repo.
    pub can_transfer: bool,
    /// Some repo has staged, unstaged or untracked changes: the dialog's
    /// "include uncommitted changes" checkbox is worth showing.
    pub has_uncommitted_changes: bool,
    /// Some repo is still being read: a partial result, with more to come.
    /// `can_transfer` is what is known so far.
    pub scanning: bool,
}

impl ProjectTransferPreflight {
    pub fn add_blocker(&mut self, issue: TransferIssue) {
        self.blockers.push(issue);
        self.can_transfer = false;
    }

    fn finalize(&mut self) {
        self.can_transfer =
            self.blockers.is_empty() && self.repos.iter().all(|r| r.blockers.is_empty());
        self.has_uncommitted_changes = self.repos.iter().any(|r| r.has_uncommitted_changes);
        self.scanning = self.repos.iter().any(|r| r.scanning);
    }
}

/// Watches an inspection as it runs.
pub(super) struct PreflightObserver<'a> {
    /// Called with the preflight as it stands: once the store has been
    /// read (every row that may have a checkout `scanning`), once Blox has
    /// been asked about the workspaces, and again as each part of each
    /// checkout is read. It is final once nothing in it is `scanning`.
    pub on_partial: &'a dyn Fn(&ProjectTransferPreflight),
    /// Whether to stop reading checkouts. The inspection then fails as
    /// cancelled, once the runs already going have finished.
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
}

fn ignore_partial(_: &ProjectTransferPreflight) {}

fn never() -> bool {
    false
}

impl PreflightObserver<'static> {
    /// Sees nothing and never cancels.
    pub fn none() -> Self {
        Self {
            on_partial: &ignore_partial,
            cancelled: &never,
        }
    }
}

/// A branch as the orchestrator needs it, alongside its preflight row.
#[derive(Debug, Clone)]
pub(super) struct BranchSource {
    pub branch: Branch,
    pub github_repo: String,
    /// The checkout to carry from, as its endpoint sees it: the worktree
    /// locally, the workspace clone remotely. `None` with nothing to carry.
    pub source_dir: Option<String>,
    /// The source workspace, for a remote source.
    pub workspace: Option<String>,
    /// The size of the source repository's object store, which a snapshot
    /// or clone of it takes time in proportion to.
    pub repo_bytes: u64,
}

#[derive(Debug, Clone)]
pub(super) struct Inspection {
    pub project: Project,
    /// In the order of [`ProjectTransferPreflight::repos`].
    pub branches: Vec<BranchSource>,
    /// Source workspaces found running and inspected: the ones a transfer
    /// to local deletes once it has carried everything out of them.
    pub running_workspaces: Vec<String>,
    pub preflight: ProjectTransferPreflight,
}

/// Inspect `project_id` for a transfer away from its current location.
///
/// `Err` is reserved for the store failing; everything about the project
/// and its checkouts comes back as blockers and warnings.
pub(super) fn inspect(
    store: &Store,
    host: &dyn TransferHost,
    project_id: &str,
) -> Result<Inspection, String> {
    inspect_after_resuming(
        store,
        host,
        project_id,
        &BTreeMap::new(),
        &PreflightObserver::none(),
    )
}

/// [`inspect`], once the transfer has tried to resume the source workspaces
/// in `resumed` (with the error, when resuming failed): one of them that is
/// still not running is a blocker rather than something to resume. `Err`
/// also when `observer` cancels, with [`TransferError::Cancelled`]'s message.
pub(super) fn inspect_after_resuming(
    store: &Store,
    host: &dyn TransferHost,
    project_id: &str,
    resumed: &BTreeMap<String, Option<String>>,
    observer: &PreflightObserver<'_>,
) -> Result<Inspection, String> {
    let project = store
        .get_project(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {project_id}"))?;
    let direction = TransferDirection::from_source(project.location);
    let repos = store
        .list_project_repos(project_id)
        .map_err(|e| e.to_string())?;
    let branches = store
        .list_branches_for_project(project_id)
        .map_err(|e| e.to_string())?;
    let repos_by_id: HashMap<&str, &ProjectRepo> =
        repos.iter().map(|r| (r.id.as_str(), r)).collect();

    let mut preflight = ProjectTransferPreflight {
        project_id: project_id.to_string(),
        from: direction.source(),
        to: direction.destination(),
        direction,
        workspace_name: None,
        repos: Vec::new(),
        blockers: Vec::new(),
        warnings: Vec::new(),
        can_transfer: true,
        has_uncommitted_changes: false,
        scanning: false,
    };

    if repos.is_empty() {
        preflight.add_blocker(TransferIssue::new(
            "no_repos",
            "Attach a repository to the project before transferring it.",
        ));
    }
    if direction == TransferDirection::ToRemote {
        remote_layout_blockers(&project, &repos_by_id, &repos, &branches, &mut preflight);
    }
    if project_session_running(store, project_id)? {
        preflight.warnings.push(TransferIssue::new(
            "project_session_running",
            "A project session is running. If it adds a branch before the transfer finishes, \
             the transfer stops and nothing changes.",
        ));
    }

    let source_type = match direction {
        TransferDirection::ToRemote => BranchType::Local,
        TransferDirection::ToLocal => BranchType::Remote,
    };
    let mut sources = Vec::new();
    for branch in branches {
        let repo = branch_repo(&repos_by_id, &branch);
        let github_repo = branch_github_repo(&project, &repos_by_id, &branch);
        let mut row = RepoPreflight {
            branch_id: branch.id.clone(),
            project_repo_id: branch.project_repo_id.clone(),
            github_repo: github_repo.clone(),
            subpath: repo.and_then(|r| r.subpath.clone()),
            branch_name: branch.branch_name.clone(),
            dest_branch_name: branch.branch_name.clone(),
            state: RepoTransferState::NothingToCarry,
            head: None,
            upstream: None,
            unpushed_commits: 0,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
            untracked_bytes: 0,
            ignored_files: 0,
            ignored_bytes: 0,
            ignored_dirs: 0,
            lfs_present: 0,
            lfs_present_bytes: 0,
            lfs_missing: 0,
            lfs_missing_bytes: 0,
            has_uncommitted_changes: false,
            blockers: Vec::new(),
            warnings: Vec::new(),
            scanning: false,
        };
        if github_repo.is_empty() {
            row.blockers.push(TransferIssue::new(
                "no_repo",
                "The branch has no repository.",
            ));
        }
        if branch.branch_type != source_type {
            row.blockers.push(TransferIssue::new(
                "mixed_branch_types",
                format!(
                    "{} is a {} branch in a {} project.",
                    branch.branch_name,
                    branch.branch_type.as_str(),
                    project.location.as_str()
                ),
            ));
        }
        // The database's own status, whatever Blox says by now: the branch is
        // still being provisioned, and that would land on rows the transfer
        // has just rewritten.
        if branch.workspace_status == Some(WorkspaceStatus::Starting) {
            row.blockers.push(TransferIssue::new(
                "workspace_starting",
                format!(
                    "{}'s workspace is still starting; wait for it to finish.",
                    branch.branch_name
                ),
            ));
        }
        session_blockers(store, &branch, &mut row)?;
        sources.push(BranchSource {
            branch,
            github_repo,
            source_dir: None,
            workspace: None,
            repo_bytes: 0,
        });
        preflight.repos.push(row);
    }

    // Which checkouts there may be to read, from the store alone: a worktree
    // on disk, or a workspace the branch names. Every such row is `scanning`
    // in this first step, so the dialog shows them all before Blox has been
    // asked anything.
    let (mut units, kind, by_workspace) = match direction {
        TransferDirection::ToRemote => (
            plan_local_scans(store, &sources, &mut preflight)?,
            "worktree",
            BTreeMap::new(),
        ),
        TransferDirection::ToLocal => (
            Vec::new(),
            "clone",
            plan_remote_sources(&mut sources, &mut preflight),
        ),
    };
    for unit in &units {
        preflight.repos[unit.row].scanning = true;
    }
    for &i in by_workspace.values().flatten() {
        preflight.repos[i].scanning = true;
    }
    preflight.finalize();
    (observer.on_partial)(&preflight);
    if (observer.cancelled)() {
        return Err(TransferError::Cancelled.to_string());
    }

    // Then Blox: whether it can be reached at all, and the state of each
    // source workspace, which decides which rows are read now and which
    // wait for the transfer to resume their workspace.
    if let Err(unavailable) = host.blox_ready() {
        preflight.add_blocker(TransferIssue::new(unavailable.code, unavailable.message));
    }
    let mut running_workspaces = Vec::new();
    if direction == TransferDirection::ToLocal {
        let (remote_units, running) =
            plan_remote_scans(host, resumed, by_workspace, &sources, &mut preflight);
        units = remote_units;
        running_workspaces = running;
        for row in &mut preflight.repos {
            row.scanning = false;
        }
        for unit in &units {
            preflight.repos[unit.row].scanning = true;
        }
        // Rows that won't be read (a workspace to resume first) still get
        // their local name planned; the rows read get it as they land.
        for (source, row) in sources.iter().zip(preflight.repos.iter_mut()) {
            if !row.scanning {
                plan_local_name(host, source, row);
            }
        }
    }
    preflight.finalize();
    (observer.on_partial)(&preflight);
    if (observer.cancelled)() {
        return Err(TransferError::Cancelled.to_string());
    }

    read_checkouts(
        host,
        direction,
        kind,
        units,
        &mut sources,
        &mut preflight,
        observer,
    );
    if (observer.cancelled)() {
        return Err(TransferError::Cancelled.to_string());
    }
    Ok(Inspection {
        project,
        branches: sources,
        running_workspaces,
        preflight,
    })
}

fn branch_repo<'a>(
    repos_by_id: &HashMap<&str, &'a ProjectRepo>,
    branch: &Branch,
) -> Option<&'a ProjectRepo> {
    branch
        .project_repo_id
        .as_deref()
        .and_then(|id| repos_by_id.get(id).copied())
}

/// The repository a branch is on: its project repo's, or — with none, or
/// one that is gone — the project's primary repository.
fn branch_github_repo(
    project: &Project,
    repos_by_id: &HashMap<&str, &ProjectRepo>,
    branch: &Branch,
) -> String {
    match branch_repo(repos_by_id, branch) {
        Some(repo) => repo.github_repo.clone(),
        None => project.primary_repo().unwrap_or_default().to_string(),
    }
}

/// A Blox workspace has one clone per repo at `~/<repo name>`, so a remote
/// project can hold each repo once, under a name no other repo has, with
/// one branch on it.
fn remote_layout_blockers(
    project: &Project,
    repos_by_id: &HashMap<&str, &ProjectRepo>,
    repos: &[ProjectRepo],
    branches: &[Branch],
    preflight: &mut ProjectTransferPreflight,
) {
    let mut seen_repos = HashSet::new();
    let mut seen_names: HashMap<String, &str> = HashMap::new();
    for repo in repos {
        if !seen_repos.insert(repo.github_repo.as_str()) {
            preflight.add_blocker(TransferIssue::new(
                "duplicate_repo",
                format!(
                    "{} is attached more than once; a workspace clones each repository once.",
                    repo.github_repo
                ),
            ));
            continue;
        }
        let name = crate::branches::repo_name_from_github_repo(&repo.github_repo);
        if let Some(other) = seen_names.insert(name.clone(), repo.github_repo.as_str()) {
            preflight.add_blocker(TransferIssue::new(
                "duplicate_repo_name",
                format!(
                    "{other} and {} would both be cloned to ~/{name} in the workspace.",
                    repo.github_repo
                ),
            ));
        }
    }
    // By the repository each branch resolves to, so a branch with no project
    // repo counts against the primary one.
    let mut per_repo: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for branch in branches {
        per_repo
            .entry(branch_github_repo(project, repos_by_id, branch))
            .or_default()
            .push(branch.branch_name.as_str());
    }
    for names in per_repo.values().filter(|names| names.len() > 1) {
        preflight.add_blocker(TransferIssue::new(
            "multiple_branches_per_repo",
            format!(
                "{} are on the same repository; a workspace has one checkout per repository.",
                names.join(", ")
            ),
        ));
    }
}

fn project_session_running(store: &Store, project_id: &str) -> Result<bool, String> {
    for session in store.get_running_sessions().map_err(|e| e.to_string())? {
        if session.branch_id.is_none()
            && store
                .get_project_id_for_session(&session.id)
                .map_err(|e| e.to_string())?
                .as_deref()
                == Some(project_id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A running session blocks the transfer; queued ones don't — they wait
/// for it (see `session_queue`), and start at the destination.
fn session_blockers(store: &Store, branch: &Branch, row: &mut RepoPreflight) -> Result<(), String> {
    if store
        .has_running_session_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
    {
        row.blockers.push(TransferIssue::new(
            "session_running",
            format!(
                "A session is running on {}; stop it or let it finish first.",
                branch.branch_name
            ),
        ));
    }
    Ok(())
}

/// Where a checkout is read from.
enum ScanEnd {
    /// This machine; the path is the worktree.
    Local(PathBuf),
    /// A running workspace.
    Workspace(String),
}

/// One run of one part of [`PREFLIGHT_SCRIPT`] on one checkout.
struct ScanUnit {
    /// Index into [`ProjectTransferPreflight::repos`].
    row: usize,
    part: PreflightPart,
    end: ScanEnd,
    dir: String,
    branch: String,
}

/// A checkout's reading as it stands: what the parts that have landed say,
/// merged.
struct Scan {
    /// The row before any part landed: the project-level blockers. Each
    /// landing rebuilds the row from it, so nothing is read twice.
    base: RepoPreflight,
    dir: String,
    report: ScriptReport,
    parts_left: usize,
    /// The first part that could not run.
    failed: Option<String>,
}

/// To remote: a unit per part for each branch's worktree.
fn plan_local_scans(
    store: &Store,
    sources: &[BranchSource],
    preflight: &mut ProjectTransferPreflight,
) -> Result<Vec<ScanUnit>, String> {
    let mut units = Vec::new();
    for (i, (source, row)) in sources.iter().zip(preflight.repos.iter_mut()).enumerate() {
        let workdir = store
            .get_workdir_for_branch(&source.branch.id)
            .map_err(|e| e.to_string())?;
        let Some(dir) = workdir.map(|w| w.path).filter(|p| Path::new(p).is_dir()) else {
            row.warnings.push(TransferIssue::new(
                "no_worktree",
                format!(
                    "{} has no worktree, so there is nothing to carry; it starts from origin.",
                    row.branch_name
                ),
            ));
            continue;
        };
        for part in [PreflightPart::Status, PreflightPart::Content] {
            units.push(ScanUnit {
                row: i,
                part,
                end: ScanEnd::Local(PathBuf::from(&dir)),
                dir: dir.clone(),
                branch: row.branch_name.clone(),
            });
        }
    }
    Ok(units)
}

/// To local, from the store alone: the rows by the source workspace their
/// branch names (set on each source too), with a warning on every row that
/// names none. The first workspace names the preflight.
fn plan_remote_sources(
    sources: &mut [BranchSource],
    preflight: &mut ProjectTransferPreflight,
) -> BTreeMap<String, Vec<usize>> {
    let mut by_workspace: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, source) in sources.iter_mut().enumerate() {
        match source
            .branch
            .workspace_name
            .as_deref()
            .filter(|n| !n.is_empty())
        {
            Some(name) => {
                source.workspace = Some(name.to_string());
                by_workspace.entry(name.to_string()).or_default().push(i);
            }
            None => {
                let row = &mut preflight.repos[i];
                let issue = TransferIssue::new(
                    "workspace_missing",
                    format!(
                        "{} has no workspace, so there is nothing to carry; it starts from origin.",
                        row.branch_name
                    ),
                );
                row.warnings.push(issue);
            }
        }
    }
    preflight.workspace_name = by_workspace.keys().next().cloned();
    by_workspace
}

/// To local, asking Blox: a unit per part for each branch's clone in a
/// running source workspace of [`plan_remote_sources`]'s; returns the units
/// and those workspaces. One that is suspended, stopped or failed is for the
/// transfer to resume — unless it is in `resumed` already, when it is a
/// blocker.
fn plan_remote_scans(
    host: &dyn TransferHost,
    resumed: &BTreeMap<String, Option<String>>,
    by_workspace: BTreeMap<String, Vec<usize>>,
    sources: &[BranchSource],
    preflight: &mut ProjectTransferPreflight,
) -> (Vec<ScanUnit>, Vec<String>) {
    let mut units = Vec::new();
    let mut running = Vec::new();
    for (workspace, indexes) in by_workspace {
        let state = match host.workspace_state(&workspace) {
            Ok(state) => state,
            Err(e) => {
                preflight.add_blocker(TransferIssue::new(
                    "workspace_unreachable",
                    format!("Can't read workspace {workspace}: {e}"),
                ));
                continue;
            }
        };
        let not_running = match state {
            WorkspaceState::Running => None,
            WorkspaceState::Starting => {
                preflight.add_blocker(TransferIssue::new(
                    "workspace_starting",
                    format!("Workspace {workspace} is still starting; wait for it to finish."),
                ));
                continue;
            }
            // Not running, but it may still hold work: resume it and read it,
            // or stop rather than leave that work behind.
            WorkspaceState::Suspended => Some(("workspace_suspended", "suspended")),
            WorkspaceState::Stopped => Some(("workspace_stopped", "stopped")),
            WorkspaceState::Error => Some(("workspace_error", "in an error state")),
            WorkspaceState::NotFound => {
                preflight.warnings.push(TransferIssue::new(
                    "workspace_missing",
                    format!(
                        "Workspace {workspace} no longer exists, so there is nothing to carry; \
                         branches start from origin."
                    ),
                ));
                continue;
            }
        };
        if let Some((code, why)) = not_running {
            match resumed.get(&workspace) {
                None => {
                    preflight.warnings.push(TransferIssue::new(
                        code,
                        format!(
                            "Workspace {workspace} is {why}; the transfer resumes it first, and \
                             stops if it can't."
                        ),
                    ));
                    for &i in &indexes {
                        preflight.repos[i].state = RepoTransferState::Unknown;
                    }
                }
                Some(error) => preflight.add_blocker(TransferIssue::new(
                    "workspace_unreachable",
                    format!(
                        "Workspace {workspace} is still {why}{}, so the work in it can't be \
                         read. Resume it from Blox, then try again.",
                        match error {
                            Some(e) => format!(" after trying to resume it ({})", e.trim()),
                            None => " after resuming it".to_string(),
                        }
                    ),
                )),
            }
            continue;
        }
        running.push(workspace.clone());

        for &i in &indexes {
            match host.workspace_repo_dir(&workspace, &sources[i].github_repo) {
                Ok(dir) => {
                    for part in [PreflightPart::Status, PreflightPart::Content] {
                        units.push(ScanUnit {
                            row: i,
                            part,
                            end: ScanEnd::Workspace(workspace.clone()),
                            dir: dir.clone(),
                            branch: preflight.repos[i].branch_name.clone(),
                        });
                    }
                }
                Err(e) => preflight.repos[i].blockers.push(inspect_failed(&e)),
            }
        }
    }
    (units, running)
}

/// Run `units`, up to [`MAX_PARALLEL_SCANS`] at once, and fill each row in
/// as its parts land — rebuilding the row from what it was before the first
/// part, so a row read in two steps ends up as one read in one — telling
/// `observer` after each. Stops taking units once `observer` cancels; the
/// rows not read stay `scanning`.
#[allow(clippy::too_many_arguments)]
fn read_checkouts(
    host: &dyn TransferHost,
    direction: TransferDirection,
    kind: &str,
    units: Vec<ScanUnit>,
    sources: &mut [BranchSource],
    preflight: &mut ProjectTransferPreflight,
    observer: &PreflightObserver<'_>,
) {
    if units.is_empty() {
        return;
    }
    let mut scans: HashMap<usize, Scan> = HashMap::new();
    for unit in &units {
        scans
            .entry(unit.row)
            .or_insert_with(|| Scan {
                base: RepoPreflight {
                    scanning: false,
                    ..preflight.repos[unit.row].clone()
                },
                dir: unit.dir.clone(),
                report: ScriptReport::default(),
                parts_left: 0,
                failed: None,
            })
            .parts_left += 1;
    }
    let workers = units.len().min(MAX_PARALLEL_SCANS);
    let queue = Mutex::new(VecDeque::from(units));
    let (tx, rx) = mpsc::channel::<(usize, Result<ScriptReport, String>)>();
    std::thread::scope(|s| {
        for _ in 0..workers {
            let tx = tx.clone();
            let queue = &queue;
            let cancelled = observer.cancelled;
            s.spawn(move || {
                while !cancelled() {
                    let Some(unit) = queue.lock().unwrap().pop_front() else {
                        break;
                    };
                    let result = run_scan_unit(host, &unit);
                    if tx.send((unit.row, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        // Until every worker has dropped its sender.
        for (row, result) in rx {
            let scan = scans.get_mut(&row).expect("a scan per unit");
            scan.parts_left -= 1;
            match result {
                Ok(report) => scan.report.merge(report),
                Err(e) => {
                    scan.failed.get_or_insert(e);
                }
            }
            rebuild_row(
                host,
                direction,
                kind,
                scan,
                &mut sources[row],
                &mut preflight.repos[row],
            );
            preflight.finalize();
            (observer.on_partial)(preflight);
        }
    });
}

/// One run of one part of the script, as the report of its one checkout.
fn run_scan_unit(host: &dyn TransferHost, unit: &ScanUnit) -> Result<ScriptReport, String> {
    let endpoint = match &unit.end {
        ScanEnd::Local(dir) => host.local_endpoint(dir)?,
        ScanEnd::Workspace(workspace) => host.workspace_endpoint(workspace),
    };
    let args = scripts::preflight_part_args(unit.part, [(unit.dir.as_str(), unit.branch.as_str())]);
    let output = endpoint
        .run_script(PREFLIGHT_SCRIPT, &args)
        .map_err(|e| e.to_string())?;
    scripts::sections(&output.stdout)
        .into_iter()
        .next()
        .ok_or_else(|| "the inspection printed nothing".to_string())
}

/// `row` as `scan`'s parts so far say, from the row as it was before any.
fn rebuild_row(
    host: &dyn TransferHost,
    direction: TransferDirection,
    kind: &str,
    scan: &Scan,
    source: &mut BranchSource,
    row: &mut RepoPreflight,
) {
    let mut rebuilt = scan.base.clone();
    rebuilt.scanning = scan.parts_left > 0;
    if let Some(e) = &scan.failed {
        rebuilt.blockers.push(inspect_failed(e));
    }
    let carries = !scan.report.is_empty() && read_checkout(&mut rebuilt, &scan.report, kind);
    if carries {
        source.source_dir = Some(scan.dir.clone());
        source.repo_bytes = repo_bytes(&scan.report);
    } else {
        source.source_dir = None;
        source.repo_bytes = 0;
    }
    ignored_cap_warning(&mut rebuilt, host.ignored_files_cap(), kind);
    if direction == TransferDirection::ToLocal {
        plan_local_name(host, source, &mut rebuilt);
    }
    *row = rebuilt;
}

/// Plan the local name of a branch to carry, and say when it differs.
fn plan_local_name(host: &dyn TransferHost, source: &BranchSource, row: &mut RepoPreflight) {
    if row.state == RepoTransferState::NothingToCarry {
        return;
    }
    let Ok(clone) = host.local_clone_path(&source.github_repo) else {
        return;
    };
    if !clone.join(".git").exists() && !clone.join("HEAD").exists() {
        return;
    }
    match local_branch_name(
        &clone,
        &row.branch_name,
        row.head.as_deref(),
        &HashSet::new(),
    ) {
        Ok((name, _)) if name != row.branch_name => {
            row.warnings.push(TransferIssue::new(
                "renamed",
                format!(
                    "{} is already used in the local clone; it lands as {name}.",
                    row.branch_name
                ),
            ));
            row.dest_branch_name = name;
        }
        Ok(_) => {}
        Err(e) => log::warn!(
            "[project_transfer] cannot plan the local name of {}: {e}",
            row.branch_name
        ),
    }
}

/// A local branch name for `branch` in `clone`: `branch` itself, or the
/// first free `branch-N`. A name is free when no ref has it — or when it
/// points at `head` already and no worktree has it checked out, in which case
/// it is reused (`false`) rather than created (`true`). `taken` are names the
/// project's other rows hold on the same repo.
pub(super) fn local_branch_name(
    clone: &Path,
    branch: &str,
    head: Option<&str>,
    taken: &HashSet<String>,
) -> Result<(String, bool), String> {
    let checked_out: HashSet<String> = crate::git::list_worktrees(clone)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|(_, b)| b)
        .collect();
    for n in 1..100 {
        let name = if n == 1 {
            branch.to_string()
        } else {
            format!("{branch}-{n}")
        };
        if taken.contains(&name) || checked_out.contains(&name) {
            continue;
        }
        let existing = crate::git::cli_run(
            clone,
            &["rev-parse", "-q", "--verify", &format!("refs/heads/{name}")],
        )
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
        match existing {
            None => return Ok((name, true)),
            Some(sha) if Some(sha.as_str()) == head => return Ok((name, false)),
            Some(_) => {}
        }
    }
    Err(format!("no free local name for {branch}"))
}

fn inspect_failed(message: &str) -> TransferIssue {
    TransferIssue::new(
        "inspect_failed",
        format!("Couldn't inspect the checkout: {}", message.trim()),
    )
}

/// The source repository's size, from its section of the report.
fn repo_bytes(report: &ScriptReport) -> u64 {
    report
        .get("repo_kib")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .saturating_mul(1024)
}

/// Ignored files over `cap` can be neither carried nor backed up: say so, as
/// removing the source deletes them.
fn ignored_cap_warning(row: &mut RepoPreflight, cap: u64, kind: &str) {
    if row.ignored_bytes <= cap {
        return;
    }
    let mb = |bytes: u64| bytes.div_ceil(1024 * 1024);
    row.warnings.push(TransferIssue::new(
        IGNORED_TOO_LARGE,
        format!(
            "{}'s ignored files come to {} MB, more than the {} MB a transfer can carry or \
             back up, so they are deleted with the {kind}.",
            row.branch_name,
            mb(row.ignored_bytes),
            mb(cap)
        ),
    ));
}

/// Fill `row` from its section of the report. Returns whether there is a
/// checkout to carry (blocked or not).
fn read_checkout(row: &mut RepoPreflight, report: &ScriptReport, kind: &str) -> bool {
    let count = |key: &str| report.get(key).and_then(|v| v.parse().ok()).unwrap_or(0);
    if report.get("missing").is_some() {
        row.warnings.push(TransferIssue::new(
            "no_checkout",
            format!(
                "{} has no {kind}, so there is nothing to carry; it starts from origin.",
                row.branch_name
            ),
        ));
        return false;
    }
    row.state = RepoTransferState::Carry;
    if report.get("not_a_checkout").is_some() {
        row.blockers.push(TransferIssue::new(
            "not_a_checkout",
            format!("The {kind} for {} is not a git checkout.", row.branch_name),
        ));
        return true;
    }
    row.head = report.get("head").map(str::to_string);
    row.upstream = report.get("upstream").map(str::to_string);
    row.unpushed_commits = count("unpushed");
    row.staged = count("staged");
    row.unstaged = count("unstaged");
    row.untracked = count("untracked");
    row.conflicted = count("conflicted");
    row.untracked_bytes = report
        .get("untracked_bytes")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // `ignored` counts every ignored entry outside an ignored directory;
    // `ignored_files` only those a transfer archives (no symlinks).
    row.ignored_files = count("ignored_files");
    row.ignored_bytes = report
        .get("ignored_bytes")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    row.ignored_dirs = count("ignored_dirs");
    // Unlisted, they aren't carried (`ignored_files` is 0), and a source
    // whose backup fails the same way is kept.
    if report.get("ignored_unlisted").is_some() {
        row.warnings.push(TransferIssue::new(
            IGNORED_UNLISTED,
            format!(
                "{}'s ignored files couldn't be listed, so they won't be transferred; if they \
                 still can't be backed up when the transfer ends, the {kind} is kept.",
                row.branch_name
            ),
        ));
    }
    if let Some(lfs) = LfsCounts::from_report(report) {
        let changed = |key: &str| report.get(key).and_then(|v| v.parse::<u64>().ok());
        row.lfs_present = lfs.present + changed("lfs_changed").unwrap_or(0);
        row.lfs_present_bytes = lfs.present_bytes + changed("lfs_changed_bytes").unwrap_or(0);
        row.lfs_missing = lfs.missing;
        row.lfs_missing_bytes = lfs.missing_bytes;
    }
    row.has_uncommitted_changes = row.staged + row.unstaged + row.untracked + row.conflicted > 0;

    if report.get("detached").is_some() {
        row.blockers.push(TransferIssue::new(
            "detached_head",
            format!("The {kind} for {} has a detached HEAD.", row.branch_name),
        ));
    } else if let Some(head_ref) = report.get("head_ref").filter(|r| *r != row.branch_name) {
        row.blockers.push(TransferIssue::new(
            "wrong_branch",
            format!(
                "The {kind} for {} has {head_ref} checked out.",
                row.branch_name
            ),
        ));
    } else if row.head.is_none() {
        row.blockers.push(TransferIssue::new(
            "no_commits",
            format!("{} has no commits.", row.branch_name),
        ));
    }
    for operation in report.all("operation") {
        row.blockers.push(TransferIssue::new(
            "operation_in_progress",
            format!(
                "A {operation} is in progress on {}; finish or abort it first.",
                row.branch_name
            ),
        ));
    }
    if row.conflicted > 0 {
        row.blockers.push(TransferIssue::new(
            "conflicts",
            format!("{} has unresolved conflicts.", row.branch_name),
        ));
    }
    // A snapshot would record a repository inside the checkout as a bare
    // commit id, and its contents would be lost with the source.
    for path in report.all("nested_repo") {
        row.blockers.push(TransferIssue::new(
            "nested_repo",
            format!(
                "{path} in the {kind} for {} is a separate git repository, which can't be \
                 carried. Move it out of the checkout, or delete it, first.",
                row.branch_name
            ),
        ));
    }
    if report.all("warning").any(|w| w == "submodules") {
        row.warnings.push(TransferIssue::new(
            "submodules",
            "Submodule commits aren't carried; the destination checks out what origin has.",
        ));
    }
    if row.lfs_missing > 0 {
        let here = if kind == "worktree" {
            "locally"
        } else {
            "in the workspace"
        };
        row.warnings.push(TransferIssue::new(
            "lfs_missing",
            format!(
                "{} {} not available {here} and will be fetched from origin if possible.",
                row.lfs_missing,
                if row.lfs_missing == 1 {
                    "LFS object is"
                } else {
                    "LFS objects are"
                }
            ),
        ));
    }
    if row.untracked_bytes > LARGE_PAYLOAD_BYTES {
        row.warnings.push(TransferIssue::new(
            "large_payload",
            format!(
                "{} has {} MB of untracked files to carry.",
                row.branch_name,
                row.untracked_bytes / (1024 * 1024)
            ),
        ));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> RepoPreflight {
        RepoPreflight {
            branch_id: "b".to_string(),
            project_repo_id: None,
            github_repo: "acme/widgets".to_string(),
            subpath: None,
            branch_name: "feature".to_string(),
            dest_branch_name: "feature".to_string(),
            state: RepoTransferState::NothingToCarry,
            head: None,
            upstream: None,
            unpushed_commits: 0,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
            untracked_bytes: 0,
            ignored_files: 0,
            ignored_bytes: 0,
            ignored_dirs: 0,
            lfs_present: 0,
            lfs_present_bytes: 0,
            lfs_missing: 0,
            lfs_missing_bytes: 0,
            has_uncommitted_changes: false,
            blockers: Vec::new(),
            warnings: Vec::new(),
            scanning: false,
        }
    }

    /// Ignored files that can't be listed can't be carried or backed up: a
    /// warning says so, rather than a count of none.
    #[test]
    fn ignored_files_that_cant_be_listed_are_a_warning() {
        let report = ScriptReport::parse(
            "head_ref=feature\nhead=abc\nstaged=0\nunstaged=0\nuntracked=0\nconflicted=0\n\
             ignored=3\nignored_dirs=1\nignored_unlisted=1\n",
        );
        let mut unlisted = row();
        assert!(read_checkout(&mut unlisted, &report, "worktree"));
        assert_eq!(unlisted.ignored_files, 0);
        assert!(unlisted.blockers.is_empty());
        let codes: Vec<&str> = unlisted.warnings.iter().map(|w| w.code.as_str()).collect();
        assert_eq!(codes, [IGNORED_UNLISTED]);
        assert!(
            unlisted.warnings[0]
                .message
                .contains("the worktree is kept"),
            "{:?}",
            unlisted.warnings
        );

        let listed = ScriptReport::parse(
            "head_ref=feature\nhead=abc\nignored=3\nignored_files=3\nignored_bytes=30\n",
        );
        let mut row = row();
        read_checkout(&mut row, &listed, "worktree");
        assert_eq!(row.ignored_files, 3);
        assert!(row.warnings.is_empty(), "{:?}", row.warnings);
    }
}
