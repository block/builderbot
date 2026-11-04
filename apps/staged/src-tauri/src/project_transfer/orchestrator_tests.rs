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
use super::commands::preflight_for;
use super::host::{BloxUnavailable, TransferHost, WorkspaceState};
use super::orchestrator::{run_transfer, ProjectTransferResult, TransferPhase, TransferProgress};
use super::preflight::{ProjectTransferPreflight, RepoTransferState, TransferIssue};
use super::registry;
use super::scripts::{
    APPLY_SCRIPT, CLONE_SCRIPT, ORIGIN_TIPS_SCRIPT, PREFLIGHT_SCRIPT, SNAPSHOT_SCRIPT,
};
use super::tests::{
    commit, configure_identity, git, make_a_mess, noise, observe, rev, source_fingerprint, try_git,
    write, Observed,
};
use super::{LocalEndpoint, ScriptOutput, TransferEndpoint, TransferError};
use crate::store::{
    Branch, BranchType, Project, ProjectLocation, ProjectRepo, Session, Store, Workdir,
    WorkspaceStatus,
};
use crate::test_utils::TempDir;

const WIDGETS: &str = "acme/widgets";
const GADGETS: &str = "acme/gadgets";
const SPROCKETS: &str = "acme/sprockets";
/// What the workspace is named after the primary branch, `feature`.
const WORKSPACE: &str = "stg-feature";

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
}

#[derive(Default)]
struct Faults {
    armed: Option<Fault>,
    seen: usize,
    fired: bool,
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
            Fault::Script(_, _, nth) => nth,
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
}

impl TransferEndpoint for FaultyEndpoint {
    fn describe(&self) -> String {
        format!("{:?}", self.side)
    }

