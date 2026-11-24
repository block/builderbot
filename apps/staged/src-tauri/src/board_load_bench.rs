//! Board-load benchmark: the backend half of the kanban board baseline.
//!
//! An `#[ignore]`d test, so it never runs in CI. It creates a throwaway
//! SQLite database per dataset size inside `STAGED_BOARD_BENCH_DIR` (which
//! must not lead into `~/.staged`, through `..` or a symlink either, nor
//! into the directory `STAGED_DATA_DIR` names when that is set), seeds
//! it through the real `Store` write methods, then times the reads the frontend issues to fill the board:
//! `list_projects`, `get_tag_state`, `get_all_repo_badges`, and the branch
//! and repo lists of every project. Those lists are timed both ways the app
//! can ask for them: the two all-projects commands it hydrates with
//! (`list_branches_by_project`, `list_project_repos_by_project`), and one
//! `list_branches_for_project` plus one `list_project_repos` per project,
//! which it still sends for a single project and which was the only way
//! before the bulk commands. It also writes each dataset's real command
//! output to `fixture-<n>.json`, which the browser harness
//! (`apps/staged/scripts/board-perf/`) serves back to the real frontend.
//!
//! Run from `apps/staged/src-tauri`:
//!
//! ```sh
//! STAGED_BOARD_BENCH_DIR=/tmp/kanban-perf/data \
//!   cargo test --lib board_load_bench -- --ignored --nocapture
//! ```
//!
//! `STAGED_BOARD_BENCH_SIZES` (default `50,200,1000`) picks the project
//! counts and `STAGED_BOARD_BENCH_ROUNDS` (default 7) the timed rounds; the
//! table reports the median round.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;

use crate::branches::list_branches_for_project_impl;
use crate::project_commands::{list_branches_by_project_impl, list_project_repos_by_project_impl};
use crate::store::{
    Branch, Commit, NewTag, Note, Project, ProjectRepo, RepoBadge, Session, SessionStatus, Store,
    TagColor, Workdir,
};

const REPOS: [(&str, Option<&str>); 12] = [
    ("acme/storefront", None),
    ("acme/storefront", Some("apps/checkout")),
    ("acme/storefront", Some("apps/catalog")),
    ("acme/payments-api", None),
    ("acme/ledger", None),
    ("acme/mobile", Some("ios")),
    ("acme/mobile", Some("android")),
    ("acme/design-system", None),
    ("acme/infra", Some("terraform")),
    ("acme/data-pipelines", None),
    ("acme/docs", None),
    ("acme/search", None),
];
const BADGES: [&str; 12] = [
    "store", "chk", "cat", "pay", "ledg", "ios", "andr", "ds", "tf", "data", "docs", "srch",
];
const VERBS: [&str; 10] = [
    "Fix",
    "Add",
    "Refactor",
    "Speed up",
    "Audit",
    "Migrate",
    "Remove",
    "Rework",
    "Document",
    "Investigate",
];
const NOUNS: [&str; 12] = [
    "checkout retries",
    "ledger export",
    "search ranking",
    "session expiry",
    "invoice rounding",
    "webhook delivery",
    "onboarding flow",
    "rate limiter",
    "image uploads",
    "offline sync",
    "audit log",
    "feature flags",
];

struct Seeded {
    project_ids: Vec<String>,
    branches: usize,
    rows: usize,
}

