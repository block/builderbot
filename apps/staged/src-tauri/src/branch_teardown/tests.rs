use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{cleanup_lane, cleanup_project_branches_best_effort, MAX_PARALLEL_CLONE_CLEANUPS};
use crate::store::{Branch, BranchType, Project, ProjectRepo, Store};

fn remote_branch(project_id: &str, id: &str, branch_name: &str, workspace_name: &str) -> Branch {
    let mut branch = Branch::new_remote(project_id, branch_name, "main", workspace_name);
    branch.id = id.to_string();
    branch
}

fn local_branch(project_id: &str, id: &str) -> Branch {
    let mut branch = Branch::new(project_id, id, "main");
    branch.id = id.to_string();
    branch
}

/// Counts calls per branch id, for closures that may run on several threads.
#[derive(Default)]
struct CallLog {
    calls: Mutex<Vec<String>>,
}

impl CallLog {
    fn record(&self, branch: &Branch) -> usize {
        let mut calls = self.calls.lock().unwrap();
        calls.push(branch.id.clone());
        calls.iter().filter(|id| **id == branch.id).count()
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, id: &str) -> usize {
        self.calls().iter().filter(|call| *call == id).count()
    }
}

#[test]
fn delete_project_cleanup_retries_branch_row_deletes_and_re_sweeps_remote_workspaces() {
    let branches = vec![
        remote_branch("project-1", "a", "feature-a", "ws-shared"),
        remote_branch("project-1", "b", "feature-b", "ws-shared"),
    ];

    let cleanups = CallLog::default();
    let deletes = CallLog::default();

    cleanup_project_branches_best_effort(
        &branches,
        |_| None,
        |branch| {
            cleanups.record(branch);
        },
        |branch| {
            if deletes.record(branch) == 1 && branch.id == "a" {
                Err("simulated transient delete failure".to_string())
            } else {
                Ok(())
            }
        },
    );

    assert_eq!(deletes.count("a"), 2);
    assert_eq!(deletes.count("b"), 1);
    assert_eq!(cleanups.calls(), ["a", "b", "a", "b"]);
}

#[test]
fn delete_project_cleanup_final_sweep_only_reprocesses_remote_branches() {
    let branches = vec![
        remote_branch("project-1", "remote", "feature-remote", "ws-shared"),
        local_branch("project-1", "local"),
    ];

    let cleanups = CallLog::default();
    let deletes = CallLog::default();

    cleanup_project_branches_best_effort(
        &branches,
        |branch| (branch.branch_type == BranchType::Local).then(|| "owner/repo".to_string()),
        |branch| {
            cleanups.record(branch);
        },
        |branch| {
            if deletes.record(branch) == 1 && branch.id == "remote" {
                Err("simulated transient delete failure".to_string())
            } else {
                Ok(())
            }
        },
    );

    assert_eq!(deletes.count("remote"), 2);
    assert_eq!(deletes.count("local"), 1);
    // The two lanes run in parallel, so only the sweep's place is fixed.
    let mut calls = cleanups.calls();
    assert_eq!(calls.pop().as_deref(), Some("remote"));
    calls.sort();
    assert_eq!(calls, ["local", "remote"]);
}

/// Six clones of two branches each: four clones are torn down at once, and
/// a clone's branches never overlap and keep their list order.
#[test]
fn delete_project_cleanup_runs_clones_in_parallel_up_to_the_cap_and_each_clone_in_order() {
    const CLONES: usize = 6;
    let clone_of = |id: &str| id.split('-').next().unwrap().to_string();
    let branches: Vec<Branch> = (1..=2)
        .flat_map(|n| (0..CLONES).map(move |clone| format!("clone{clone}-{n}")))
        .map(|id| local_branch("project-1", &id))
        .collect();

    let running: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let peak = AtomicUsize::new(0);
    let clone_overlapped = AtomicBool::new(false);
    let cleanups = CallLog::default();
    let deletes = CallLog::default();

    cleanup_project_branches_best_effort(
        &branches,
        |branch| Some(clone_of(&branch.id)),
        |branch| {
            {
                let mut running = running.lock().unwrap();
                if running
                    .iter()
                    .any(|id| clone_of(id) == clone_of(&branch.id))
                {
                    clone_overlapped.store(true, Ordering::SeqCst);
                }
                running.push(branch.id.clone());
                peak.fetch_max(running.len(), Ordering::SeqCst);
            }
            cleanups.record(branch);
            // Hold the branch until the pool has filled, so the peak is
            // observed rather than raced.
            let deadline = Instant::now() + Duration::from_secs(5);
            while peak.load(Ordering::SeqCst) < MAX_PARALLEL_CLONE_CLEANUPS
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            running.lock().unwrap().retain(|id| *id != branch.id);
        },
        |branch| {
            deletes.record(branch);
            Ok(())
        },
    );

    assert_eq!(peak.load(Ordering::SeqCst), MAX_PARALLEL_CLONE_CLEANUPS);
    assert!(!clone_overlapped.load(Ordering::SeqCst));
    for branch in &branches {
        assert_eq!(cleanups.count(&branch.id), 1, "{} cleaned once", branch.id);
        assert_eq!(deletes.count(&branch.id), 1, "{} deleted once", branch.id);
    }
    let calls = cleanups.calls();
    let position = |id: String| calls.iter().position(|call| *call == id).unwrap();
    for clone in 0..CLONES {
        assert!(position(format!("clone{clone}-1")) < position(format!("clone{clone}-2")));
    }
}

/// Local branches share a lane exactly when they share a clone: subpath repos
/// of one GitHub repo do, whatever the slug's case. Remote branches all take
/// the `None` lane.
#[test]
fn cleanup_lanes_follow_the_clone_directory() {
    let store = Arc::new(Store::in_memory().unwrap());
    let project = Project::new("block/builderbot");
    store.create_project(&project).unwrap();
    let repo = |github_repo: &str, subpath: &str| {
        let repo = ProjectRepo::new(
            &project.id,
            github_repo,
            "feature",
            Some(subpath.to_string()),
        );
        store.create_project_repo(&repo).unwrap();
        repo
    };
    let staged = repo("block/builderbot", "apps/staged");
    let differ = repo("Block/BuilderBot", "apps/differ");
    let goose = repo("block/goose", "ui");
    let local = |repo: &ProjectRepo| {
        Branch::new(&project.id, "feature", "main").with_project_repo(&repo.id)
    };

    let lane = |branch: &Branch| cleanup_lane(&store, branch);
    let builderbot = Some("block/builderbot".to_string());
    assert_eq!(lane(&local(&staged)), builderbot);
    assert_eq!(lane(&local(&differ)), builderbot);
    // No project repo: the project's primary repo.
    assert_eq!(
        lane(&Branch::new(&project.id, "feature", "main")),
        builderbot
    );
    assert_eq!(lane(&local(&goose)), Some("block/goose".to_string()));
    assert_eq!(
        lane(&remote_branch(&project.id, "r", "feature", "ws-1").with_project_repo(&goose.id)),
        None
    );
}
