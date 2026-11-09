//! Whole-project transfers end to end, against real repositories: a bare
//! origin per repository, a shared local clone of each with a worktree per
//! branch, and a directory per "workspace" holding a clone per repository.
//! [`FakeHost`] stands in for `~/.staged` and Blox — each workspace end is a
//! [`LocalEndpoint`] on its own scratch directory — and fails on request, so
//! every phase's rollback can be checked: the source and the database as they
//! were, nothing left at the destination, the bundles kept.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::backups;
use super::commands::{preflight_for, runs_prerun, settle};
use super::host::{BloxUnavailable, TransferHost, WorkspaceState};
use super::orchestrator::{
    run_transfer, ProjectTransferResult, SerialLegs, TransferOptions, TransferPhase,
    TransferProgress,
};
use super::preflight::{
    inspect_after_resuming, PreflightObserver, ProjectTransferPreflight, RepoTransferState,
    TransferDirection, TransferIssue,
};
use super::recovery::{self, Recovered};
use super::registry;
use super::scripts::{
    APPLY_SCRIPT, CLONE_SCRIPT, ORIGIN_TIPS_SCRIPT, PREFLIGHT_SCRIPT, SNAPSHOT_SCRIPT,
    VERIFY_SCRIPT,
};
use super::tests::{
    commit, configure_identity, git, make_a_mess, noise, observe, rev, source_fingerprint, try_git,
    write, Observed,
};
use super::undo::{self, AddedRemote, CreatedBranch, ReplacedConfig, Undo};
use super::{LocalEndpoint, ScriptOutput, TransferEndpoint, TransferError};
use crate::store::{
    Branch, BranchType, Project, ProjectLocation, ProjectRepo, Session, Store, Workdir,
    WorkspaceStatus,
};
use crate::test_utils::TempDir;

mod ignored;
mod lfs;

const WIDGETS: &str = "acme/widgets";
const GADGETS: &str = "acme/gadgets";
const SPROCKETS: &str = "acme/sprockets";

// ---------------------------------------------------------------------------
// The fake host
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Local,
    Workspace,
}