/// `n` projects, newest first by index, each with one or two repos, one to
/// three local branches (a workdir, three commits, two notes and two
/// completed sessions apiece, most with pull request state), and tags from
/// a pinned "Status" group, a "Priority" group and two free tags.
fn seed(store: &Store, n: usize) -> Seeded {
    let base_time = 1_780_000_000_000_i64;
    let mut rows = 0usize;

    for (i, (repo, subpath)) in REPOS.iter().enumerate() {
        store
            .create_repo_badge(&RepoBadge {
                github_repo: repo.to_string(),
                subpath: subpath.unwrap_or("").to_string(),
                short_name: BADGES[i].to_string(),
                hue: (i * 30) as f64,
                created_at: base_time,
                pinned: i < 3,
                pin_sort_order: (i < 3).then_some(i as i32),
                default_branch: Some("main".to_string()),
            })
            .unwrap();
        rows += 1;
    }

    let tag = |name: &str, color: TagColor, icon: &str| NewTag {
        name: name.to_string(),
        color: Some(color),
        icon: Some(icon.to_string()),
    };
    let status = store
        .create_tag_group(
            "Status",
            &[
                tag("Backlog", TagColor::Gray, "circle"),
                tag("In progress", TagColor::Blue, "circle-dot"),
                tag("In review", TagColor::Amber, "eye"),
                tag("Done", TagColor::Green, "circle-check"),
            ],
        )
        .unwrap();
    store.set_tag_group_sidebar(&status.id, true).unwrap();
    let priority = store
        .create_tag_group(
            "Priority",
            &[
                tag("P0", TagColor::Red, "flame"),
                tag("P1", TagColor::Amber, "arrow-up"),
                tag("P2", TagColor::Gray, "arrow-down"),
            ],
        )
        .unwrap();
    let free = |name: &str, color: TagColor| NewTag {
        name: name.to_string(),
        color: Some(color),
        icon: None,
    };
    let free_a = store
        .create_tag(&free("needs-design", TagColor::Purple), None, None)
        .unwrap();
    let free_b = store
        .create_tag(&free("customer", TagColor::Cyan), None, None)
        .unwrap();
    let state = store.get_tag_state().unwrap();
    let tags_of = |group_id: &str| -> Vec<String> {
        state
            .tags
            .iter()
            .filter(|t| t.group_id.as_deref() == Some(group_id))
            .map(|t| t.id.clone())
            .collect()
    };
    let status_tags = tags_of(&status.id);
    let priority_tags = tags_of(&priority.id);

    let mut project_ids = Vec::with_capacity(n);
    let mut branch_count = 0usize;
    for i in 0..n {
        let (repo, subpath) = REPOS[i % REPOS.len()];
        let name = format!(
            "{} {} {}",
            VERBS[i % VERBS.len()],
            NOUNS[(i / VERBS.len() + i) % NOUNS.len()],
            i + 1
        );
        let mut project = Project::named(&name).with_primary_repo(repo);
        project.subpath = subpath.map(str::to_string);
        // Index 0 is the newest, so `list_projects` returns them in index order.
        project.created_at = base_time - (i as i64) * 3_600_000;
        project.updated_at = project.created_at;
        store.create_project(&project).unwrap();
        rows += 1;

        let slug = format!("bench-{}", i + 1);
        let mut primary =
            ProjectRepo::new(&project.id, repo, &slug, subpath.map(str::to_string)).primary();
        primary.created_at = project.created_at;
        store.create_project_repo(&primary).unwrap();
        rows += 1;
        let mut repo_ids = vec![primary.id.clone()];
        if i % 4 == 0 {
            let (other, other_subpath) = REPOS[(i + 5) % REPOS.len()];
            let mut second =
                ProjectRepo::new(&project.id, other, &slug, other_subpath.map(str::to_string));
            second.created_at = project.created_at + 1;
            store.create_project_repo(&second).unwrap();
            rows += 1;
            repo_ids.push(second.id.clone());
        }

        for b in 0..(1 + i % 3) {
            let mut branch = Branch::new(&project.id, &format!("{slug}-{b}"), "main");
            branch.project_repo_id = Some(repo_ids[b % repo_ids.len()].clone());
            branch.setup_complete = true;
            branch.created_at = project.created_at + b as i64;
            if (i + b) % 3 != 0 {
                let number = 1000 + (i * 3 + b) as u64;
                branch.pr_number = Some(number);
                branch.pr_state =
                    Some(if (i + b) % 5 == 0 { "MERGED" } else { "OPEN" }.to_string());
                branch.pr_checks_status = Some(
                    if (i + b) % 7 == 0 {
                        "FAILURE"
                    } else {
                        "SUCCESS"
                    }
                    .to_string(),
                );
                branch.pr_review_decision = Some("REVIEW_REQUIRED".to_string());
                branch.pr_mergeable = Some(true);
                branch.pr_draft = Some((i + b) % 4 == 0);
                branch.pr_url = Some(format!("https://github.com/{repo}/pull/{number}"));
                branch.pr_title = Some(format!("{name} (part {})", b + 1));
                branch.pr_fetched_at = Some(base_time);
                branch.pr_updated_at = Some(base_time);
            }
            store.create_branch(&branch).unwrap();
            let workdir = Workdir::new(
                &project.id,
                &format!("/nonexistent/staged-board-bench/{}/{slug}-{b}", project.id),
            )
            .with_branch(&branch.id);
            store.create_workdir(&workdir).unwrap();
            rows += 2;
            branch_count += 1;

            for s in 0..2 {
                let mut session = Session::new_running(
                    &format!("{name}: step {}", s + 1),
                    Path::new(&workdir.path),
                );
                session.status = SessionStatus::Completed;
                session.owner_pid = None;
                session.branch_id = Some(branch.id.clone());
                session.created_at = branch.created_at + s as i64;
                session.updated_at = session.created_at;
                store.create_session(&session).unwrap();
                let body = format!("## {name}\n\n{}", "Findings and next steps. ".repeat(24));
                let mut note = Note::new(&branch.id, &format!("{name} notes {}", s + 1), &body)
                    .with_session(&session.id);
                note.read_at = Some(base_time);
                store.create_note(&note).unwrap();
                rows += 2;
            }
            for c in 0..3 {
                let sha = format!(
                    "{:040x}",
                    (i * 97 + b * 13 + c + 1) as u128 * 0x9e37_79b9_7f4a
                );
                store
                    .create_commit(&Commit::new_with_sha(&branch.id, &sha))
                    .unwrap();
                rows += 1;
            }
        }

        // 85% carry a Status tag, half a Priority, a fifth a free tag.
        if i % 20 < 17 {
            let tag_id = &status_tags[[0, 0, 1, 1, 2, 3, 0, 1, 3, 0][i % 10]];
            store
                .set_project_group_tag(&project.id, &status.id, Some(tag_id))
                .unwrap();
            rows += 1;
        }
        if i % 2 == 0 {
            let tag_id = &priority_tags[(i / 2) % priority_tags.len()];
            store
                .set_project_group_tag(&project.id, &priority.id, Some(tag_id))
                .unwrap();
            rows += 1;
        }
        if i % 5 == 0 {
            let free = if i % 10 == 0 { &free_a } else { &free_b };
            store
                .set_project_tag_assigned(&project.id, &free.id, true)
                .unwrap();
            rows += 1;
        }
        project_ids.push(project.id);
    }
    Seeded {
        project_ids,
        branches: branch_count,
        rows,
    }
}