    fn run_script(&self, script: &str, args: &[String]) -> Result<ScriptOutput, TransferError> {
        let hit =
            self.faults.lock().unwrap().hit(
                |f| matches!(f, Fault::Script(side, s, _) if *side == self.side && *s == script),
            );
        if hit {
            return Err(injected(self.describe()));
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

    fn endpoint(&self, env_dir: &Path, scratch_root: PathBuf, side: Side) -> FaultyEndpoint {
        FaultyEndpoint {
            inner: LocalEndpoint::new(env_dir, scratch_root),
            side,
            faults: Arc::clone(&self.faults),
        }
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
        progress("Workspace running");
        Ok(())
    }

    fn resume_workspace(&self, name: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("resume {name}"));
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
    result: Result<ProjectTransferResult, String>,
    events: Vec<TransferProgress>,
    transfer_id: String,
}

impl Fixture {
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
        hook: &dyn Fn(&TransferProgress),
    ) -> Transfer {
        let transfer_id = uuid::Uuid::new_v4().to_string();
        let branch_ids = self.branches.iter().map(|b| b.id.clone()).collect();
        let guard = registry::begin(&self.project.id, &transfer_id, branch_ids).unwrap();
        let events = Mutex::new(Vec::new());
        let result = run_transfer(
            &self.store,
            &self.host,
            &guard,
            include_uncommitted,
            &|progress| {
                events.lock().unwrap().push(progress.clone());
                hook(progress);
            },
        );
        drop(guard);
        assert!(!registry::is_project_transferring(&self.project.id));
        Transfer {
            result,
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
    let ids: Vec<String> = f.branches.iter().map(|b| b.id.clone()).collect();
    let to_remote = f.transfer_with(true, &|progress| {
        if progress.phase == TransferPhase::Snapshotting {
            // Everything that would start work on the project refuses.
            assert!(registry::ensure_project_not_transferring(&f.project.id).is_err());
            for id in &ids {
                assert!(registry::ensure_branch_not_transferring(id).is_err());
            }
            *guarded.lock().unwrap() = true;
        }
    });
    let result = to_remote.result.expect("to remote");
    assert!(*guarded.lock().unwrap());

    // The database points at the workspace.
    assert_eq!(result.location, ProjectLocation::Remote);
    assert_eq!(result.workspace_name.as_deref(), Some(WORKSPACE));
    for branch in &f.branches {
        let branch = f.store.get_branch(&branch.id).unwrap().unwrap();
        assert_eq!(branch.branch_type, BranchType::Remote);
        assert_eq!(branch.workspace_name.as_deref(), Some(WORKSPACE));
        assert_eq!(branch.workspace_status, Some(WorkspaceStatus::Running));
        assert!(f
            .store
            .get_workdir_for_branch(&branch.id)
            .unwrap()
            .is_none());
    }
    assert_eq!(f.host.calls(), vec![format!("start {WORKSPACE}")]);

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
        .workspace_dir(WORKSPACE)
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
    assert_eq!(preflight.workspace_name.as_deref(), Some(WORKSPACE));
    assert_eq!(preflight.repos[0].untracked, 5);

    let result = f.transfer(true).result.expect("to local");
    assert_eq!(result.location, ProjectLocation::Local);
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
        vec![format!("start {WORKSPACE}"), format!("delete {WORKSPACE}")]
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
    assert_eq!(branch.workspace_name.as_deref(), Some(WORKSPACE));
    assert_eq!(
        branch.workspace_status,
        Some(WorkspaceStatus::Starting),
        "workspace setup provisions it"
    );

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
    f.host.set_state(WORKSPACE, WorkspaceState::Suspended);

    let preflight = f.preflight();
    assert!(preflight.can_transfer, "{:?}", preflight.blockers);
    assert_eq!(codes(&preflight.warnings), vec!["workspace_suspended"]);
    assert_eq!(preflight.repos[0].state, RepoTransferState::Unknown);
    assert_eq!(preflight.repos[0].dest_branch_name, "feature-2");
    assert_eq!(codes(&preflight.repos[0].warnings), vec!["renamed"]);

    let result = f.transfer(true).result.expect("to local");
    assert_eq!(result.repos[0].branch_name, "feature-2");
    assert_eq!(result.repos[0].previous_branch_name, "feature");
    assert!(f.host.calls().contains(&format!("resume {WORKSPACE}")));
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
fn a_stopped_workspace_is_left_and_its_branches_start_from_origin() {
    let f = Fixture::new(&[(WIDGETS, "feature")]);
    f.transfer(true).result.expect("to remote");
    f.host.set_state(WORKSPACE, WorkspaceState::Stopped);

    let preflight = f.preflight();
    assert!(preflight.can_transfer);
    assert_eq!(codes(&preflight.warnings), vec!["workspace_stopped"]);
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
    assert_eq!(f.host.workspaces(), vec![WORKSPACE.to_string()]);
    assert!(!f.host.calls().contains(&format!("delete {WORKSPACE}")));
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
    assert_eq!(widgets.unpushed_commits, 2);
    assert_eq!(widgets.upstream.as_deref(), Some("origin/feature"));
    assert_eq!(widgets.head, Some(rev(&f.worktree(0), "HEAD")));
    assert!(widgets.has_uncommitted_changes);
    assert!(widgets.blockers.is_empty(), "{:?}", widgets.blockers);

    let gadgets = &preflight.repos[1];
    assert_eq!(codes(&gadgets.blockers), vec!["detached_head"]);
    assert_eq!(codes(&gadgets.warnings), vec!["submodules", "lfs"]);
    assert!(!gadgets.has_uncommitted_changes);

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
    let guard = registry::begin(&f.project.id, "running-transfer", vec![]).unwrap();
    let preflight = f.preflight();
    drop(guard);
    assert_eq!(
        codes(&preflight.blockers),
        vec!["blox_not_authenticated", "already_transferring"]
    );
    assert_eq!(codes(&preflight.repos[0].blockers), vec!["session_running"]);
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
    /// Bundles kept on this machine.
    bundles: usize,
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
    assert_eq!(
        f.bundles(&transfer.transfer_id),
        case.bundles,
        "{}: bundles",
        case.name
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
            .workspace_dir(WORKSPACE)
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
        (TransferPhase::Checking, 0),
        (TransferPhase::PreparingDestination, 0),
        (TransferPhase::Snapshotting, 0),
        (TransferPhase::Moving, 1),
        (TransferPhase::Applying, 1),
        (TransferPhase::Committing, 2),
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
            bundles: 0,
        },
        Case {
            name: "inspect",
            trigger: Trigger::Fault(Fault::Script(Local, PREFLIGHT_SCRIPT, 1)),
            bundles: 0,
        },
        Case {
            name: "start workspace",
            trigger: Trigger::Fault(Fault::StartWorkspace),
            bundles: 0,
        },
        Case {
            name: "clone second repo",
            trigger: Trigger::Fault(Fault::Script(Workspace, CLONE_SCRIPT, 2)),
            bundles: 0,
        },
        Case {
            name: "origin tips",
            trigger: Trigger::Fault(Fault::Script(Workspace, ORIGIN_TIPS_SCRIPT, 1)),
            bundles: 0,
        },
        Case {
            name: "second snapshot",
            trigger: Trigger::Fault(Fault::Script(Local, SNAPSHOT_SCRIPT, 2)),
            bundles: 1,
        },
        Case {
            name: "upload",
            trigger: Trigger::Fault(Fault::PutFile(Workspace)),
            bundles: 1,
        },
        Case {
            name: "first apply",
            trigger: Trigger::Fault(Fault::Script(Workspace, APPLY_SCRIPT, 1)),
            bundles: 1,
        },
        Case {
            name: "second apply",
            trigger: Trigger::Fault(Fault::Script(Workspace, APPLY_SCRIPT, 2)),
            bundles: 2,
        },
        Case {
            name: "session at commit",
            trigger: Trigger::SessionAtCommit,
            bundles: 2,
        },
    ] {
        assert_to_remote_rolls_back(case);
    }
}

#[test]
fn a_cancelled_transfer_to_local_rolls_back_from_every_phase() {
    for (phase, bundles) in [
        (TransferPhase::Checking, 0),
        (TransferPhase::PreparingDestination, 0),
        (TransferPhase::Snapshotting, 0),
        (TransferPhase::Moving, 0),
        (TransferPhase::Applying, 1),
        (TransferPhase::Committing, 2),
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
            bundles: 0,
        },
        Case {
            name: "local clone",
            trigger: Trigger::Fault(Fault::EnsureClone),
            bundles: 0,
        },
        Case {
            name: "second origin tips",
            trigger: Trigger::Fault(Fault::Script(Local, ORIGIN_TIPS_SCRIPT, 2)),
            bundles: 0,
        },
        Case {
            name: "snapshot",
            trigger: Trigger::Fault(Fault::Script(Workspace, SNAPSHOT_SCRIPT, 1)),
            bundles: 0,
        },
        Case {
            name: "download",
            trigger: Trigger::Fault(Fault::GetFile(Workspace)),
            bundles: 0,
        },
        Case {
            name: "land the branch",
            trigger: Trigger::Fault(Fault::Script(Local, APPLY_SCRIPT, 1)),
            bundles: 1,
        },
        Case {
            name: "finish in the worktree",
            trigger: Trigger::Fault(Fault::Script(Local, APPLY_SCRIPT, 2)),
            bundles: 1,
        },
        Case {
            name: "second repo",
            trigger: Trigger::Fault(Fault::Script(Local, APPLY_SCRIPT, 3)),
            bundles: 2,
        },
        Case {
            name: "session at commit",
            trigger: Trigger::SessionAtCommit,
            bundles: 2,
        },
    ] {
        assert_to_local_rolls_back(case);
    }
}