/// A failure to inject. Each fires once.
#[derive(Clone, Copy)]
enum Fault {
    /// The `nth` run (from 1) of the script at the side cannot run at all.
    Script(Side, &'static str, usize),
    /// Copying a bundle to the side fails.
    PutFile(Side),
    /// Copying a bundle from the side fails.
    GetFile(Side),
    /// The workspace start fails after the workspace was created.
    StartWorkspace,
    /// The local clone cannot be made.
    EnsureClone,
    /// Resuming a workspace fails, leaving it as it was.
    ResumeWorkspace,
    /// Resuming a workspace waits until the transfer is cancelled.
    ResumeHangs,
    /// The `nth` run (from 1) of the script at the side panics.
    Panic(Side, &'static str, usize),
    /// The `nth` run (from 1) of the script at the side runs once the hook
    /// [`FakeHost::before`] set has: something changing just before it.
    Before(Side, &'static str, usize),
}

#[derive(Default)]
struct Faults {
    armed: Option<Fault>,
    seen: usize,
    fired: bool,
    /// What [`Fault::Before`] runs.
    hook: Option<Box<dyn FnOnce() + Send>>,
}

impl Faults {
    /// Whether this call, if `matches` the armed fault, is the one to fail.
    fn hit(&mut self, matches: impl Fn(&Fault) -> bool) -> bool {
        let Some(fault) = self.armed else {
            return false;
        };
        if self.fired || !matches(&fault) {
            return false;
        }
        self.seen += 1;
        let nth = match fault {
            Fault::Script(_, _, nth) | Fault::Panic(_, _, nth) | Fault::Before(_, _, nth) => nth,
            _ => 1,
        };
        self.fired = self.seen == nth;
        self.fired
    }
}

fn injected(endpoint: String) -> TransferError {
    TransferError::Exec {
        endpoint,
        message: "injected failure".to_string(),
    }
}

/// A [`LocalEndpoint`] that fails where [`Faults`] says.
struct FaultyEndpoint {
    inner: LocalEndpoint,
    side: Side,
    faults: Arc<Mutex<Faults>>,
    script_hook: Option<Arc<dyn Fn(Side, &str) + Send + Sync>>,
}

impl TransferEndpoint for FaultyEndpoint {
    fn describe(&self) -> String {
        format!("{:?}", self.side)
    }

    fn run_script(&self, script: &str, args: &[String]) -> Result<ScriptOutput, TransferError> {
        if let Some(hook) = &self.script_hook {
            hook(self.side, script);
        }
        let hit =
            self.faults.lock().unwrap().hit(
                |f| matches!(f, Fault::Script(side, s, _) if *side == self.side && *s == script),
            );
        if hit {
            return Err(injected(self.describe()));
        }
        let panics =
            self.faults.lock().unwrap().hit(
                |f| matches!(f, Fault::Panic(side, s, _) if *side == self.side && *s == script),
            );
        if panics {
            panic!("injected panic");
        }
        let hook = {
            let mut faults = self.faults.lock().unwrap();
            let hit = faults.hit(
                |f| matches!(f, Fault::Before(side, s, _) if *side == self.side && *s == script),
            );
            if hit {
                faults.hook.take()
            } else {
                None
            }
        };
        if let Some(hook) = hook {
            hook();
        }
        self.inner.run_script(script, args)
    }

    fn scratch_dir(&self, transfer_id: &str) -> Result<String, TransferError> {
        self.inner.scratch_dir(transfer_id)
    }

    fn put_file(&self, local: &Path, path: &str) -> Result<(), TransferError> {
        let hit = self
            .faults
            .lock()
            .unwrap()
            .hit(|f| matches!(f, Fault::PutFile(side) if *side == self.side));
        if hit {
            return Err(injected(self.describe()));
        }
        self.inner.put_file(local, path)
    }

    fn get_file(&self, path: &str, local: &Path, size: u64) -> Result<(), TransferError> {
        let hit = self
            .faults
            .lock()
            .unwrap()
            .hit(|f| matches!(f, Fault::GetFile(side) if *side == self.side));
        if hit {
            return Err(injected(self.describe()));
        }
        self.inner.get_file(path, local, size)
    }
}

/// `~/.staged` and Blox under a temp directory:
///
/// - `origins/<owner>/<repo>.git`: each repository's bare origin
/// - `clones/<owner>/<repo>`: the shared local clones
/// - `worktrees/<repo>/<branch>`: the project's worktrees
/// - `transfers/<transfer id>`: the bundles and manifest kept as the backup
/// - `ws/<name>/<repo>`: a workspace's clones, with its scratch directory at
///   `ws/<name>/.staged-transfer`
struct FakeHost {
    _tmp: TempDir,
    root: PathBuf,
    /// States set by the test (or by resuming); otherwise a workspace is
    /// running while its directory exists.
    states: Mutex<HashMap<String, WorkspaceState>>,
    /// Workspace lifecycle calls, as `start <name>`, `resume <name>`,
    /// `delete <name>`.
    calls: Mutex<Vec<String>>,
    faults: Arc<Mutex<Faults>>,
    blox: Mutex<Option<BloxUnavailable>>,
    /// What a new workspace's bootstrap does to its primary clone.
    bootstrap: Mutex<Option<fn(&Path)>>,
    /// Set for every script, at either end.
    script_vars: Mutex<Vec<(String, String)>>,
    /// The ignored files' cap, when a test lowers it.
    ignored_cap: Mutex<Option<u64>>,
    /// Called at the start of every script run, at either end, on the
    /// thread running it.
    script_hook: Mutex<Option<Arc<dyn Fn(Side, &str) + Send + Sync>>>,
}

impl FakeHost {
    /// Origins for `repos`, each seeded with the files [`make_a_mess`]
    /// works on, and their shared local clones.
    fn new(repos: &[&str]) -> Self {
        let tmp = TempDir::new("transfer-project");
        // Canonical, so the paths git prints match the ones stored.
        let root = tmp.path().canonicalize().unwrap();
        let host = Self {
            _tmp: tmp,
            root,
            states: Mutex::default(),
            calls: Mutex::default(),
            faults: Arc::default(),
            blox: Mutex::default(),
            bootstrap: Mutex::default(),
            script_vars: Mutex::default(),
            ignored_cap: Mutex::default(),
            script_hook: Mutex::default(),
        };
        for repo in repos {
            let origin = host.origin(repo);
            fs::create_dir_all(&origin).unwrap();
            git(&origin, &["init", "-q", "--bare", "--initial-branch=main"]);
            let seed = host.root.join("seeds").join(repo);
            fs::create_dir_all(seed.parent().unwrap()).unwrap();
            git(&host.root, &["clone", "-q", path(&origin), path(&seed)]);
            configure_identity(&seed);
            write(&seed, ".gitignore", "*.log\n");
            write(&seed, "a.txt", "alpha\n");
            write(&seed, "rename-me.txt", "rename me\n");
            write(&seed, "delete-staged.txt", "staged delete\n");
            write(&seed, "delete-unstaged.txt", "unstaged delete\n");
            write(&seed, "mode.sh", "#!/bin/sh\necho hi\n");
            write(&seed, "partial.txt", "one\ntwo\nthree\nfour\nfive\n");
            write(&seed, "bin.dat", noise(4096, 7));
            commit(&seed, "seed");
            git(&seed, &["push", "-q", "origin", "main"]);
            host.ensure_local_clone(repo).unwrap();
        }
        host
    }

    fn origin(&self, repo: &str) -> PathBuf {
        self.root.join("origins").join(format!("{repo}.git"))
    }

    fn clone_of(&self, repo: &str) -> PathBuf {
        self.local_clone_path(repo).unwrap()
    }

    fn workspace_dir(&self, name: &str) -> PathBuf {
        self.root.join("ws").join(name)
    }

    fn workspace_clone(&self, name: &str, repo: &str) -> PathBuf {
        PathBuf::from(self.workspace_repo_dir(name, repo).unwrap())
    }

    /// Every workspace that exists.
    fn workspaces(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.root.join("ws"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn set_state(&self, name: &str, state: WorkspaceState) {
        self.states.lock().unwrap().insert(name.to_string(), state);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn arm(&self, fault: Fault) {
        *self.faults.lock().unwrap() = Faults {
            armed: Some(fault),
            ..Faults::default()
        };
    }

    fn fired(&self) -> bool {
        self.faults.lock().unwrap().fired
    }

    /// Run `hook` right before the `nth` run (from 1) of `script` at `side`.
    fn before(
        &self,
        side: Side,
        script: &'static str,
        nth: usize,
        hook: impl FnOnce() + Send + 'static,
    ) {
        *self.faults.lock().unwrap() = Faults {
            armed: Some(Fault::Before(side, script, nth)),
            hook: Some(Box::new(hook)),
            ..Faults::default()
        };
    }

    fn bootstrap_with(&self, bootstrap: fn(&Path)) {
        *self.bootstrap.lock().unwrap() = Some(bootstrap);
    }

    fn endpoint(&self, env_dir: &Path, scratch_root: PathBuf, side: Side) -> FaultyEndpoint {
        let mut inner = LocalEndpoint::new(env_dir, scratch_root);
        for (key, value) in self.script_vars.lock().unwrap().iter() {
            inner = inner.with_var(key, value);
        }
        FaultyEndpoint {
            inner,
            side,
            faults: Arc::clone(&self.faults),
            script_hook: self.script_hook.lock().unwrap().clone(),
        }
    }

    /// Run `hook` at the start of every script run from now on.
    fn on_every_script(&self, hook: impl Fn(Side, &str) + Send + Sync + 'static) {
        *self.script_hook.lock().unwrap() = Some(Arc::new(hook));
    }
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

impl TransferHost for FakeHost {
    fn local_endpoint(&self, env_dir: &Path) -> Result<Box<dyn TransferEndpoint>, String> {
        Ok(Box::new(self.endpoint(
            env_dir,
            self.transfers_dir()?,
            Side::Local,
        )))
    }

    fn transfers_dir(&self) -> Result<PathBuf, String> {
        Ok(self.root.join("transfers"))
    }

    fn local_clone_path(&self, github_repo: &str) -> Result<PathBuf, String> {
        Ok(self.root.join("clones").join(github_repo))
    }

    fn ensure_local_clone(&self, github_repo: &str) -> Result<PathBuf, String> {
        if self
            .faults
            .lock()
            .unwrap()
            .hit(|f| matches!(f, Fault::EnsureClone))
        {
            return Err("injected failure".to_string());
        }
        let clone = self.local_clone_path(github_repo)?;
        if !clone.exists() {
            fs::create_dir_all(clone.parent().unwrap()).unwrap();
            git(
                &self.root,
                &["clone", "-q", path(&self.origin(github_repo)), path(&clone)],
            );
            configure_identity(&clone);
        }
        Ok(clone)
    }

    fn local_worktree_path(
        &self,
        _project_id: &str,
        github_repo: &str,
        branch: &str,
    ) -> Result<PathBuf, String> {
        Ok(self
            .root
            .join("worktrees")
            .join(crate::branches::repo_name_from_github_repo(github_repo))
            .join(branch))
    }

    fn remote_session_working_dir(&self, github_repo: &str) -> String {
        format!("/remote/{github_repo}")
    }

    fn blox_ready(&self) -> Result<(), BloxUnavailable> {
        match self.blox.lock().unwrap().clone() {
            Some(unavailable) => Err(unavailable),
            None => Ok(()),
        }
    }

    fn workspace_state(&self, name: &str) -> Result<WorkspaceState, String> {
        if let Some(state) = self.states.lock().unwrap().get(name) {
            return Ok(*state);
        }
        Ok(if self.workspace_dir(name).exists() {
            WorkspaceState::Running
        } else {
            WorkspaceState::NotFound
        })
    }

    /// Like Blox: create the workspace, then clone the primary repo at `base`.
    fn start_workspace(
        &self,
        name: &str,
        github_repo: &str,
        base: &str,
        progress: &dyn Fn(&str),
        _cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("start {name}"));
        fs::create_dir_all(self.workspace_dir(name)).unwrap();
        if self
            .faults
            .lock()
            .unwrap()
            .hit(|f| matches!(f, Fault::StartWorkspace))
        {
            return Err("injected failure".to_string());
        }
        let clone = self.workspace_clone(name, github_repo);
        git(
            &self.root,
            &[
                "clone",
                "-q",
                "--branch",
                base,
                path(&self.origin(github_repo)),
                path(&clone),
            ],
        );
        configure_identity(&clone);
        if let Some(bootstrap) = *self.bootstrap.lock().unwrap() {
            bootstrap(&clone);
        }
        progress("Workspace running");
        Ok(())
    }

    fn resume_workspace(&self, name: &str, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("resume {name}"));
        if self
            .faults
            .lock()
            .unwrap()
            .hit(|f| matches!(f, Fault::ResumeWorkspace))
        {
            return Err("injected failure".to_string());
        }
        if self
            .faults
            .lock()
            .unwrap()
            .hit(|f| matches!(f, Fault::ResumeHangs))
        {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !cancelled() {
                assert!(std::time::Instant::now() < deadline, "never cancelled");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            return Err(TransferError::Cancelled.to_string());
        }
        self.set_state(name, WorkspaceState::Running);
        Ok(())
    }

    fn delete_workspace(&self, name: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("delete {name}"));
        self.states.lock().unwrap().remove(name);
        let dir = self.workspace_dir(name);
        if dir.exists() {
            fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn workspace_endpoint(&self, name: &str) -> Box<dyn TransferEndpoint> {
        Box::new(self.endpoint(
            &self.root,
            self.workspace_dir(name).join(".staged-transfer"),
            Side::Workspace,
        ))
    }

    fn workspace_repo_dir(&self, name: &str, github_repo: &str) -> Result<String, String> {
        Ok(path(
            &self
                .workspace_dir(name)
                .join(crate::branches::repo_name_from_github_repo(github_repo)),
        )
        .to_string())
    }

    fn workspace_clone_url(&self, github_repo: &str) -> String {
        path(&self.origin(github_repo)).to_string()
    }

    fn configure_workspace_clone(&self, _name: &str, repo_dir: &str) {
        configure_identity(Path::new(repo_dir));
    }

    fn ignored_files_cap(&self) -> u64 {
        self.ignored_cap
            .lock()
            .unwrap()
            .unwrap_or(super::scripts::IGNORED_FILES_CAP)
    }
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

struct Fixture {
    host: FakeHost,
    store: Store,
    project: Project,
    /// One per repository, in the order given.
    branches: Vec<Branch>,
}

/// One transfer's outcome and everything it reported.
struct Transfer {
    /// As the command reports it.
    result: Result<ProjectTransferResult, String>,
    /// What the command's after-transfer work (diff caches, prerun actions,
    /// queue kicks) was run on, if it ran.
    after_switch: Option<ProjectTransferResult>,
    /// Whether the command's after-failure work (setting the work held for
    /// the transfer going where the project still is) ran.
    held_work_released: bool,
    events: Vec<TransferProgress>,
    transfer_id: String,
}

impl Fixture {
    /// What a transfer names the workspace: after the primary branch, with
    /// the hash of this fixture's project id. Not a constant, because the
    /// id is fresh per fixture: the transfer registry is process-wide and
    /// keyed by it, so tests sharing one id would refuse each other.
    fn workspace(&self) -> String {
        crate::branches::infer_workspace_name(&self.project.id, &self.branches[0].branch_name)
    }

    /// A local project with a branch per `(repo, branch)`, each in its own
    /// worktree off `origin/main`, tracking nothing.
    fn new(repos: &[(&str, &str)]) -> Self {
        Self::build(repos, true)
    }

    /// The same, but with no worktrees: nothing to carry.
    fn without_worktrees(repos: &[(&str, &str)]) -> Self {
        Self::build(repos, false)
    }

    fn build(repos: &[(&str, &str)], worktrees: bool) -> Self {
        let names: Vec<&str> = repos.iter().map(|(repo, _)| *repo).collect();
        let host = FakeHost::new(&names);
        let store = Store::in_memory().unwrap();
        let project = Project::named("transfer").with_primary_repo(repos[0].0);
        store.create_project(&project).unwrap();
        let mut branches = Vec::new();
        for (i, (repo, name)) in repos.iter().enumerate() {
            let mut repo_row = ProjectRepo::new(&project.id, repo, name, None);
            if i == 0 {
                repo_row = repo_row.primary();
            }
            store.create_project_repo(&repo_row).unwrap();
            let branch =
                Branch::new(&project.id, name, "origin/main").with_project_repo(&repo_row.id);
            store.create_branch(&branch).unwrap();
            if worktrees {
                let worktree = host.local_worktree_path(&project.id, repo, name).unwrap();
                git(
                    &host.clone_of(repo),
                    &[
                        "worktree",
                        "add",
                        "-q",
                        "--no-track",
                        "-b",
                        name,
                        path(&worktree),
                        "origin/main",
                    ],
                );
                store
                    .create_workdir(
                        &Workdir::new(&project.id, path(&worktree)).with_branch(&branch.id),
                    )
                    .unwrap();
                store.mark_branch_setup_complete(&branch.id).unwrap();
            }
            branches.push(branch);
        }
        Self {
            host,
            store,
            project,
            branches,
        }
    }

    fn repo(&self, i: usize) -> String {
        let id = self.branches[i].project_repo_id.as_deref().unwrap();
        self.store
            .get_project_repo(id)
            .unwrap()
            .unwrap()
            .github_repo
    }

    fn worktree(&self, i: usize) -> PathBuf {
        let workdir = self
            .store
            .get_workdir_for_branch(&self.branches[i].id)
            .unwrap()
            .expect("the branch has a worktree");
        PathBuf::from(workdir.path)
    }

    /// Where branch `i` is checked out now, wherever the project is.
    fn checkout(&self, i: usize) -> PathBuf {
        let branch = self
            .store
            .get_branch(&self.branches[i].id)
            .unwrap()
            .unwrap();
        match branch.branch_type {
            BranchType::Local => self.worktree(i),
            BranchType::Remote => self
                .host
                .workspace_clone(branch.workspace_name.as_deref().unwrap(), &self.repo(i)),
        }
    }

    fn preflight(&self) -> ProjectTransferPreflight {
        preflight_for(&self.store, &self.host, &self.project.id).unwrap()
    }

    fn transfer(&self, include_uncommitted: bool) -> Transfer {
        self.transfer_with(include_uncommitted, &|_| {})
    }

    /// Transfer the project, calling `hook` on every progress event.
    fn transfer_with(
        &self,
        include_uncommitted: bool,
        hook: &(dyn Fn(&TransferProgress) + Sync),
    ) -> Transfer {
        let options = TransferOptions {
            include_uncommitted,
            include_ignored: false,
        };
        self.transfer_options(options, hook)
    }

    /// [`Self::transfer_with`], with every option.
    fn transfer_options(
        &self,
        options: TransferOptions,
        hook: &(dyn Fn(&TransferProgress) + Sync),
    ) -> Transfer {
        self.transfer_as(self.direction(), options, hook)
    }

    /// The way the project goes from where it is now, as a dialog opened now
    /// would show it.
    fn direction(&self) -> TransferDirection {
        let project = self.store.get_project(&self.project.id).unwrap().unwrap();
        TransferDirection::from_source(project.location)
    }

    /// [`Self::transfer_options`], asked for in `direction` whichever way the
    /// project goes now.
    fn transfer_as(
        &self,
        direction: TransferDirection,
        options: TransferOptions,
        hook: &(dyn Fn(&TransferProgress) + Sync),
    ) -> Transfer {
        let transfer_id = uuid::Uuid::new_v4().to_string();
        let branch_ids = self.branches.iter().map(|b| b.id.clone()).collect();
        let guard = registry::begin(&self.project.id, &transfer_id, direction, branch_ids).unwrap();
        let events = Mutex::new(Vec::new());
        let outcome = run_transfer(&self.store, &self.host, &guard, options, &|progress| {
            // A window that loads now reads the same phase from the registry.
            let listed = registry::list()
                .into_iter()
                .find(|p| p.transfer_id == transfer_id)
                .map(|p| p.phase);
            assert_eq!(listed, Some(progress.phase));
            events.lock().unwrap().push(progress.clone());
            hook(progress);
        });
        drop(guard);
        assert!(!registry::is_project_transferring(&self.project.id));
        let mut after_switch = None;
        let mut held_work_released = false;
        let result = settle(
            outcome,
            |result| after_switch = Some(result.clone()),
            || held_work_released = true,
        );
        Transfer {
            result,
            after_switch,
            held_work_released,
            events: events.into_inner().unwrap(),
            transfer_id,
        }
    }

    /// Everything a transfer switches in the database.
    fn db_state(&self) -> String {
        let project = self.store.get_project(&self.project.id).unwrap().unwrap();
        let mut state = format!("project {:?}\n", project.location);
        for branch in self
            .store
            .list_branches_for_project(&self.project.id)
            .unwrap()
        {
            let workdir = self
                .store
                .get_workdir_for_branch(&branch.id)
                .unwrap()
                .map(|w| w.path);
            state.push_str(&format!(
                "branch {} {:?} {:?} {:?} {:?} setup={}\n",
                branch.branch_name,
                branch.branch_type,
                branch.workspace_name,
                branch.workspace_status,
                workdir,
                branch.setup_complete,
            ));
        }
        for repo in self.store.list_project_repos(&self.project.id).unwrap() {
            state.push_str(&format!("repo {} {}\n", repo.github_repo, repo.branch_name));
        }
        state
    }

    fn bundles(&self, transfer_id: &str) -> usize {
        fs::read_dir(self.host.root.join("transfers").join(transfer_id))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "bundle"))
            .count()
    }

    fn manifest(&self, transfer_id: &str) -> Option<backups::TransferManifest> {
        backups::read_manifest(&self.host.root.join("transfers").join(transfer_id))
    }

    /// Every path under the worktrees directory.
    fn worktree_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for repo in fs::read_dir(self.host.root.join("worktrees"))
            .into_iter()
            .flatten()
            .flatten()
        {
            for wt in fs::read_dir(repo.path()).into_iter().flatten().flatten() {
                dirs.push(wt.path());
            }
        }
        dirs.sort();
        dirs
    }
}

/// widgets on `feature`: pushed, then two commits ahead of origin, with every
/// kind of uncommitted change. gadgets on `gizmo`: never pushed, one commit
/// and an untracked file.
fn two_repo_project() -> Fixture {
    let f = Fixture::new(&[(WIDGETS, "feature"), (GADGETS, "gizmo")]);
    let widgets = f.worktree(0);
    git(&widgets, &["push", "-q", "-u", "origin", "feature"]);
    write(&widgets, "ahead-1.txt", "one\n");
    commit(&widgets, "ahead 1");
    write(&widgets, "ahead-2.txt", "two\n");
    commit(&widgets, "ahead 2");
    make_a_mess(&widgets);
    let gadgets = f.worktree(1);
    write(&gadgets, "gizmo.txt", "local only\n");
    commit(&gadgets, "gizmo");
    write(&gadgets, "notes.txt", "untracked\n");
    f
}

fn temp_refs(repo: &Path) -> String {
    git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/staged-transfer/",
        ],
    )
}

/// A clone's branches, refs, worktrees and remotes.
fn clone_state(clone: &Path) -> String {
    [
        git(
            clone,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        ),
        git(clone, &["worktree", "list", "--porcelain"]),
        git(clone, &["remote", "-v"]),
    ]
    .join("\n")
}

fn phases(events: &[TransferProgress]) -> Vec<TransferPhase> {
    let mut phases: Vec<TransferPhase> = events.iter().map(|e| e.phase).collect();
    phases.dedup();
    phases
}

fn codes(issues: &[TransferIssue]) -> Vec<&str> {
    issues.iter().map(|i| i.code.as_str()).collect()
}

fn add_running_session(store: &Store, branch_id: &str) {
    let session = Session::new_running("work", Path::new("/tmp")).with_branch(branch_id);
    store.create_session(&session).unwrap();
}

// ---------------------------------------------------------------------------
// Successful transfers
// ---------------------------------------------------------------------------

#[test]
fn a_two_repo_project_goes_to_a_workspace_and_back() {
    let f = two_repo_project();
    let (widgets_wt, gadgets_wt) = (f.worktree(0), f.worktree(1));
    let widgets_head = rev(&widgets_wt, "HEAD");
    let gizmo_head = rev(&gadgets_wt, "HEAD");

    let preflight = f.preflight();
    assert!(preflight.can_transfer, "{:?}", preflight.blockers);
    assert!(preflight.has_uncommitted_changes);
    assert_eq!(preflight.repos[0].state, RepoTransferState::Carry);
    assert_eq!(preflight.repos[0].unpushed_commits, 2);
    assert_eq!(
        preflight.repos[0].upstream.as_deref(),
        Some("origin/feature")
    );
    assert_eq!(preflight.repos[1].unpushed_commits, 1);
    assert_eq!(preflight.repos[1].upstream, None, "gizmo was never pushed");

    let expected: Vec<Observed> = vec![observe(&widgets_wt), observe(&gadgets_wt)];
    let guarded = Mutex::new(false);
    let released = Mutex::new(false);
    let ids: Vec<String> = f.branches.iter().map(|b| b.id.clone()).collect();
    let to_remote = f.transfer_with(true, &|progress| match progress.phase {
        TransferPhase::Snapshotting => {
            // Everything that would start work on the project refuses.
            assert!(registry::ensure_project_not_transferring(&f.project.id).is_err());
            for id in &ids {
                assert!(registry::ensure_branch_not_transferring(id).is_err());
            }
            *guarded.lock().unwrap() = true;
        }
        TransferPhase::TearingDown => {
            // Switched over: the branches are free for setup to provision,
            // while the project stays registered until the source is down.
            assert!(registry::ensure_project_not_transferring(&f.project.id).is_err());
            for id in &ids {
                assert!(registry::ensure_branch_not_transferring(id).is_ok());
            }
            *released.lock().unwrap() = true;
        }
        _ => {}
    });
    let result = to_remote.result.expect("to remote");
    assert!(*guarded.lock().unwrap());
    assert!(*released.lock().unwrap());

    // The database points at the workspace.
    assert_eq!(result.location, ProjectLocation::Remote);
    assert_eq!(
        result.workspace_name.as_deref(),
        Some(f.workspace().as_str())
    );
    for branch in &f.branches {
        let branch = f.store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Remote);
        assert_eq!(
            branch.workspace_name.as_deref(),
            Some(f.workspace().as_str())
        );
        assert_eq!(branch.workspace_status, Some(WorkspaceStatus::Running));
        assert!(f
            .store
            .get_workdir_for_branch(&branch.id)
            .unwrap()
            .is_none());
    }
    assert_eq!(f.host.calls(), vec![format!("start {}", f.workspace())]);

    // Each workspace clone is the worktree it came from, unpushed commits,
    // tracking and uncommitted changes included.
    for (i, expected) in expected.iter().enumerate() {
        let clone = f.checkout(i);
        assert_eq!(&observe(&clone), expected, "{}", f.repo(i));
        assert_eq!(temp_refs(&clone), "");
    }
    assert!(try_git(&f.checkout(1), &["rev-parse", "@{upstream}"]).is_err());
    assert_eq!(result.repos[0].head.as_deref(), Some(widgets_head.as_str()));
    assert_eq!(result.repos[1].head.as_deref(), Some(gizmo_head.as_str()));
    assert!(result.repos.iter().all(|r| r.carried));
    assert!(result.repos[0].uncommitted_restored);
    assert!(!result.repos[0].uncommitted_left_behind);
    assert!(!f
        .host
        .workspace_dir(&f.workspace())
        .join(".staged-transfer")
        .join(&to_remote.transfer_id)
        .exists());

    // The worktrees are gone; their snapshots stay under backup refs.
    assert!(!widgets_wt.exists() && !gadgets_wt.exists());
    let manifest = f.manifest(&to_remote.transfer_id).expect("manifest");
    assert_eq!(manifest.status, "completed");
    assert_eq!(manifest.repos.len(), 2);
    assert_eq!(f.bundles(&to_remote.transfer_id), 2);
    for (i, repo) in result.repos.iter().enumerate() {
        let clone = f.host.clone_of(&f.repo(i));
        let backup_ref = repo.backup_ref.as_deref().expect("backup ref");
        assert_eq!(manifest.repos[i].backup_ref.as_deref(), Some(backup_ref));
        assert_eq!(manifest.repos[i].local_clone.as_deref(), Some(path(&clone)));
        assert_eq!(rev(&clone, backup_ref), manifest.repos[i].snapshot);
        assert!(!git(&clone, &["worktree", "list"]).contains("worktrees"));
        assert!(try_git(
            &clone,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", repo.branch_name)
            ]
        )
        .is_err());
    }
    let widgets_backup = result.repos[0].backup_ref.clone().unwrap();
    git(
        &f.host.clone_of(WIDGETS),
        &["cat-file", "-e", &format!("{widgets_backup}:untracked.bin")],
    );

    let phases = phases(&to_remote.events);
    assert_eq!(phases.first(), Some(&TransferPhase::Checking));
    assert_eq!(phases.last(), Some(&TransferPhase::Completed));
    for phase in [
        TransferPhase::PreparingDestination,
        TransferPhase::Snapshotting,
        TransferPhase::Moving,
        TransferPhase::Applying,
        TransferPhase::Committing,
        TransferPhase::TearingDown,
    ] {
        assert!(phases.contains(&phase), "{phase:?} in {phases:?}");
    }
    assert!(!phases.contains(&TransferPhase::Failed));
    assert!(to_remote
        .events
        .iter()
        .filter(|e| e.phase == TransferPhase::Moving)
        .all(|e| e.bytes.is_some_and(|b| b > 0) && e.branch_id.is_some()));

    // More work in the workspace, then back.
    write(&f.checkout(0), "remote-only.txt", "made remotely\n");
    let expected: Vec<Observed> = vec![observe(&f.checkout(0)), observe(&f.checkout(1))];
    let preflight = f.preflight();
    assert!(preflight.can_transfer, "{:?}", preflight.blockers);
    assert_eq!(
        preflight.workspace_name.as_deref(),
        Some(f.workspace().as_str())
    );
    assert_eq!(preflight.repos[0].untracked, 5);

    let workspace_guarded = Mutex::new(false);
    let to_local = f.transfer_with(true, &|progress| {
        if progress.phase == TransferPhase::Snapshotting {
            assert!(registry::ensure_workspace_not_transferring(&f.workspace()).is_err());
            *workspace_guarded.lock().unwrap() = true;
        }
    });
    let result = to_local.result.expect("to local");
    assert!(*workspace_guarded.lock().unwrap());
    assert_eq!(result.location, ProjectLocation::Local);
    // The workspace's snapshots stay under backup refs in the local clones.
    let manifest = f.manifest(&to_local.transfer_id).expect("manifest");
    for (i, repo) in result.repos.iter().enumerate() {
        let clone = f.host.clone_of(&f.repo(i));
        let backup_ref = repo.backup_ref.as_deref().expect("backup ref");
        assert_eq!(manifest.repos[i].backup_ref.as_deref(), Some(backup_ref));
        assert_eq!(manifest.repos[i].local_clone.as_deref(), Some(path(&clone)));
        assert_eq!(rev(&clone, backup_ref), manifest.repos[i].snapshot);
    }
    assert!(!manifest.keep_until_removed, "everything was restored");
    for (i, expected) in expected.iter().enumerate() {
        let branch = f.store.get_branch(&f.branches[i].id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Local);
        assert_eq!(branch.workspace_name, None);
        let worktree = f.worktree(i);
        assert_eq!(
            worktree,
            f.host
                .local_worktree_path(&f.project.id, &f.repo(i), &branch.branch_name)
                .unwrap()
        );
        assert_eq!(&observe(&worktree), expected, "{}", f.repo(i));
        assert_eq!(temp_refs(&worktree), "");
    }
    assert!(f.host.workspaces().is_empty(), "the workspace is deleted");
    assert_eq!(
        f.host.calls(),
        vec![
            format!("start {}", f.workspace()),
            format!("delete {}", f.workspace())
        ]
    );
}

#[test]
fn unchecked_uncommitted_changes_stay_behind_in_the_backup() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "ahead.txt", "ahead\n");
    let head = commit(&worktree, "ahead");
    make_a_mess(&worktree);

    let to_remote = f.transfer(false);
    let result = to_remote.result.expect("to remote");
    let clone = f.checkout(0);
    assert_eq!(rev(&clone, "HEAD"), head);
    assert_eq!(git(&clone, &["status", "--porcelain"]), "", "arrives clean");
    assert!(result.repos[0].uncommitted_left_behind);
    assert!(!result.repos[0].uncommitted_restored);
    let manifest = f.manifest(&to_remote.transfer_id).unwrap();
    assert!(manifest.keep_until_removed, "the only copy is never pruned");
    // The changes are in the backup ref (and bundle), not lost.
    let backup_ref = result.repos[0].backup_ref.clone().unwrap();
    let local_clone = f.host.clone_of(WIDGETS);
    git(
        &local_clone,
        &["cat-file", "-e", &format!("{backup_ref}:untracked.bin")],
    );
    assert_eq!(f.bundles(&to_remote.transfer_id), 1);

    // And back: the workspace's changes stay in the workspace's bundle.
    write(&clone, "remote.txt", "remote\n");
    let to_local = f.transfer(false);
    let result = to_local.result.expect("to local");
    let worktree = f.worktree(0);
    assert_eq!(rev(&worktree, "HEAD"), head);
    assert_eq!(git(&worktree, &["status", "--porcelain"]), "");
    assert!(result.repos[0].uncommitted_left_behind);
    let manifest = f.manifest(&to_local.transfer_id).unwrap();
    assert!(!manifest.include_uncommitted);
    assert!(manifest.repos[0].has_uncommitted_changes);
    assert!(manifest.keep_until_removed);
    let backup_ref = result.repos[0].backup_ref.clone().unwrap();
    git(
        &local_clone,
        &["cat-file", "-e", &format!("{backup_ref}:remote.txt")],
    );
    assert_eq!(f.bundles(&to_local.transfer_id), 1);
}