fn median_ms(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn time_ms<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed().as_secs_f64() * 1000.0)
}

fn bench_dir() -> PathBuf {
    let dir = PathBuf::from(
        std::env::var("STAGED_BOARD_BENCH_DIR")
            .expect("set STAGED_BOARD_BENCH_DIR to a throwaway directory"),
    );
    // `~/.staged` as well as the directory `STAGED_DATA_DIR` names: with the
    // override set, `paths::data_dir()` alone is the scratch directory, and
    // the installed one would go unguarded.
    let data_dirs = crate::paths::all_data_dirs();
    assert!(!data_dirs.is_empty(), "home directory");
    guarded_bench_dir(&dir, &data_dirs).unwrap_or_else(|refusal| panic!("{refusal}"))
}

/// [`guarded_scratch_dir`] for the benchmark's directory, where it deletes
/// and rewrites `data-<n>.db` files.
fn guarded_bench_dir(dir: &Path, data_dirs: &[PathBuf]) -> Result<PathBuf, String> {
    guarded_scratch_dir("STAGED_BOARD_BENCH_DIR", dir, data_dirs)
}

/// Create a scratch directory and return its real path, or refuse it when
/// that path is inside one of `data_dirs`. What runs in it writes database
/// files of its own, so the check is made on where the path really leads,
/// not on how it is spelled: a relative path, a `..` that climbs back in,
/// and a symlink into a data directory are all resolved first. `var` is the
/// environment variable `dir` came from, for the refusal.
///
/// Checked against every data directory before anything is created, so a
/// refused path leaves them all untouched, and once more on the created
/// directory's canonical path, in case the path changed under the check.
pub(crate) fn guarded_scratch_dir(
    var: &str,
    dir: &Path,
    data_dirs: &[PathBuf],
) -> Result<PathBuf, String> {
    // Resolved the same way as the scratch directory, so a symlink on the
    // way to a data directory does not hide it, whether or not the data
    // directory exists yet.
    let real_data_dirs = data_dirs
        .iter()
        .map(|data_dir| Ok((data_dir, resolve_without_creating(data_dir)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let refuse_inside_data_dir = |path: &Path| match real_data_dirs
        .iter()
        .find(|(_, real_data_dir)| path.starts_with(real_data_dir))
    {
        Some((data_dir, _)) => Err(format!(
            "{var} must not be inside {}: {} resolves to {}",
            data_dir.display(),
            dir.display(),
            path.display()
        )),
        None => Ok(()),
    };

    let resolved = resolve_without_creating(dir)?;
    refuse_inside_data_dir(&resolved)?;
    std::fs::create_dir_all(&resolved)
        .map_err(|e| format!("cannot create {}: {e}", resolved.display()))?;
    let created = resolved
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", resolved.display()))?;
    refuse_inside_data_dir(&created)?;
    Ok(created)
}

/// Where `path` leads, without creating any of it: the part that exists is
/// canonicalized, which follows its symlinks and its `..`, and the part that
/// does not exist yet is applied to that by name, where a `..` can only step
/// back out of a directory that is not there to be a symlink.
fn resolve_without_creating(path: &Path) -> Result<PathBuf, String> {
    use std::path::Component;

    let absolute = std::path::absolute(path)
        .map_err(|e| format!("cannot make {} absolute: {e}", path.display()))?;
    let components: Vec<Component> = absolute.components().collect();
    // The longest leading run of components that exists.
    let mut existing = components.len();
    let mut resolved = loop {
        let prefix: PathBuf = components[..existing].iter().collect();
        match prefix.canonicalize() {
            Ok(real) => break real,
            Err(_) if existing > 1 => existing -= 1,
            Err(e) => return Err(format!("cannot resolve {}: {e}", absolute.display())),
        }
    };
    for component in &components[existing..] {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    /// A stand-in home: `data` plays `~/.staged`, and everything else in the
    /// temporary directory is outside it. Canonical, as macOS hands out
    /// temporary directories behind a symlink.
    fn sandbox() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("data");
        std::fs::create_dir(&data).unwrap();
        (temp, root, data)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_directory_outside_the_data_directory_is_created_and_returned() {
        let (_temp, root, data) = sandbox();

        let dir =
            guarded_bench_dir(&root.join("bench/run-1"), std::slice::from_ref(&data)).unwrap();

        assert_eq!(dir, root.join("bench/run-1"));
        assert!(dir.is_dir());
        assert!(entries(&data).is_empty());
    }

    #[test]
    fn a_directory_inside_the_data_directory_is_refused_and_not_created() {
        let (_temp, _root, data) = sandbox();

        let refusal =
            guarded_bench_dir(&data.join("bench"), std::slice::from_ref(&data)).unwrap_err();

        assert!(refusal.contains("must not be inside"), "{refusal}");
        assert!(entries(&data).is_empty());
        // The data directory itself is refused too.
        assert!(guarded_bench_dir(&data, std::slice::from_ref(&data)).is_err());
    }

    /// `std::path::absolute` keeps `..`, so this path does not start with
    /// the data directory as written, and it leads straight into it.
    #[test]
    fn a_parent_reference_that_climbs_into_the_data_directory_is_refused() {
        let (_temp, root, data) = sandbox();
        std::fs::create_dir(root.join("elsewhere")).unwrap();

        // Through a directory that exists, and through one that does not.
        for detour in ["elsewhere/../data/bench", "missing/../data/bench"] {
            let path = root.join(detour);
            assert!(!path.starts_with(&data), "the spelling hides it: {detour}");

            let refusal = guarded_bench_dir(&path, std::slice::from_ref(&data)).unwrap_err();

            assert!(refusal.contains("must not be inside"), "{refusal}");
        }
        assert!(entries(&data).is_empty());
        assert!(!root.join("missing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_into_the_data_directory_is_refused() {
        let (_temp, root, data) = sandbox();
        std::os::unix::fs::symlink(&data, root.join("link")).unwrap();

        // The link itself, and a directory yet to be made behind it.
        for path in [root.join("link"), root.join("link/bench")] {
            let refusal = guarded_bench_dir(&path, std::slice::from_ref(&data)).unwrap_err();

            assert!(refusal.contains("must not be inside"), "{refusal}");
        }
        assert!(entries(&data).is_empty());
    }

    /// The data directory may itself be reached through a symlink, as a
    /// home directory on another volume is.
    #[cfg(unix)]
    #[test]
    fn a_data_directory_behind_a_symlink_is_still_guarded() {
        let (_temp, root, data) = sandbox();
        std::os::unix::fs::symlink(&data, root.join("home-link")).unwrap();
        let spelled = root.join("home-link");

        assert!(guarded_bench_dir(&data.join("bench"), std::slice::from_ref(&spelled)).is_err());
        assert!(guarded_bench_dir(&spelled.join("bench"), std::slice::from_ref(&spelled)).is_err());
        assert!(entries(&data).is_empty());
    }

    /// A data directory that has not been made yet cannot be canonicalized,
    /// and a benchmark directory inside it would be the first thing in it.
    #[cfg(unix)]
    #[test]
    fn a_data_directory_yet_to_be_made_is_guarded_through_a_symlink() {
        let (_temp, root, _data) = sandbox();
        std::fs::create_dir(root.join("volume")).unwrap();
        std::os::unix::fs::symlink(root.join("volume"), root.join("home")).unwrap();
        let spelled = root.join("home/.staged");

        for path in [spelled.join("bench"), root.join("volume/.staged/bench")] {
            let refusal = guarded_bench_dir(&path, std::slice::from_ref(&spelled)).unwrap_err();

            assert!(refusal.contains("must not be inside"), "{refusal}");
        }
        assert!(!root.join("volume/.staged").exists());
        // Beside it is still outside it.
        let dir =
            guarded_bench_dir(&root.join("home/bench"), std::slice::from_ref(&spelled)).unwrap();
        assert_eq!(dir, root.join("volume/bench"));
    }

    /// A symlink whose target is yet to be made resolves to nothing, so the
    /// check cannot see where it leads. Creating the directory fails on the
    /// link, and nothing is made behind it.
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_into_the_data_directory_creates_nothing() {
        let (_temp, root, data) = sandbox();
        std::os::unix::fs::symlink(data.join("bench"), root.join("link")).unwrap();

        for path in [root.join("link"), root.join("link/run-1")] {
            let refusal = guarded_bench_dir(&path, std::slice::from_ref(&data)).unwrap_err();

            assert!(refusal.contains("cannot create"), "{refusal}");
        }
        assert!(entries(&data).is_empty());
    }

    /// With `STAGED_DATA_DIR` set, `paths::data_dir()` is the scratch
    /// directory it names. `~/.staged` is still not the benchmark's to
    /// write in, and neither is the directory in use.
    #[test]
    fn the_installed_directory_is_refused_while_an_override_names_another() {
        let (_temp, root, _data) = sandbox();
        let home = root.join("home");
        let installed = home.join(".staged");
        let scratch = root.join("scratch");
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::create_dir(&scratch).unwrap();
        let data_dirs =
            crate::paths::resolve_all_data_dirs(Some(scratch.clone().into()), Some(home));

        for inside in [installed.join("bench"), scratch.join("bench")] {
            let refusal = guarded_bench_dir(&inside, &data_dirs).unwrap_err();

            assert!(refusal.contains("must not be inside"), "{refusal}");
        }
        assert!(entries(&installed).is_empty());
        assert!(entries(&scratch).is_empty());
        // Beside both is outside both.
        let dir = guarded_bench_dir(&root.join("bench"), &data_dirs).unwrap();
        assert_eq!(dir, root.join("bench"));
    }

    #[test]
    fn a_parent_reference_that_stays_outside_is_resolved() {
        let (_temp, root, data) = sandbox();

        let dir =
            guarded_bench_dir(&root.join("a/b/../bench"), std::slice::from_ref(&data)).unwrap();

        assert_eq!(dir, root.join("a/bench"));
        assert!(!root.join("a/b").exists());
    }

    #[test]
    fn a_relative_path_is_resolved_against_the_working_directory() {
        let (_temp, _root, data) = sandbox();
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();

        // Not created: only where it leads is asked.
        let resolved = resolve_without_creating(Path::new("bench-dir-that-is-not-there")).unwrap();

        assert_eq!(resolved, cwd.join("bench-dir-that-is-not-there"));
        assert!(!resolved.exists());
        let _ = data;
    }
}

#[test]
#[ignore = "benchmark: run by hand, see the module docs"]
fn board_load_bench() {
    let dir = bench_dir();
    let sizes: Vec<usize> = std::env::var("STAGED_BOARD_BENCH_SIZES")
        .unwrap_or_else(|_| "50,200,1000".to_string())
        .split(',')
        .map(|s| s.trim().parse().expect("a project count"))
        .collect();
    let rounds: usize = std::env::var("STAGED_BOARD_BENCH_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);

    println!(
        "\n| projects | branches | seeded rows | list_projects ms | get_tag_state ms | get_all_repo_badges ms | list_branches_by_project ms | list_project_repos_by_project ms | per-project calls | per-project serial ms | per-project 8 threads ms | per call µs | payload KB |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");

    for n in sizes {
        let db_path = dir.join(format!("data-{n}.db"));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", db_path.display()));
        }
        let store = Arc::new(Store::new(&db_path).unwrap());
        let seeded = seed(&store, n);
        let ids = Arc::new(seeded.project_ids.clone());

        let mut list_projects = Vec::new();
        let mut tag_state = Vec::new();
        let mut badges = Vec::new();
        let mut bulk_branches = Vec::new();
        let mut bulk_repos = Vec::new();
        let mut serial = Vec::new();
        let mut threaded = Vec::new();
        for _ in 0..rounds {
            list_projects.push(time_ms(|| store.list_projects().unwrap()).1);
            tag_state.push(time_ms(|| store.get_tag_state().unwrap()).1);
            badges.push(time_ms(|| store.list_repo_badges().unwrap()).1);
            // The two all-projects commands the app hydrates with.
            bulk_branches.push(time_ms(|| list_branches_by_project_impl(&store).unwrap()).1);
            bulk_repos.push(time_ms(|| list_project_repos_by_project_impl(&store).unwrap()).1);
            // The same lists asked for one project at a time.
            serial.push(
                time_ms(|| {
                    for id in ids.iter() {
                        list_branches_for_project_impl(&store, id).unwrap();
                        store.list_project_repos(id).unwrap();
                    }
                })
                .1,
            );
            // The app runs these on Tauri's blocking pool, many at once, all
            // behind the store's one connection mutex.
            threaded.push(
                time_ms(|| {
                    let handles: Vec<_> = (0..8)
                        .map(|worker| {
                            let store = Arc::clone(&store);
                            let ids = Arc::clone(&ids);
                            std::thread::spawn(move || {
                                for id in ids.iter().skip(worker).step_by(8) {
                                    list_branches_for_project_impl(&store, id).unwrap();
                                    store.list_project_repos(id).unwrap();
                                }
                            })
                        })
                        .collect();
                    for handle in handles {
                        handle.join().unwrap();
                    }
                })
                .1,
            );
        }

        // What the commands return, for the browser harness. The mock
        // answers the all-projects commands with these maps whole, and the
        // per-project ones with one entry each, so check here that the two
        // agree rather than let the harness serve a pairing the app can't
        // produce.
        let projects = store.list_projects().unwrap();
        let branches = list_branches_by_project_impl(&store).unwrap();
        let repos = list_project_repos_by_project_impl(&store).unwrap();
        let mut per_project_branches = BTreeMap::new();
        let mut per_project_repos = BTreeMap::new();
        for id in ids.iter() {
            per_project_branches.insert(
                id.clone(),
                list_branches_for_project_impl(&store, id).unwrap(),
            );
            per_project_repos.insert(id.clone(), store.list_project_repos(id).unwrap());
        }
        assert_eq!(
            json!(branches),
            json!(per_project_branches),
            "the all-projects branch list must match the per-project lists"
        );
        assert_eq!(
            json!(repos),
            json!(per_project_repos),
            "the all-projects repo list must match the per-project lists"
        );
        let fixture = json!({
            "projects": projects,
            "branches": branches,
            "repos": repos,
            "tagState": store.get_tag_state().unwrap(),
            "badges": store.list_repo_badges().unwrap(),
        });
        let bytes = serde_json::to_vec(&fixture).unwrap();
        std::fs::write(dir.join(format!("fixture-{n}.json")), &bytes).unwrap();

        let serial_ms = median_ms(serial);
        println!(
            "| {n} | {} | {} | {:.2} | {:.2} | {:.2} | {:.1} | {:.1} | {} | {:.1} | {:.1} | {:.0} | {} |",
            seeded.branches,
            seeded.rows,
            median_ms(list_projects),
            median_ms(tag_state),
            median_ms(badges),
            median_ms(bulk_branches),
            median_ms(bulk_repos),
            2 * n,
            serial_ms,
            median_ms(threaded),
            serial_ms * 1000.0 / (2 * n) as f64,
            bytes.len() / 1024,
        );
    }
}
