//! What a transfer would carry, and what stops it. Read-only: the checkouts
//! are inspected with [`PREFLIGHT_SCRIPT`], one run per local worktree and
//! one per workspace, and nothing is resumed, fetched or cloned.
//!
//! The orchestrator runs the same [`inspect`] once the transfer is
//! registered, so the dialog and the transfer agree on what each branch
//! carries; the snapshot script re-checks the repo-level blockers as it
//! snapshots.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::store::{
    Branch, BranchType, Project, ProjectLocation, ProjectRepo, Store, WorkspaceStatus,
};

use super::host::{TransferHost, WorkspaceState};
use super::scripts::{self, ScriptReport, PREFLIGHT_SCRIPT};

/// Untracked content past this size gets a warning: it is carried in full.
const LARGE_PAYLOAD_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    /// Ignored files outside ignored directories, each counted — including
    /// every file of an untracked directory that holds nothing else. None of
    /// them is carried, and removing the source deletes them.
    pub ignored_files: u32,
    /// Directories an ignore pattern matches (`node_modules/`, `.venv/`),
    /// each counted once however much they hold. Like [`Self::ignored_files`],
    /// never carried.
    pub ignored_dirs: u32,
    pub has_uncommitted_changes: bool,
    pub blockers: Vec<TransferIssue>,
    pub warnings: Vec<TransferIssue>,
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
    inspect_after_resuming(store, host, project_id, &BTreeMap::new())
}

/// [`inspect`], once the transfer has tried to resume the source workspaces
/// in `resumed` (with the error, when resuming failed): one of them that is
/// still not running is a blocker rather than something to resume.
pub(super) fn inspect_after_resuming(
    store: &Store,
    host: &dyn TransferHost,
    project_id: &str,
    resumed: &BTreeMap<String, Option<String>>,
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
    };

    if repos.is_empty() {
        preflight.add_blocker(TransferIssue::new(
            "no_repos",
            "Attach a repository to the project before transferring it.",
        ));
    }
    if let Err(unavailable) = host.blox_ready() {
        preflight.add_blocker(TransferIssue::new(unavailable.code, unavailable.message));
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
            ignored_dirs: 0,
            has_uncommitted_changes: false,
            blockers: Vec::new(),
            warnings: Vec::new(),
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

    let mut running_workspaces = Vec::new();
    match direction {
        TransferDirection::ToRemote => {
            inspect_local_sources(store, host, &mut sources, &mut preflight)?
        }
        TransferDirection::ToLocal => {
            running_workspaces =
                inspect_remote_sources(host, resumed, &mut sources, &mut preflight);
            plan_local_names(host, &sources, &mut preflight);
        }
    }
    preflight.finalize();
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
    if !store
        .get_queued_sessions_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
        .is_empty()
    {
        row.blockers.push(TransferIssue::new(
            "session_queued",
            format!(
                "Sessions are queued on {}; let them run or cancel them first.",
                branch.branch_name
            ),
        ));
    }
    Ok(())
}

fn inspect_local_sources(
    store: &Store,
    host: &dyn TransferHost,
    sources: &mut [BranchSource],
    preflight: &mut ProjectTransferPreflight,
) -> Result<(), String> {
    for (source, row) in sources.iter_mut().zip(preflight.repos.iter_mut()) {
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
        let report = host
            .local_endpoint(Path::new(&dir))
            .and_then(|endpoint| {
                endpoint
                    .run_script(
                        PREFLIGHT_SCRIPT,
                        &scripts::preflight_args([(dir.as_str(), row.branch_name.as_str())]),
                    )
                    .map_err(|e| e.to_string())
            })
            .map(|output| scripts::sections(&output.stdout));
        match report {
            Ok(sections) => match sections.into_iter().next() {
                Some(section) => {
                    if read_checkout(row, &section, "worktree") {
                        source.source_dir = Some(dir);
                        source.repo_bytes = repo_bytes(&section);
                    }
                }
                None => row
                    .blockers
                    .push(inspect_failed("the inspection printed nothing")),
            },
            Err(e) => row.blockers.push(inspect_failed(&e)),
        }
    }
    Ok(())
}

/// Inspect each source workspace that is running; returns those. One that is
/// suspended, stopped or failed is for the transfer to resume — unless it is
/// in `resumed` already, when it is a blocker.
fn inspect_remote_sources(
    host: &dyn TransferHost,
    resumed: &BTreeMap<String, Option<String>>,
    sources: &mut [BranchSource],
    preflight: &mut ProjectTransferPreflight,
) -> Vec<String> {
    let mut by_workspace: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, source) in sources.iter().enumerate() {
        match source
            .branch
            .workspace_name
            .as_deref()
            .filter(|n| !n.is_empty())
        {
            Some(name) => by_workspace.entry(name.to_string()).or_default().push(i),
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

    let mut running = Vec::new();
    for (workspace, indexes) in by_workspace {
        for &i in &indexes {
            sources[i].workspace = Some(workspace.clone());
        }
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

        let mut dirs = Vec::new();
        for &i in &indexes {
            match host.workspace_repo_dir(&workspace, &sources[i].github_repo) {
                Ok(dir) => dirs.push((i, dir)),
                Err(e) => preflight.repos[i].blockers.push(inspect_failed(&e)),
            }
        }
        let args = scripts::preflight_args(
            dirs.iter()
                .map(|(i, dir)| (dir.as_str(), preflight.repos[*i].branch_name.as_str())),
        );
        let output = host
            .workspace_endpoint(&workspace)
            .run_script(PREFLIGHT_SCRIPT, &args)
            .map(|output| scripts::sections(&output.stdout));
        match output {
            Ok(sections) => {
                for (n, (i, dir)) in dirs.into_iter().enumerate() {
                    let row = &mut preflight.repos[i];
                    match sections.get(n) {
                        Some(section) => {
                            if read_checkout(row, section, "clone") {
                                sources[i].source_dir = Some(dir);
                                sources[i].repo_bytes = repo_bytes(section);
                            }
                        }
                        None => row
                            .blockers
                            .push(inspect_failed("the inspection stopped early")),
                    }
                }
            }
            Err(e) => {
                for (i, _) in dirs {
                    preflight.repos[i]
                        .blockers
                        .push(inspect_failed(&e.to_string()));
                }
            }
        }
    }
    running
}

/// Plan the local name of every branch to carry, and say when it differs.
fn plan_local_names(
    host: &dyn TransferHost,
    sources: &[BranchSource],
    preflight: &mut ProjectTransferPreflight,
) {
    for (source, row) in sources.iter().zip(preflight.repos.iter_mut()) {
        if row.state == RepoTransferState::NothingToCarry {
            continue;
        }
        let Ok(clone) = host.local_clone_path(&source.github_repo) else {
            continue;
        };
        if !clone.join(".git").exists() && !clone.join("HEAD").exists() {
            continue;
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
    row.ignored_files = count("ignored");
    row.ignored_dirs = count("ignored_dirs");
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
    for warning in report.all("warning") {
        match warning {
            "submodules" => row.warnings.push(TransferIssue::new(
                "submodules",
                "Submodule commits aren't carried; the destination checks out what origin has.",
            )),
            "lfs" => row.warnings.push(TransferIssue::new(
                "lfs",
                "Git LFS content isn't carried; the destination fetches it from the LFS server, \
                 so unpushed LFS files arrive as pointers.",
            )),
            _ => {}
        }
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