#[test]
fn branches_with_nothing_to_carry_land_unprovisioned() {
    let f = Fixture::without_worktrees(&[(WIDGETS, "feature")]);
    let preflight = f.preflight();
    assert!(preflight.can_transfer);
    assert_eq!(preflight.repos[0].state, RepoTransferState::NothingToCarry);
    assert_eq!(codes(&preflight.repos[0].warnings), vec!["no_worktree"]);

    let to_remote = f.transfer(true);
    let result = to_remote.result.expect("to remote");
    assert!(!result.repos[0].carried);
    assert_eq!(result.backup_dir, None);
    assert!(f.host.calls().is_empty(), "no workspace is started");
    let branch = f.store.get_branch(&f.branches[0].id).unwrap().unwrap();
    assert_eq!(branch.branch_type, BranchType::Remote);
    assert_eq!(
        branch.workspace_name.as_deref(),
        Some(f.workspace().as_str())
    );
    assert_eq!(
        branch.workspace_status,
        Some(WorkspaceStatus::Starting),
        "workspace setup provisions it"
    );

    // Until setup has provisioned it, it can't go back: the database's
    // `Starting` is enough, whatever Blox says (here: no workspace at all).
    let preflight = f.preflight();
    assert!(!preflight.can_transfer);
    assert_eq!(
        codes(&preflight.repos[0].blockers),
        vec!["workspace_starting"]
    );
    let err = f.transfer(true).result.unwrap_err();
    assert!(err.contains("still starting"), "{err}");
    f.store
        .update_branch_workspace_status(&f.branches[0].id, &WorkspaceStatus::Error)
        .unwrap();

    // The workspace was never created, so back to local there is nothing
    // to carry either.
    let preflight = f.preflight();
    assert_eq!(codes(&preflight.warnings), vec!["workspace_missing"]);
    assert_eq!(preflight.repos[0].state, RepoTransferState::NothingToCarry);
    let result = f.transfer(true).result.expect("to local");
    assert!(!result.repos[0].carried);
    let branch = f.store.get_branch(&f.branches[0].id).unwrap().unwrap();
    assert_eq!(branch.branch_type, BranchType::Local);
    assert!(f
        .store
        .get_workdir_for_branch(&branch.id)
        .unwrap()
        .is_none());
    assert!(f.host.calls().is_empty(), "nothing to delete");
}

#[test]
fn a_taken_local_name_lands_deduplicated_after_resuming_the_workspace() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "ahead.txt", "ahead\n");
    let head = commit(&worktree, "ahead");
    f.transfer(true).result.expect("to remote");

    // Meanwhile someone made a different `feature` locally, and the
    // workspace went to sleep.
    let clone = f.host.clone_of(WIDGETS);
    git(&clone, &["branch", "feature", "origin/main"]);
    let theirs = rev(&clone, "refs/heads/feature");
    f.host.set_state(&f.workspace(), WorkspaceState::Suspended);

    let preflight = f.preflight();
    assert!(preflight.can_transfer, "{:?}", preflight.blockers);
    assert_eq!(codes(&preflight.warnings), vec!["workspace_suspended"]);
    assert_eq!(preflight.repos[0].state, RepoTransferState::Unknown);
    assert_eq!(preflight.repos[0].dest_branch_name, "feature-2");
    assert_eq!(codes(&preflight.repos[0].warnings), vec!["renamed"]);

    let result = f.transfer(true).result.expect("to local");
    assert_eq!(result.repos[0].branch_name, "feature-2");
    assert_eq!(result.repos[0].previous_branch_name, "feature");
    assert!(f
        .host
        .calls()
        .contains(&format!("resume {}", f.workspace())));
    let branch = f.store.get_branch(&f.branches[0].id).unwrap().unwrap();
    assert_eq!(branch.branch_name, "feature-2");
    let repo = f.store.list_project_repos(&f.project.id).unwrap().remove(0);
    assert_eq!(repo.branch_name, "feature-2");
    let worktree = f.worktree(0);
    assert!(worktree.ends_with("widgets/feature-2"));
    assert_eq!(rev(&worktree, "HEAD"), head);
    assert_eq!(
        rev(&clone, "refs/heads/feature"),
        theirs,
        "theirs is untouched"
    );
}

#[test]
fn a_stopped_workspace_is_resumed_or_else_blocks_the_transfer() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "ahead.txt", "ahead\n");
    let head = commit(&worktree, "ahead");
    f.transfer(true).result.expect("to remote");
    write(&f.checkout(0), "remote-only.txt", "made remotely\n");

    // Stopped or failed, it may still hold work: resumed first.
    for (state, code) in [
        (WorkspaceState::Error, "workspace_error"),
        (WorkspaceState::Stopped, "workspace_stopped"),
    ] {
        f.host.set_state(&f.workspace(), state);
        let preflight = f.preflight();
        assert!(preflight.can_transfer, "{:?}", preflight.blockers);
        assert_eq!(codes(&preflight.warnings), vec![code]);
        assert_eq!(preflight.repos[0].state, RepoTransferState::Unknown);
    }

    // It won't come back: the transfer stops rather than leave its work.
    f.host.arm(Fault::ResumeWorkspace);
    let db = f.db_state();
    let err = f.transfer(true).result.unwrap_err();
    assert!(f.host.fired());
    assert!(err.contains("still stopped"), "{err}");
    assert!(err.contains("injected failure"), "{err}");
    assert_eq!(f.db_state(), db);
    assert_eq!(f.host.workspaces(), vec![f.workspace()]);

    // It comes back: carried like any other, then deleted.
    let result = f.transfer(true).result.expect("to local");
    assert!(result.repos[0].carried);
    let worktree = f.worktree(0);
    assert_eq!(rev(&worktree, "HEAD"), head);
    assert!(worktree.join("remote-only.txt").exists());
    assert!(f.host.workspaces().is_empty());
    let resume = format!("resume {}", f.workspace());
    assert_eq!(f.host.calls().iter().filter(|c| **c == resume).count(), 2);
}

// ---------------------------------------------------------------------------
// Tearing down
// ---------------------------------------------------------------------------

/// A worktree touched between its snapshot and the teardown is kept, with a
/// warning saying why; the transfer still succeeds.
#[test]
fn a_worktree_that_changed_after_its_snapshot_is_kept() {
    type Mutation = (&'static str, fn(&Path));
    let mutations: [Mutation; 4] = [
        ("files changed", |wt| write(wt, "late.txt", "saved late\n")),
        ("the index changed", |wt| {
            git(wt, &["add", "notes.txt"]);
        }),
        ("HEAD moved", |wt| {
            git(wt, &["commit", "-q", "--allow-empty", "-m", "late"]);
        }),
        ("is no longer checked out", |wt| {
            git(wt, &["checkout", "-q", "-b", "elsewhere"]);
        }),
    ];
    for (what, mutate) in mutations {
        let f = Fixture::new(&[(WIDGETS, "feature")]);
        let worktree = f.worktree(0);
        write(&worktree, "notes.txt", "untracked\n");
        let result = f
            .transfer_with(true, &|progress| {
                if progress.phase == TransferPhase::Committing {
                    mutate(&worktree);
                }
            })
            .result
            .expect("the transfer succeeds");

        assert_eq!(result.location, ProjectLocation::Remote, "{what}");
        assert_eq!(codes(&result.warnings), vec!["worktree_left"], "{what}");
        assert!(
            result.warnings[0].message.contains(what),
            "{what}: {}",
            result.warnings[0].message
        );
        assert!(worktree.is_dir(), "{what}: the worktree is kept");
        assert_eq!(temp_refs(&worktree), "", "{what}");
        // The snapshot is kept too, as for any carried worktree.
        let backup_ref = result.repos[0].backup_ref.clone().expect("backup ref");
        git(
            &f.host.clone_of(WIDGETS),
            &["cat-file", "-e", &format!("{backup_ref}:notes.txt")],
        );
        assert!(
            !f.host
                .root
                .join("transfers")
                .join(&result.transfer_id)
                .join("0-widgets.bundle.index")
                .exists(),
            "{what}: the scratch index is removed"
        );
    }
}

#[test]
fn a_workspace_that_changed_after_its_snapshot_is_kept() {
    let f = two_repo_project();
    f.transfer(true).result.expect("to remote");
    let gadgets = f.checkout(1);
    let calls = f.host.calls();

    let to_local = f.transfer_with(true, &|progress| {
        if progress.phase == TransferPhase::Committing {
            write(&gadgets, "late.txt", "saved late\n");
        }
    });
    let result = to_local.result.expect("the transfer succeeds");

    assert_eq!(result.location, ProjectLocation::Local);
    assert_eq!(codes(&result.warnings), vec!["workspace_left"]);
    let message = &result.warnings[0].message;
    assert!(message.contains("gizmo: files changed"), "{message}");
    assert_eq!(f.host.calls(), calls, "not deleted");
    assert!(gadgets.join("late.txt").exists());
    assert_eq!(temp_refs(&gadgets), "");
    assert!(!f
        .host
        .workspace_dir(&f.workspace())
        .join(".staged-transfer")
        .join(&to_local.transfer_id)
        .exists());
    // Both snapshots are kept in the local clones all the same.
    for (i, repo) in result.repos.iter().enumerate() {
        let backup_ref = repo.backup_ref.as_deref().expect("backup ref");
        rev(&f.host.clone_of(&f.repo(i)), backup_ref);
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

#[test]
fn preflight_counts_changes_and_reports_blockers_and_warnings() {
    let f = Fixture::new(&[(WIDGETS, "feature"), (GADGETS, "gizmo"), (SPROCKETS, "cog")]);
    let widgets = f.worktree(0);
    git(&widgets, &["push", "-q", "-u", "origin", "feature"]);
    write(&widgets, "ahead-1.txt", "one\n");
    commit(&widgets, "ahead 1");
    write(&widgets, "ahead-2.txt", "two\n");
    commit(&widgets, "ahead 2");
    make_a_mess(&widgets);
    // Ignored folders count once each, ignored files anywhere else each.
    fs::write(
        f.host.clone_of(WIDGETS).join(".git/info/exclude"),
        "node_modules/\n.venv/\n",
    )
    .unwrap();
    write(&widgets, "node_modules/a/index.js", "x\n");
    write(&widgets, "node_modules/b.js", "x\n");
    write(&widgets, ".venv/bin/python", "x\n");
    write(&widgets, "logs/one.log", "x\n");
    write(&widgets, "logs/two.log", "x\n");

    let gadgets = f.worktree(1);
    write(
        &gadgets,
        ".gitmodules",
        "[submodule \"lib\"]\n\tpath = lib\n\turl = ../lib.git\n",
    );
    write(
        &gadgets,
        ".gitattributes",
        "*.psd filter=lfs diff=lfs merge=lfs -text\n",
    );
    commit(&gadgets, "attributes");
    git(&gadgets, &["checkout", "-q", "--detach"]);
    // A clone inside the checkout: a snapshot could only record its commit.
    let vendored = gadgets.join("vendor").join("lib");
    fs::create_dir_all(&vendored).unwrap();
    git(&vendored, &["init", "-q"]);
    write(&vendored, "lib.txt", "vendored\n");

    let sprockets = f.worktree(2);
    git(&sprockets, &["checkout", "-q", "-b", "side"]);
    write(&sprockets, "a.txt", "side\n");
    commit(&sprockets, "side");
    git(&sprockets, &["checkout", "-q", "cog"]);
    write(&sprockets, "a.txt", "cog\n");
    commit(&sprockets, "cog");
    assert!(try_git(&sprockets, &["merge", "-q", "side"]).is_err());

    let before: Vec<_> = (0..3).map(|i| source_fingerprint(&f.worktree(i))).collect();
    let preflight = f.preflight();
    let after: Vec<_> = (0..3).map(|i| source_fingerprint(&f.worktree(i))).collect();
    assert_eq!(before, after, "the preflight writes nothing");

    let widgets = &preflight.repos[0];
    assert_eq!(widgets.state, RepoTransferState::Carry);
    assert_eq!(
        (
            widgets.staged,
            widgets.unstaged,
            widgets.untracked,
            widgets.conflicted
        ),
        (6, 3, 4, 0)
    );
    assert!(widgets.untracked_bytes >= 1000);
    assert_eq!(
        (widgets.ignored_files, widgets.ignored_dirs),
        (3, 2),
        "debug.log and logs/*.log; node_modules/ and .venv/"
    );
    assert_eq!(widgets.unpushed_commits, 2);
    assert_eq!(widgets.upstream.as_deref(), Some("origin/feature"));
    assert_eq!(widgets.head, Some(rev(&f.worktree(0), "HEAD")));
    assert!(widgets.has_uncommitted_changes);
    assert!(widgets.blockers.is_empty(), "{:?}", widgets.blockers);

    let gadgets = &preflight.repos[1];
    assert_eq!(
        codes(&gadgets.blockers),
        vec!["detached_head", "nested_repo"]
    );
    assert!(gadgets.blockers[1].message.starts_with("vendor/lib/ "));
    assert_eq!(codes(&gadgets.warnings), vec!["submodules"]);
    // Set up for LFS, but nothing uses it yet.
    assert_eq!((gadgets.lfs_present, gadgets.lfs_missing), (0, 0));
    // The nested repository is the blocker, not an untracked file.
    assert!(!gadgets.has_uncommitted_changes);
    assert_eq!((gadgets.ignored_files, gadgets.ignored_dirs), (0, 0));

    let sprockets = &preflight.repos[2];
    assert_eq!(sprockets.conflicted, 1);
    assert_eq!(
        codes(&sprockets.blockers),
        vec!["operation_in_progress", "conflicts"]
    );
    assert!(!preflight.can_transfer);
    assert!(preflight.has_uncommitted_changes);

    // Sessions, Blox and a transfer already running block it too.
    add_running_session(&f.store, &f.branches[0].id);
    *f.host.blox.lock().unwrap() = Some(BloxUnavailable {
        code: "blox_not_authenticated",
        message: "Not authenticated with Blox.".to_string(),
    });
    let guard = registry::begin(
        &f.project.id,
        "running-transfer",
        TransferDirection::ToRemote,
        vec![],
    )
    .unwrap();
    let preflight = f.preflight();
    drop(guard);
    assert_eq!(
        codes(&preflight.blockers),
        vec!["blox_not_authenticated", "already_transferring"]
    );
    assert_eq!(codes(&preflight.repos[0].blockers), vec!["session_running"]);
}

/// The preflight comes in steps: first the project-level checks with every
/// checkout still to read, then each checkout's status and content parts as
/// they land, in any order; the last step reads the same as a one-shot run.
#[test]
fn preflight_streams_each_checkout_as_its_parts_land() {
    let f = Fixture::new(&[(WIDGETS, "feature"), (GADGETS, "gizmo")]);
    let widgets = f.worktree(0);
    write(&widgets, "ahead.txt", "one\n");
    commit(&widgets, "ahead");
    make_a_mess(&widgets);
    fs::write(
        f.host.clone_of(WIDGETS).join(".git/info/exclude"),
        "node_modules/\n",
    )
    .unwrap();
    write(&widgets, "node_modules/a/index.js", "x\n");

    let steps: Mutex<Vec<ProjectTransferPreflight>> = Mutex::new(Vec::new());
    let on_partial = |p: &ProjectTransferPreflight| steps.lock().unwrap().push(p.clone());
    let inspection = inspect_after_resuming(
        &f.store,
        &f.host,
        &f.project.id,
        &Default::default(),
        &PreflightObserver {
            on_partial: &on_partial,
            cancelled: &|| false,
        },
    )
    .unwrap();
    let steps = steps.into_inner().unwrap();

    // One step before any read, then one per part per checkout.
    assert_eq!(steps.len(), 1 + 2 * 2, "{:?}", steps.len());
    let first = &steps[0];
    assert!(first.scanning);
    assert!(first.repos.iter().all(|r| r.scanning));
    assert!(first.can_transfer, "nothing known yet blocks it");
    assert!(!first.has_uncommitted_changes);
    // Each step knows at least as much as the one before.
    for pair in steps.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        for (b, a) in before.repos.iter().zip(&after.repos) {
            assert!(b.scanning || !a.scanning, "a row read stays read");
            assert!(a.staged >= b.staged && a.ignored_files >= b.ignored_files);
        }
    }
    let last = steps.last().unwrap();
    assert!(!last.scanning);
    assert!(last.repos.iter().all(|r| !r.scanning));
    assert!(last.has_uncommitted_changes);
    let widgets = &last.repos[0];
    assert_eq!(widgets.state, RepoTransferState::Carry);
    assert_eq!(widgets.unpushed_commits, 1);
    assert_eq!(widgets.ignored_dirs, 1);
    assert_eq!(last.repos[1].state, RepoTransferState::Carry);
    assert!(!last.repos[1].has_uncommitted_changes);

    // The same as read in one go.
    let one_shot = f.preflight();
    assert_eq!(
        serde_json::to_value(last).unwrap(),
        serde_json::to_value(&one_shot).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&inspection.preflight).unwrap(),
        serde_json::to_value(&one_shot).unwrap()
    );
    assert_eq!(
        inspection.branches[0].source_dir.as_deref(),
        Some(f.worktree(0).to_str().unwrap())
    );
}

/// Asked to stop, the inspection reads no more checkouts and fails as
/// cancelled, the same way a cancelled transfer does.
#[test]
fn a_cancelled_preflight_stops_reading_and_says_so() {
    let f = Fixture::new(&[(WIDGETS, "feature"), (GADGETS, "gizmo")]);
    let steps = Mutex::new(0usize);
    let on_partial = |_: &ProjectTransferPreflight| *steps.lock().unwrap() += 1;
    let cancelled = || *steps.lock().unwrap() >= 1;
    let result = inspect_after_resuming(
        &f.store,
        &f.host,
        &f.project.id,
        &Default::default(),
        &PreflightObserver {
            on_partial: &on_partial,
            cancelled: &cancelled,
        },
    );
    assert_eq!(result.err(), Some(TransferError::Cancelled.to_string()));
    assert_eq!(*steps.lock().unwrap(), 1, "no checkout was read");
}

/// A dialog left open while another window moved the project asks for the
/// direction it showed. That is refused before anything happens, so the
/// project isn't moved straight back and what the other transfer made stays.
#[test]
fn a_transfer_asked_for_in_a_direction_the_project_no_longer_goes_is_refused() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let transfers = || {
        fs::read_dir(f.host.root.join("transfers"))
            .map(|dir| dir.count())
            .unwrap_or(0)
    };

    // Local, asked to come back from Blox.
    let before = f.db_state();
    let stale = f.transfer_as(
        TransferDirection::ToLocal,
        TransferOptions::default(),
        &|_| {},
    );
    let err = stale.result.unwrap_err();
    assert_eq!(
        err,
        "The project is already local; it was transferred after this dialog opened, so \
         nothing changed."
    );
    assert!(stale.events.is_empty(), "{:?}", stale.events);
    assert_eq!(f.db_state(), before);
    assert!(f.host.calls().is_empty());
    assert_eq!(transfers(), 0, "no manifest");
    assert!(f.worktree(0).is_dir());

    // On Blox, asked to go to Blox again: the workspace it went to stays.
    f.transfer(true).result.expect("to remote");
    let (before, calls, made) = (f.db_state(), f.host.calls(), transfers());
    let stale = f.transfer_as(
        TransferDirection::ToRemote,
        TransferOptions::default(),
        &|_| {},
    );
    let err = stale.result.unwrap_err();
    assert!(err.starts_with("The project is already on Blox;"), "{err}");
    assert!(err.contains(super::orchestrator::STALE_DIRECTION), "{err}");
    assert!(stale.events.is_empty(), "{:?}", stale.events);
    assert_eq!(f.db_state(), before);
    assert_eq!(f.host.calls(), calls);
    assert_eq!(f.host.workspaces(), vec![f.workspace()]);
    assert_eq!(transfers(), made);
    assert!(!registry::is_project_transferring(&f.project.id));

    // Asked for the way it does go, it goes.
    let back = f.transfer_as(
        TransferDirection::ToLocal,
        TransferOptions::default(),
        &|_| {},
    );
    assert_eq!(
        back.result.expect("to local").location,
        ProjectLocation::Local
    );
}

/// A long branch name with a multibyte character across byte 32 names the
/// workspace it goes to: that used to panic, failing the transfer. The name
/// is the one every other launch derives — cut down, ASCII, hash-suffixed.
#[test]
fn a_long_unicode_branch_name_names_its_workspace_in_ascii() {
    let branch = format!("{}é-über/naïve", "a".repeat(27));
    let f = Fixture::new(&[(WIDGETS, branch.as_str())]);
    write(&f.worktree(0), "notes.txt", "untracked\n");

    let result = f.transfer(true).result.expect("to remote");

    let workspace = crate::branches::infer_workspace_name(&f.project.id, &branch);
    assert!(
        workspace.starts_with(&format!("{}-", "a".repeat(25))),
        "{workspace}"
    );
    assert_eq!(workspace.len(), crate::branches::WORKSPACE_NAME_MAX_LENGTH);
    assert!(workspace.is_ascii(), "{workspace}");
    assert_eq!(result.workspace_name.as_deref(), Some(workspace.as_str()));
    assert_eq!(f.host.workspaces(), vec![workspace]);
    assert_eq!(result.repos[0].branch_name, branch);
    assert!(f.checkout(0).join("notes.txt").is_file());
}

/// When the derived name is taken, the transfer moves on to the next
/// ordinal's name, which keeps the `{readable}-{hash}` shape. Appending `-2`
/// used to eat into the hash once a long branch name had the base at the
/// length limit (`…-2f46-2`, `…-2f4-10`).
#[test]
fn a_taken_workspace_name_falls_through_to_the_next_ordinal_in_full_shape() {
    use crate::branches::{
        infer_workspace_name_nth, WORKSPACE_NAME_HASH_LENGTH, WORKSPACE_NAME_MAX_LENGTH,
    };
    let branch = "agent-flows-point-nowhere-pr-1";
    let f = Fixture::new(&[(WIDGETS, branch)]);
    let candidates: Vec<String> = (0..3)
        .map(|n| infer_workspace_name_nth(&f.project.id, branch, n))
        .collect();
    // Someone else holds the first two.
    f.host.set_state(&candidates[0], WorkspaceState::Running);
    f.host.set_state(&candidates[1], WorkspaceState::Suspended);

    let result = f.transfer(true).result.expect("to remote");

    assert_eq!(
        result.workspace_name.as_deref(),
        Some(candidates[2].as_str())
    );
    assert_eq!(f.host.workspaces(), vec![candidates[2].clone()]);
    for name in &candidates {
        assert_eq!(name.len(), WORKSPACE_NAME_MAX_LENGTH, "{name}");
        assert!(
            name.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "{name}"
        );
        assert!(!name.contains("--") && !name.ends_with('-'), "{name}");
        let hash = name.strip_prefix("agent-flows-point-nowhere-").expect(name);
        assert_eq!(hash.len(), WORKSPACE_NAME_HASH_LENGTH, "{name}");
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()), "{name}");
    }
    assert_ne!(candidates[0], candidates[1]);
    assert_ne!(candidates[1], candidates[2]);
    assert_ne!(candidates[0], candidates[2]);
    assert_eq!(result.repos[0].branch_name, branch);
}

#[test]
fn a_remote_project_needs_one_branch_per_repository() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let repo_id = f.branches[0].project_repo_id.clone().unwrap();
    let second = Branch::new(&f.project.id, "second", "origin/main").with_project_repo(&repo_id);
    f.store.create_branch(&second).unwrap();

    let preflight = f.preflight();
    assert!(!preflight.can_transfer);
    assert_eq!(
        codes(&preflight.blockers),
        vec!["multiple_branches_per_repo"]
    );
    let transfer = f.transfer(true);
    assert!(transfer.result.unwrap_err().contains("same repository"));
    assert!(f.host.calls().is_empty());
}

/// A branch with no project repo is on the primary repository, so it counts
/// against the branch that names that repository.
#[test]
fn a_branch_without_a_project_repo_counts_against_the_primary_one() {
    let f = Fixture::new(&[(WIDGETS, "feature"), (GADGETS, "gizmo")]);
    let loose = Branch::new(&f.project.id, "loose", "origin/main");
    f.store.create_branch(&loose).unwrap();

    let preflight = f.preflight();

    assert!(!preflight.can_transfer);
    assert_eq!(
        codes(&preflight.blockers),
        vec!["multiple_branches_per_repo"]
    );
    assert!(
        preflight.blockers[0].message.starts_with("feature, loose "),
        "{}",
        preflight.blockers[0].message
    );
}

/// Sessions queued on a branch, and replies held for its sessions, don't
/// block a transfer: they wait for it, move with the branch, and are what
/// the transfer's after-work drains. When it fails, its error says they are
/// still there.
#[test]
fn queued_sessions_and_held_replies_wait_for_the_transfer_and_move_with_it() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let branch_id = f.branches[0].id.clone();
    let queued = Session::new_queued("start later").with_branch(&branch_id);
    f.store.create_session(&queued).unwrap();
    let finished = Session::new_running("done", &f.worktree(0)).with_branch(&branch_id);
    f.store.create_session(&finished).unwrap();
    f.store
        .update_session_status(
            &finished.id,
            crate::store::SessionStatus::Completed,
            None,
            None,
        )
        .unwrap();
    let held = f
        .store
        .add_queued_session_message_held_for_transfer(
            &finished.id,
            "and then this",
            &[],
            Some(&branch_id),
        )
        .unwrap();

    let preflight = f.preflight();
    assert!(preflight.can_transfer, "{:?}", preflight.repos[0].blockers);
    assert!(codes(&preflight.repos[0].blockers).is_empty());

    // A failure leaves them queued, says so, and — once the project is free
    // again — has the command set them going where the project still is.
    f.host.arm(Fault::Script(Side::Workspace, APPLY_SCRIPT, 1));
    let failed = f.transfer(true);
    assert!(failed.held_work_released);
    assert!(failed.after_switch.is_none());
    let err = failed.result.unwrap_err();
    assert!(
        err.ends_with(
            "1 session and 1 reply queued for the transfer are going ahead where the project is."
        ),
        "{err}"
    );
    let still_queued = f.store.get_session(&queued.id).unwrap().unwrap();
    assert_eq!(still_queued.status, crate::store::SessionStatus::Queued);
    assert_eq!(
        f.store
            .session_ids_with_queued_messages_for_branch(&branch_id)
            .unwrap(),
        vec![finished.id.clone()]
    );

    // A success moves them with the branch: the queued row is the branch's
    // to start at the destination, the reply is there to be drained.
    let moved_with = f.transfer(true);
    assert!(!moved_with.held_work_released);
    assert!(moved_with.after_switch.is_some());
    let result = moved_with.result.expect("to remote");
    assert_eq!(result.location, ProjectLocation::Remote);
    let moved = f.store.get_session(&queued.id).unwrap().unwrap();
    assert_eq!(moved.status, crate::store::SessionStatus::Queued);
    assert_eq!(moved.branch_id.as_deref(), Some(branch_id.as_str()));
    assert_eq!(
        f.store
            .session_ids_with_queued_messages_for_branch(&branch_id)
            .unwrap(),
        vec![finished.id.clone()]
    );
    let replies = f.store.list_queued_session_messages(&finished.id).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].id, held.id);
    assert_eq!(
        replies[0].status,
        crate::store::QueuedSessionMessageStatus::Queued
    );
}

/// The legs of a multi-repo transfer run at once: each repository's
/// snapshot starts while the other's is still running. (Each leg's
/// snapshot waits here for the other's to start; one leg after another
/// would wait for the whole timeout, then fail.)
#[test]
fn the_legs_of_a_transfer_run_at_once() {
    use std::sync::Condvar;
    use std::time::Duration;

    fn install_barrier(f: &Fixture, side: Side, overlapped: &Arc<Mutex<Option<bool>>>) {
        let gate = Arc::new((Mutex::new(0usize), Condvar::new()));
        let overlapped = Arc::clone(overlapped);
        f.host.on_every_script(move |at, script| {
            if at != side || script != SNAPSHOT_SCRIPT {
                return;
            }
            let (count, cv) = &*gate;
            let mut started = count.lock().unwrap();
            *started += 1;
            let (guard, timed_out) = cv
                .wait_timeout_while(started, Duration::from_secs(20), |n| *n < 2)
                .unwrap();
            drop(guard);
            cv.notify_all();
            let mut overlapped = overlapped.lock().unwrap();
            *overlapped = Some(overlapped.unwrap_or(true) && !timed_out.timed_out());
        });
    }

    let f = two_repo_project();
    let overlapped = Arc::new(Mutex::new(None));
    install_barrier(&f, Side::Local, &overlapped);
    let to_remote = f.transfer(true);
    to_remote.result.expect("to remote");
    assert_eq!(*overlapped.lock().unwrap(), Some(true), "to remote");
    assert_eq!(f.bundles(&to_remote.transfer_id), 2);

    // Events from both legs come out interleaved, each saying whose it is.
    let carrying: Vec<&TransferProgress> = to_remote
        .events
        .iter()
        .filter(|e| matches!(e.phase, TransferPhase::Snapshotting | TransferPhase::Moving))
        .collect();
    assert!(carrying.iter().all(|e| e.branch_id.is_some()));
    assert_eq!(
        carrying
            .iter()
            .filter_map(|e| e.branch_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2
    );

    // And back: the snapshots run in the workspace.
    let overlapped = Arc::new(Mutex::new(None));
    install_barrier(&f, Side::Workspace, &overlapped);
    let to_local = f.transfer(true);
    to_local.result.expect("to local");
    assert_eq!(*overlapped.lock().unwrap(), Some(true), "to local");
    assert_eq!(f.bundles(&to_local.transfer_id), 2);
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Trigger {
    Fault(Fault),
    /// Cancel on the first event of the phase.
    Cancel(TransferPhase),
    /// A session starts on the first branch just before the commit.
    SessionAtCommit,
    BloxUnavailable,
}

struct Case {
    name: &'static str,
    trigger: Trigger,
    /// Bundles kept on this machine. A range where the legs run at once:
    /// the other leg may or may not have made its bundle before the
    /// trigger stopped it.
    bundles: std::ops::RangeInclusive<usize>,
}

fn arm(f: &Fixture, trigger: Trigger) {
    match trigger {
        Trigger::Fault(fault) => f.host.arm(fault),
        Trigger::BloxUnavailable => {
            *f.host.blox.lock().unwrap() = Some(BloxUnavailable {
                code: "sq_unavailable",
                message: "sq is not installed".to_string(),
            })
        }
        Trigger::Cancel(_) | Trigger::SessionAtCommit => {}
    }
}

fn fail_with(f: &Fixture, case: &Case) -> Transfer {
    arm(f, case.trigger);
    let fired = Mutex::new(false);
    let transfer = f.transfer_with(true, &|progress| match case.trigger {
        Trigger::Cancel(phase) if progress.phase == phase && !*fired.lock().unwrap() => {
            assert!(registry::cancel(&progress.project_id));
            *fired.lock().unwrap() = true;
        }
        Trigger::SessionAtCommit if progress.phase == TransferPhase::Committing => {
            add_running_session(&f.store, &f.branches[0].id);
            *fired.lock().unwrap() = true;
        }
        _ => {}
    });
    let fired = match case.trigger {
        Trigger::Fault(_) => f.host.fired(),
        Trigger::BloxUnavailable => true,
        _ => *fired.lock().unwrap(),
    };
    assert!(fired, "{}: the trigger never fired", case.name);
    let err = transfer.result.as_ref().err().map(String::as_str);
    assert!(err.is_some(), "{}: the transfer succeeded", case.name);
    assert!(transfer.after_switch.is_none(), "{}", case.name);
    let phases = phases(&transfer.events);
    assert_eq!(phases.last(), Some(&TransferPhase::Failed), "{}", case.name);
    assert!(
        phases.contains(&TransferPhase::RollingBack),
        "{}",
        case.name
    );
    assert!(
        !phases.contains(&TransferPhase::TearingDown),
        "{}",
        case.name
    );
    let bundles = f.bundles(&transfer.transfer_id);
    assert!(
        case.bundles.contains(&bundles),
        "{}: {bundles} bundles, expected {:?}",
        case.name,
        case.bundles
    );
    if let Some(manifest) = f.manifest(&transfer.transfer_id) {
        assert_eq!(manifest.status, "rolledBack", "{}", case.name);
    }
    transfer
}

/// Fail a two-repo transfer to remote per `case`, and check nothing changed.
fn assert_to_remote_rolls_back(case: Case) {
    let f = two_repo_project();
    let db = f.db_state();
    let worktrees = [f.worktree(0), f.worktree(1)];
    let sources: Vec<_> = worktrees.iter().map(|wt| source_fingerprint(wt)).collect();
    let clones: Vec<_> = [WIDGETS, GADGETS]
        .iter()
        .map(|repo| clone_state(&f.host.clone_of(repo)))
        .collect();

    fail_with(&f, &case);

    assert_eq!(f.db_state(), db, "{}: database", case.name);
    for (wt, before) in worktrees.iter().zip(&sources) {
        assert_eq!(
            &source_fingerprint(wt),
            before,
            "{}: {}",
            case.name,
            wt.display()
        );
        assert_eq!(temp_refs(wt), "", "{}: temporary refs", case.name);
    }
    for (repo, before) in [WIDGETS, GADGETS].iter().zip(&clones) {
        assert_eq!(
            &clone_state(&f.host.clone_of(repo)),
            before,
            "{}: {repo}",
            case.name
        );
    }
    assert!(
        f.host.workspaces().is_empty(),
        "{}: workspace left",
        case.name
    );
}

/// Take a two-repo project to remote, then fail the way back per `case`, and
/// check nothing changed.
fn assert_to_local_rolls_back(case: Case) {
    let f = two_repo_project();
    f.transfer(true).result.expect("to remote");
    write(&f.checkout(0), "remote-only.txt", "made remotely\n");
    let db = f.db_state();
    let checkouts = [f.checkout(0), f.checkout(1)];
    let sources: Vec<_> = checkouts.iter().map(|c| source_fingerprint(c)).collect();
    let clones: Vec<_> = [WIDGETS, GADGETS]
        .iter()
        .map(|repo| clone_state(&f.host.clone_of(repo)))
        .collect();
    let calls = f.host.calls();

    let transfer = fail_with(&f, &case);

    assert_eq!(f.db_state(), db, "{}: database", case.name);
    for (checkout, before) in checkouts.iter().zip(&sources) {
        assert_eq!(
            &source_fingerprint(checkout),
            before,
            "{}: {}",
            case.name,
            checkout.display()
        );
        assert_eq!(temp_refs(checkout), "", "{}: temporary refs", case.name);
    }
    assert!(
        !f.host
            .workspace_dir(&f.workspace())
            .join(".staged-transfer")
            .join(&transfer.transfer_id)
            .exists(),
        "{}: workspace scratch",
        case.name
    );
    assert_eq!(
        f.host.calls(),
        calls,
        "{}: the workspace is kept",
        case.name
    );
    for (repo, before) in [WIDGETS, GADGETS].iter().zip(&clones) {
        assert_eq!(
            &clone_state(&f.host.clone_of(repo)),
            before,
            "{}: {repo}",
            case.name
        );
    }
    assert!(
        f.worktree_dirs().is_empty(),
        "{}: {:?}",
        case.name,
        f.worktree_dirs()
    );
}

#[test]
fn a_cancelled_transfer_to_remote_rolls_back_from_every_phase() {
    for (phase, bundles) in [
        (TransferPhase::Checking, 0..=0),
        (TransferPhase::PreparingDestination, 0..=0),
        // The first leg to snapshot is stopped before it does; the other
        // sees the cancel at its own snapshot.
        (TransferPhase::Snapshotting, 0..=0),
        (TransferPhase::Moving, 1..=2),
        (TransferPhase::Applying, 1..=2),
        (TransferPhase::Committing, 2..=2),
    ] {
        assert_to_remote_rolls_back(Case {
            name: "cancel",
            trigger: Trigger::Cancel(phase),
            bundles,
        });
    }
}

#[test]
fn a_failed_transfer_to_remote_rolls_back_from_every_phase() {
    use Side::*;
    for case in [
        Case {
            name: "blox unavailable",
            trigger: Trigger::BloxUnavailable,
            bundles: 0..=0,
        },
        Case {
            name: "inspect",
            trigger: Trigger::Fault(Fault::Script(Local, PREFLIGHT_SCRIPT, 1)),
            bundles: 0..=0,
        },
        Case {
            name: "start workspace",
            trigger: Trigger::Fault(Fault::StartWorkspace),
            bundles: 0..=0,
        },
        Case {
            name: "clone second repo",
            trigger: Trigger::Fault(Fault::Script(Workspace, CLONE_SCRIPT, 2)),
            bundles: 0..=0,
        },
        Case {
            name: "origin tips",
            trigger: Trigger::Fault(Fault::Script(Workspace, ORIGIN_TIPS_SCRIPT, 1)),
            bundles: 0..=0,
        },
        Case {
            name: "second snapshot",
            trigger: Trigger::Fault(Fault::Script(Local, SNAPSHOT_SCRIPT, 2)),
            bundles: 1..=1,
        },
        Case {
            name: "upload",
            trigger: Trigger::Fault(Fault::PutFile(Workspace)),
            bundles: 1..=2,
        },
        Case {
            name: "first apply",
            trigger: Trigger::Fault(Fault::Script(Workspace, APPLY_SCRIPT, 1)),
            bundles: 1..=2,
        },
        Case {
            name: "second apply",
            trigger: Trigger::Fault(Fault::Script(Workspace, APPLY_SCRIPT, 2)),
            bundles: 2..=2,
        },
        Case {
            name: "panic in the second apply",
            trigger: Trigger::Fault(Fault::Panic(Workspace, APPLY_SCRIPT, 2)),
            bundles: 2..=2,
        },
        Case {
            name: "session at commit",
            trigger: Trigger::SessionAtCommit,
            bundles: 2..=2,
        },
    ] {
        assert_to_remote_rolls_back(case);
    }
}

#[test]
fn a_cancelled_transfer_to_local_rolls_back_from_every_phase() {
    for (phase, bundles) in [
        (TransferPhase::Checking, 0..=0),
        (TransferPhase::PreparingDestination, 0..=0),
        (TransferPhase::Snapshotting, 0..=0),
        (TransferPhase::Moving, 0..=1),
        (TransferPhase::Applying, 1..=2),
        (TransferPhase::Committing, 2..=2),
    ] {
        assert_to_local_rolls_back(Case {
            name: "cancel",
            trigger: Trigger::Cancel(phase),
            bundles,
        });
    }
}

#[test]
fn a_failed_transfer_to_local_rolls_back_from_every_phase() {
    use Side::*;
    for case in [
        Case {
            name: "inspect",
            trigger: Trigger::Fault(Fault::Script(Workspace, PREFLIGHT_SCRIPT, 1)),
            bundles: 0..=0,
        },
        Case {
            name: "local clone",
            trigger: Trigger::Fault(Fault::EnsureClone),
            bundles: 0..=0,
        },
        Case {
            name: "second origin tips",
            trigger: Trigger::Fault(Fault::Script(Local, ORIGIN_TIPS_SCRIPT, 2)),
            bundles: 0..=0,
        },
        Case {
            name: "snapshot",
            trigger: Trigger::Fault(Fault::Script(Workspace, SNAPSHOT_SCRIPT, 1)),
            bundles: 0..=0,
        },
        Case {
            name: "download",
            trigger: Trigger::Fault(Fault::GetFile(Workspace)),
            bundles: 0..=1,
        },
        Case {
            name: "land the branch",
            trigger: Trigger::Fault(Fault::Script(Local, APPLY_SCRIPT, 1)),
            bundles: 1..=2,
        },
        Case {
            name: "finish in the worktree",
            trigger: Trigger::Fault(Fault::Script(Local, APPLY_SCRIPT, 2)),
            bundles: 1..=2,
        },
        Case {
            name: "panic in the worktree",
            trigger: Trigger::Fault(Fault::Panic(Local, APPLY_SCRIPT, 2)),
            bundles: 1..=2,
        },
        Case {
            name: "second repo",
            trigger: Trigger::Fault(Fault::Script(Local, APPLY_SCRIPT, 3)),
            bundles: 2..=2,
        },
        Case {
            name: "session at commit",
            trigger: Trigger::SessionAtCommit,
            bundles: 2..=2,
        },
    ] {
        assert_to_local_rolls_back(case);
    }
}

/// A local branch the transfer reused (already at the carried tip) is left
/// where it was on rollback, with its config as it had it.
#[test]
fn a_rolled_back_transfer_to_local_keeps_a_branch_it_reused() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "ahead.txt", "ahead\n");
    let head = commit(&worktree, "ahead");
    git(&worktree, &["push", "-q", "-u", "origin", "feature"]);
    f.transfer(true).result.expect("to remote");

    // Meanwhile `feature` was made locally again, at the same commit, with
    // config of its own.
    let clone = f.host.clone_of(WIDGETS);
    git(&clone, &["fetch", "-q", "origin"]);
    git(&clone, &["branch", "--no-track", "feature", &head]);
    git(&clone, &["config", "branch.feature.description", "mine"]);
    git(
        &clone,
        &["config", "branch.feature.pushRemote", "elsewhere"],
    );
    let config = || {
        git(
            &clone,
            &["config", "--local", "--get-regexp", "^branch\\.feature\\."],
        )
    };
    let (refs, before) = (clone_state(&clone), config());
    assert_eq!(f.preflight().repos[0].dest_branch_name, "feature", "reused");

    // Fail in the worktree, after the branch's config was replaced.
    f.host.arm(Fault::Script(Side::Local, APPLY_SCRIPT, 2));
    let err = f.transfer(true).result.unwrap_err();
    assert!(f.host.fired(), "{err}");

    assert_eq!(rev(&clone, "refs/heads/feature"), head, "still there");
    assert_eq!(config(), before);
    assert_eq!(clone_state(&clone), refs);
}

/// What a rollback deletes or puts back, it only touches as the transfer
/// left it: a branch at the tip it was given, a remote at the URL it was
/// given, and a branch's config only while the branch is gone or still at
/// the carried head — at another tip it, and its config, belong to whoever
/// moved it.
#[test]
fn a_rollback_only_touches_what_is_still_as_the_transfer_left_it() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let clone = f.host.clone_of(WIDGETS);
    let carried = rev(&clone, "origin/main");
    let other = rev(&clone, "HEAD");
    write(&f.worktree(0), "other.txt", "other\n");
    let other_tip = commit(&f.worktree(0), "other");
    assert_ne!(other, other_tip);
    let config = |branch: &str| {
        try_git(
            &clone,
            &[
                "config",
                "--local",
                "--get-regexp",
                &format!("^branch\\.{branch}\\."),
            ],
        )
        .unwrap_or_default()
    };
    let replaced = |branch: &str| ReplacedConfig {
        clone: clone.clone(),
        branch: branch.to_string(),
        head: carried.clone(),
        entries: vec![(
            format!("branch.{branch}.description"),
            "before the transfer".to_string(),
        )],
    };
    let undo = Undo {
        branches: ["at-tip", "moved", "never-made"]
            .map(|branch| CreatedBranch {
                clone: clone.clone(),
                branch: branch.to_string(),
                tip: carried.clone(),
            })
            .to_vec(),
        branch_configs: vec![replaced("at-tip"), replaced("moved"), replaced("reused")],
        remotes: ["fork", "theirs", "never-added"]
            .map(|remote| AddedRemote {
                clone: clone.clone(),
                remote: remote.to_string(),
                url: "https://example.com/fork.git".to_string(),
            })
            .to_vec(),
        ..Undo::default()
    };
    git(&clone, &["branch", "--no-track", "at-tip", &carried]);
    git(&clone, &["branch", "--no-track", "moved", &other_tip]);
    git(&clone, &["branch", "--no-track", "reused", &carried]);
    for branch in ["at-tip", "moved", "reused"] {
        git(
            &clone,
            &["config", &format!("branch.{branch}.remote"), "origin"],
        );
    }
    git(
        &clone,
        &["remote", "add", "fork", "https://example.com/fork.git"],
    );
    git(
        &clone,
        &["remote", "add", "theirs", "https://example.com/theirs.git"],
    );

    let left = undo::roll_back(&undo, &f.host, "t");

    assert!(try_git(&clone, &["rev-parse", "--verify", "refs/heads/at-tip"]).is_err());
    assert_eq!(rev(&clone, "refs/heads/moved"), other_tip);
    assert_eq!(rev(&clone, "refs/heads/reused"), carried);
    assert_eq!(
        config("at-tip"),
        "branch.at-tip.description before the transfer\n"
    );
    assert_eq!(config("moved"), "branch.moved.remote origin\n", "untouched");
    assert_eq!(
        config("reused"),
        "branch.reused.description before the transfer\n"
    );
    assert!(undo::remote_url(&clone, "fork").is_none());
    assert_eq!(
        undo::remote_url(&clone, "theirs").as_deref(),
        Some("https://example.com/theirs.git")
    );
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left[0].contains("kept branch moved"), "{left:?}");
    assert!(left[1].contains("kept remote theirs"), "{left:?}");
}

// ---------------------------------------------------------------------------
// The destination's own state
// ---------------------------------------------------------------------------

/// A workspace's bootstrap can leave its clone changed. Tracked changes stop
/// the transfer before anything is snapshotted; untracked files only when
/// the branch would write over one, and otherwise stay with a warning.
#[test]
fn what_a_workspace_bootstrap_left_in_the_clone_is_checked_before_anything_moves() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    let worktree = f.worktree(0);
    write(&worktree, "notes.txt", "untracked\n");
    let source = source_fingerprint(&worktree);
    let db = f.db_state();

    f.host
        .bootstrap_with(|clone| write(clone, "a.txt", "bootstrapped\n"));
    let dirty = f.transfer(true);
    let err = dirty.result.unwrap_err();
    assert!(
        err.contains("already has changes to tracked files (a.txt)"),
        "{err}"
    );
    let phases = phases(&dirty.events);
    assert!(!phases.contains(&TransferPhase::Snapshotting), "{phases:?}");
    assert_eq!(f.bundles(&dirty.transfer_id), 0);

    f.host
        .bootstrap_with(|clone| write(clone, "notes.txt", "bootstrapped\n"));
    let err = f.transfer(true).result.unwrap_err();
    assert!(
        err.contains("has an untracked notes.txt, which the branch would write over"),
        "{err}"
    );
    assert_eq!(f.db_state(), db);
    assert_eq!(source_fingerprint(&worktree), source);
    assert!(f.host.workspaces().is_empty(), "rolled back");

    f.host
        .bootstrap_with(|clone| write(clone, ".bootstrapped", "yes\n"));
    let result = f.transfer(true).result.expect("to remote");
    let warnings: Vec<&str> = result.repos[0]
        .warnings
        .iter()
        .map(|w| w.message.as_str())
        .collect();
    assert!(
        warnings.contains(
            &"1 untracked file was already in the destination checkout \
              (.bootstrapped, for one) and stayed there"
        ),
        "{warnings:?}"
    );
    assert!(f.checkout(0).join(".bootstrapped").exists());
}

// ---------------------------------------------------------------------------
// Resuming the source
// ---------------------------------------------------------------------------

/// A cancel stops a resume that is still waiting; a resume that finished
/// leaves the rows running, whatever the transfer comes to.
#[test]
fn a_resume_honours_cancel_and_leaves_the_rows_running() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    f.transfer(true).result.expect("to remote");
    let suspend = || {
        f.host.set_state(&f.workspace(), WorkspaceState::Suspended);
        f.store
            .update_workspace_status_by_workspace_name(&f.workspace(), &WorkspaceStatus::Suspended)
            .unwrap();
    };
    let status = || {
        f.store
            .get_branch(&f.branches[0].id)
            .unwrap()
            .unwrap()
            .workspace_status
    };
    let resume = format!("resume {}", f.workspace());

    suspend();
    f.host.arm(Fault::ResumeHangs);
    let hung = std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !f.host.calls().contains(&resume) {
                assert!(std::time::Instant::now() < deadline, "never resumed");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(registry::cancel(&f.project.id));
        });
        f.transfer(true)
    });
    assert!(f.host.fired());
    let err = hung.result.unwrap_err();
    assert!(err.contains("transfer was cancelled"), "{err}");
    assert_eq!(status(), Some(WorkspaceStatus::Suspended));

    let cancelled = f.transfer_with(true, &|progress| {
        if progress.phase == TransferPhase::Snapshotting {
            registry::cancel(&progress.project_id);
        }
    });
    assert!(cancelled.result.is_err());
    assert_eq!(f.host.calls().iter().filter(|c| **c == resume).count(), 2);
    assert_eq!(status(), Some(WorkspaceStatus::Running), "resumed");
}

// ---------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------

/// Past the switch there is nothing to roll back to: a panic while the
/// source comes down leaves the project where it went, and frees everything
/// it held. The transfer is reported as one that moved, with a warning
/// saying it stopped, and what follows a switch — prerun actions in new
/// local worktrees, among the rest — still runs on every branch.
#[test]
fn a_panic_after_the_switch_leaves_the_project_at_its_destination() {
    let f = two_repo_project();
    let worktrees = [f.worktree(0), f.worktree(1)];
    // The first worktree's check before it is removed.
    f.host.arm(Fault::Panic(Side::Local, VERIFY_SCRIPT, 1));

    let transfer = f.transfer(true);

    assert!(f.host.fired());
    let result = transfer.result.expect("reported as moved");
    assert_eq!(result.location, ProjectLocation::Remote);
    assert_eq!(codes(&result.warnings), vec!["teardown_interrupted"]);
    let warning = &result.warnings[0].message;
    assert!(warning.contains("The project moved"), "{warning}");
    assert!(warning.contains("injected panic"), "{warning}");
    let after = transfer.after_switch.expect("the after-transfer work ran");
    let ids = |r: &ProjectTransferResult| -> Vec<String> {
        r.repos.iter().map(|r| r.branch_id.clone()).collect()
    };
    assert_eq!(ids(&after), branch_ids_of(&f));
    assert!(after.repos.iter().all(|r| r.carried));
    assert!(
        !after.repos.iter().any(|r| runs_prerun(&after, r)),
        "remote"
    );
    let phases = phases(&transfer.events);
    assert_eq!(phases.last(), Some(&TransferPhase::Completed));
    assert!(!phases.contains(&TransferPhase::RollingBack), "{phases:?}");
    let project = f.store.get_project(&f.project.id).unwrap().unwrap();
    assert_eq!(project.location, ProjectLocation::Remote);
    assert_eq!(f.host.workspaces(), vec![f.workspace()]);
    // The leg that panicked left its worktree; the other, running at the
    // same time, may have finished removing its own.
    assert!(worktrees.iter().any(|wt| wt.is_dir()), "not removed yet");
    let manifest = f.manifest(&transfer.transfer_id).expect("manifest");
    assert_eq!(manifest.status, backups::COMPLETED);
    assert!(
        manifest.warnings[0].contains("injected panic"),
        "{manifest:?}"
    );
    for branch in &f.branches {
        let lock = crate::session_commands::branch_session_launch_lock_for(&branch.id);
        assert!(lock.lock().is_ok());
    }
    assert!(recovery::recover(&f.store, &f.host, &|_| false).is_empty());

    // To local, the carried branches have their new worktrees, so their
    // prerun actions run.
    let f = two_repo_project();
    f.transfer(true).result.expect("to remote");
    // The workspace's checks before it is deleted.
    f.host.arm(Fault::Panic(Side::Workspace, VERIFY_SCRIPT, 1));

    let transfer = f.transfer(true);

    assert!(f.host.fired());
    let result = transfer.result.expect("reported as moved");
    assert_eq!(result.location, ProjectLocation::Local);
    assert_eq!(codes(&result.warnings), vec!["teardown_interrupted"]);
    let after = transfer.after_switch.expect("the after-transfer work ran");
    assert_eq!(ids(&after), branch_ids_of(&f));
    assert!(after.repos.iter().all(|r| runs_prerun(&after, r)));
    for i in 0..f.branches.len() {
        assert!(f.worktree(i).is_dir());
    }
    assert_eq!(f.host.workspaces(), vec![f.workspace()], "kept");
    assert_eq!(
        transfer.events.last().map(|e| e.phase),
        Some(TransferPhase::Completed)
    );
}

fn branch_ids_of(f: &Fixture) -> Vec<String> {
    f.branches.iter().map(|b| b.id.clone()).collect()
}

// ---------------------------------------------------------------------------
// Interrupted transfers
// ---------------------------------------------------------------------------

/// The manifest lists what to undo before each thing is made, so a transfer
/// cut off part way — here, between the two repos — is rolled back at the
/// next start: the orphan workspace and the temporary refs.
#[test]
fn a_transfer_to_remote_cut_off_before_the_switch_is_undone_at_the_next_start() {
    let f = two_repo_project();
    // One leg after another, so that there is a moment between the repos.
    let _serial = SerialLegs::for_this_thread();
    let worktrees = [f.worktree(0), f.worktree(1)];
    let sources: Vec<_> = worktrees.iter().map(|wt| source_fingerprint(wt)).collect();
    let checked = Mutex::new(false);

    let transfer = f.transfer_with(true, &|progress| {
        let manifest = || f.manifest(&progress.transfer_id);
        match progress.phase {
            // Nothing made yet, nothing written.
            TransferPhase::PreparingDestination if progress.detail.is_none() => {
                assert!(manifest().is_none());
            }
            // Recorded before the start.
            TransferPhase::PreparingDestination
                if progress.detail.as_deref() == Some("Workspace running") =>
            {
                let manifest = manifest().expect("written before the workspace");
                assert_eq!(manifest.status, backups::CARRYING);
                assert_eq!(manifest.owner_pid, Some(std::process::id()));
                assert_eq!(
                    manifest.workspace_name.as_deref(),
                    Some(f.workspace().as_str())
                );
                assert_eq!(
                    manifest.undo.workspace.as_deref(),
                    Some(f.workspace().as_str())
                );
            }
            TransferPhase::Applying if progress.branch_id.as_deref() == Some(&f.branches[1].id) => {
                let before = manifest().expect("manifest");
                assert_eq!(before.undo.sources.len(), 2);
                assert_eq!(before.repos.len(), 1, "the first repo is carried");
                assert!(recovery::is_running(&before), "registered here");

                let recovered = recovery::recover(&f.store, &f.host, &|_| false);

                assert_eq!(
                    recovered,
                    vec![(
                        progress.transfer_id.clone(),
                        Recovered::RolledBack { left: vec![] }
                    )]
                );
                assert!(f.host.workspaces().is_empty(), "the orphan is deleted");
                for (wt, before) in worktrees.iter().zip(&sources) {
                    assert_eq!(temp_refs(wt), "", "{}", wt.display());
                    assert_eq!(&source_fingerprint(wt), before);
                }
                let after = manifest().expect("manifest");
                assert_eq!(after.status, backups::ROLLED_BACK);
                assert!(after.warnings[0].contains("undone at the next start"));
                *checked.lock().unwrap() = true;
                // Staged was "gone": this run must not carry on.
                registry::cancel(&progress.project_id);
            }
            _ => {}
        }
    });

    assert!(*checked.lock().unwrap());
    assert!(transfer.result.is_err());
}

#[test]
fn a_transfer_to_local_cut_off_before_the_switch_is_undone_at_the_next_start() {
    let f = two_repo_project();
    f.transfer(true).result.expect("to remote");
    let checkouts = [f.checkout(0), f.checkout(1)];
    let clones: Vec<_> = [WIDGETS, GADGETS]
        .iter()
        .map(|repo| clone_state(&f.host.clone_of(repo)))
        .collect();
    let checked = Mutex::new(false);

    let transfer = f.transfer_with(true, &|progress| {
        if progress.phase != TransferPhase::Committing {
            return;
        }
        let dir = f.host.root.join("transfers").join(&progress.transfer_id);
        let undo = backups::read_manifest(&dir).expect("manifest").undo;
        assert_eq!(
            (
                undo.sources.len(),
                undo.clones.len(),
                undo.branches.len(),
                undo.branch_configs.len(),
                undo.worktrees.len()
            ),
            (2, 2, 2, 2, 2)
        );
        assert!(!f.worktree_dirs().is_empty());

        let recovered = recovery::recover(&f.store, &f.host, &|_| false);

        assert_eq!(recovered.len(), 1);
        assert!(f.worktree_dirs().is_empty(), "{:?}", f.worktree_dirs());
        for (repo, before) in [WIDGETS, GADGETS].iter().zip(&clones) {
            assert_eq!(&clone_state(&f.host.clone_of(repo)), before, "{repo}");
        }
        for checkout in &checkouts {
            assert_eq!(temp_refs(checkout), "", "{}", checkout.display());
        }
        assert_eq!(f.host.workspaces(), vec![f.workspace()], "the source");
        *checked.lock().unwrap() = true;
        registry::cancel(&progress.project_id);
    });

    assert!(*checked.lock().unwrap());
    assert!(transfer.result.is_err());
}

/// Once the database has switched the destination is the project: an
/// unfinished manifest is only marked, saying what may be left.
#[test]
fn a_transfer_cut_off_after_the_switch_is_left_for_the_user() {
    let f = two_repo_project();
    let done = f.transfer(true);
    done.result.expect("to remote");
    let dir = f.host.root.join("transfers").join(&done.transfer_id);
    let mut manifest = backups::read_manifest(&dir).unwrap();
    assert!(!manifest.undo.sources.is_empty());
    // As if Staged had died while the worktrees came down.
    manifest.status = backups::COMMITTED.to_string();
    manifest.owner_pid = Some(dead_pid());
    backups::write_manifest(&dir, &manifest).unwrap();
    let calls = f.host.calls();
    assert!(!recovery::is_running(&manifest));

    let recovered = recovery::recover(&f.store, &f.host, &recovery::is_running);

    let [(tid, Recovered::Marked(warning))] = recovered.as_slice() else {
        panic!("{recovered:?}");
    };
    assert_eq!(tid, &done.transfer_id);
    assert!(
        warning.contains("after the project had switched over"),
        "{warning}"
    );
    assert!(warning.contains(&manifest.undo.sources[0].dir), "{warning}");
    assert_eq!(f.host.calls(), calls, "nothing deleted");
    assert_eq!(f.host.workspaces(), vec![f.workspace()]);
    let marked = backups::read_manifest(&dir).unwrap();
    assert_eq!(marked.status, backups::COMPLETED);
    assert_eq!(marked.warnings, vec![warning.clone()]);
    assert!(recovery::recover(&f.store, &f.host, &|_| false).is_empty());

    // Another Staged still running it is left alone.
    manifest.owner_pid = Some(std::os::unix::process::parent_id());
    assert!(recovery::is_running(&manifest));
}

/// A `committed` manifest switched the database, so a project back at its
/// source got there by a later transfer, and the undo list names what that
/// one kept: here the workspace it left because it changed. Only marked.
#[test]
fn a_committed_transfer_whose_project_moved_back_is_not_undone() {
    let f = two_repo_project();
    let first = f.transfer(true);
    first.result.expect("to remote");
    let dir = f.host.root.join("transfers").join(&first.transfer_id);
    let mut manifest = backups::read_manifest(&dir).unwrap();
    assert_eq!(
        manifest.undo.workspace.as_deref(),
        Some(f.workspace().as_str())
    );
    // As if Staged had died while the worktrees came down.
    manifest.status = backups::COMMITTED.to_string();
    manifest.owner_pid = Some(dead_pid());
    backups::write_manifest(&dir, &manifest).unwrap();
    // Moved back before any restart, keeping the workspace that changed.
    let gadgets = f.checkout(1);
    let back = f.transfer_with(true, &|progress| {
        if progress.phase == TransferPhase::Committing {
            write(&gadgets, "late.txt", "saved late\n");
        }
    });
    let back = back.result.expect("to local");
    assert_eq!(codes(&back.warnings), vec!["workspace_left"]);
    let project = f.store.get_project(&f.project.id).unwrap().unwrap();
    assert_eq!(
        project.location,
        ProjectLocation::Local,
        "the first's source"
    );
    let worktrees = [f.worktree(0), f.worktree(1)];
    let calls = f.host.calls();

    let recovered = recovery::recover(&f.store, &f.host, &recovery::is_running);

    let [(tid, Recovered::Marked(warning))] = recovered.as_slice() else {
        panic!("{recovered:?}");
    };
    assert_eq!(tid, &first.transfer_id);
    assert!(
        warning.contains("after the project had switched over"),
        "{warning}"
    );
    assert_eq!(f.host.calls(), calls, "nothing deleted");
    assert_eq!(f.host.workspaces(), vec![f.workspace()]);
    assert!(gadgets.join("late.txt").exists());
    assert!(worktrees.iter().all(|wt| wt.is_dir()));
    assert_eq!(
        backups::read_manifest(&dir).unwrap().status,
        backups::COMPLETED
    );
}

/// A process id no process has.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}
