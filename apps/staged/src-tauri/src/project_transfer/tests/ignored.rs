//! Ignored files: which ones a transfer archives, the cap, and placing them
//! at the destination, under every shell.

use std::os::unix::fs::{symlink, PermissionsExt};

use super::*;
use scripts::{IgnoredFiles, IgnoredPlacement, IGNORED_ARCHIVE_SCRIPT, IGNORED_FILES_CAP};

pub(in crate::project_transfer) type Files = BTreeMap<String, Vec<u8>>;

/// Put `feature` in `repo` with an ignore file of its own and a file of
/// every kind beside it; returns the ignored files a transfer archives.
pub(in crate::project_transfer) fn ignored_fixture(repo: &Path) -> Files {
    git(repo, &["checkout", "-q", "-b", "feature"]);
    write(
        repo,
        ".gitignore",
        "*.log\n.env\nnode_modules/\nbuild/\n*.tmp\n",
    );
    write(repo, "src/tracked.txt", "tracked\n");
    // Tracked, though a pattern matches it: the branch's own.
    write(repo, "kept.log", "tracked log\n");
    git(repo, &["add", "-f", "kept.log"]);
    commit(repo, "ignore things");

    let archived: Files = [
        (".env", b"SECRET=1\n".to_vec()),
        ("debug.log", b"debug\n".to_vec()),
        // In a tracked directory.
        ("src/local.log", b"local\n".to_vec()),
        // An untracked directory holding nothing but ignored files.
        ("only-ignored/a.tmp", b"a\n".to_vec()),
        ("only-ignored/deep/b.log", b"b\n".to_vec()),
        // Beside an untracked file.
        ("mixed/x.log", b"x\n".to_vec()),
        ("ünïcødé.log", "ü\n".as_bytes().to_vec()),
        ("with space.log", b"space\n".to_vec()),
        ("-dash.log", b"dash\n".to_vec()),
        ("noise.tmp", noise(10_000, 5)),
    ]
    .into_iter()
    .map(|(p, c)| (p.to_string(), c))
    .collect();
    for (path, content) in &archived {
        write(repo, path, content);
    }
    // Never archived: in ignored directories, at the top or nested, not
    // ignored, or symlinks.
    for path in [
        "node_modules/pkg/index.js",
        "node_modules/.env",
        "src/node_modules/x.js",
        "src/build/y.log",
        "build/out.log",
        "mixed/todo.txt",
        "notes.txt",
    ] {
        write(repo, path, "left\n");
    }
    write(repo, "kept.log", "tracked log, edited\n");
    symlink("a.txt", repo.join("link.log")).unwrap();
    symlink("src", repo.join("dirlink.tmp")).unwrap();
    archived
}

/// Every file under `dir`, by its path from there.
pub(in crate::project_transfer) fn files_under(dir: &Path) -> Files {
    fn walk(root: &Path, dir: &Path, files: &mut Files) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                walk(root, &path, files);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string();
                files.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut files = Files::new();
    walk(dir, dir, &mut files);
    files
}

/// Unpack an archive into `into`, and check it held `expected` and nothing
/// else.
fn assert_archive_holds(archive: &Path, into: &Path, expected: &Files, what: &str) {
    fs::create_dir_all(into).unwrap();
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .unwrap();
    assert!(status.success());
    assert_holds(into, expected, what);
}

/// `dir` holds `expected` and nothing else. By lookup, as macOS tar writes
/// the names it unpacks decomposed (NFD), which the filesystem finds either
/// way.
fn assert_holds(dir: &Path, expected: &Files, what: &str) {
    assert_eq!(ignored_in(dir, expected), *expected, "{what}");
    assert_eq!(files_under(dir).len(), expected.len(), "{what}");
}

/// The excludes list [`IGNORED_ARCHIVE_SCRIPT`] writes beside `archive`.
fn excludes_beside(archive: &Path) -> PathBuf {
    archive.with_extension("excludes")
}

fn archive(
    endpoint: &LocalEndpoint,
    checkout: &Path,
    archive: &Path,
    cap: u64,
) -> Result<IgnoredFiles, String> {
    let output = endpoint
        .run_script(
            IGNORED_ARCHIVE_SCRIPT,
            &scripts::ignored_archive_args(
                checkout.to_str().unwrap(),
                archive.to_str().unwrap(),
                cap,
                excludes_beside(archive).to_str().unwrap(),
            ),
        )
        .unwrap();
    if !output.success {
        return Err(format!("{}{}", output.stdout, output.stderr));
    }
    IgnoredFiles::parse(&output.stdout).map_err(|e| e.to_string())
}

fn total(files: &Files) -> u64 {
    files.values().map(|c| c.len() as u64).sum()
}

#[test]
fn archives_ignored_files_outside_ignored_directories_and_counts_them_alike() {
    for shell in shells() {
        let w = World::new(shell);
        let expected = ignored_fixture(&w.source);
        let endpoint = w.endpoint(&w.local_scratch);
        let path = w.local_scratch.join("x.ignored.tar");

        let ignored = archive(&endpoint, &w.source, &path, IGNORED_FILES_CAP).unwrap();

        assert_archive_holds(&path, &w.root.join("unpacked"), &expected, shell);
        assert_eq!(ignored.files, expected.len() as u64, "{shell}");
        assert_eq!(ignored.bytes, total(&expected), "{shell}");
        assert!(!ignored.over_cap);
        let archive_file = ignored.archive.unwrap();
        assert_eq!(archive_file.size, fs::metadata(&path).unwrap().len());
        assert_eq!(
            archive_file.sha256,
            hex::encode(Sha256::digest(fs::read(&path).unwrap()))
        );
        // Nothing beside it: the list and the partial archive are gone.
        let names: Vec<String> = fs::read_dir(&w.local_scratch)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["x.ignored.tar"], "{shell}");

        // The preflight counts the same files.
        let preflight = endpoint
            .run_script(
                scripts::PREFLIGHT_SCRIPT,
                &scripts::preflight_args([(w.source.to_str().unwrap(), "feature")]),
            )
            .unwrap();
        let report = &scripts::sections(&preflight.stdout)[0];
        assert_eq!(
            report.get("ignored_files"),
            Some(expected.len().to_string().as_str()),
            "{shell}: {}",
            preflight.stdout
        );
        assert_eq!(
            report.get("ignored_bytes"),
            Some(total(&expected).to_string().as_str()),
            "{shell}"
        );
        // node_modules/, src/node_modules/, src/build/ and build/.
        assert_eq!(report.get("ignored_dirs"), Some("4"), "{shell}");
    }
}

#[test]
fn ignored_files_over_the_cap_are_not_archived() {
    for shell in shells() {
        let w = World::new(shell);
        let expected = ignored_fixture(&w.source);
        let endpoint = w.endpoint(&w.local_scratch);
        let path = w.local_scratch.join("x.ignored.tar");
        let bytes = total(&expected);

        let over = archive(&endpoint, &w.source, &path, bytes - 1).unwrap();
        assert!(over.over_cap, "{shell}");
        assert_eq!((over.files, over.bytes), (expected.len() as u64, bytes));
        assert_eq!(over.archive, None);
        assert!(
            fs::read_dir(&w.local_scratch).unwrap().next().is_none(),
            "{shell}: left something behind"
        );

        let at_cap = archive(&endpoint, &w.source, &path, bytes).unwrap();
        assert!(!at_cap.over_cap, "{shell}");
        assert!(at_cap.archive.is_some());

        // A checkout without any: nothing to write.
        let clean = archive(
            &endpoint,
            &w.dest,
            &w.local_scratch.join("y.ignored.tar"),
            IGNORED_FILES_CAP,
        )
        .unwrap();
        assert_eq!((clean.files, clean.bytes, clean.archive), (0, 0, None));
        assert!(!w.local_scratch.join("y.ignored.tar").exists());

        assert!(archive(&endpoint, &w.source, &path, 0).is_ok());
        let err = archive(&endpoint, &w.root.join("nope"), &path, 1).unwrap_err();
        assert!(err.contains("cannot enter"), "{shell}: {err}");
    }
}

/// The ignored files a checkout has, beside its tracked and untracked ones.
fn ignored_in(checkout: &Path, paths: &Files) -> Files {
    paths
        .keys()
        .filter_map(|p| Some((p.clone(), fs::read(checkout.join(p)).ok()?)))
        .collect()
}

#[test]
fn ignored_files_arrive_both_ways_when_asked_for() {
    for shell in shells() {
        // Local → workspace, landing with a checkout.
        let w = World::new(shell);
        let expected = ignored_fixture(&w.source);
        let source = w.endpoint(&w.local_scratch);
        let dest = w.endpoint(&w.remote_scratch);
        let plan = CarryPlan {
            include_ignored: true,
            ignored_bytes: total(&expected),
            ..w.origin_plan(
                &source,
                &dest,
                Landing::Checkout,
                ("feature", "feature"),
                true,
                &["main"],
            )
        };
        let before = observe(&w.source);
        let outcome = carry_repo(&source, &dest, &plan).unwrap();
        assert_eq!(ignored_in(&w.dest, &expected), expected, "{shell}");
        assert_eq!(
            outcome.applied.ignored,
            Some(IgnoredPlacement {
                placed: expected.len() as u64,
                ..IgnoredPlacement::default()
            }),
            "{shell}"
        );
        // They don't pass for changes there, and the source is as it was.
        assert_eq!(observe(&w.dest), before, "{shell}");
        assert_eq!(observe(&w.source), before, "{shell}");
        // Kept on this machine, beside the bundle, and unpacked at the
        // destination only for as long as placing took.
        assert_archive_holds(
            &plan.local_ignored(),
            &w.root.join("kept"),
            &expected,
            shell,
        );
        let staged = format!("{}.d", outcome.dest_ignored.as_deref().unwrap());
        assert!(!Path::new(&staged).exists(), "{shell}: {staged} left");

        // Workspace → local, finishing in a new worktree.
        let w = World::new(shell);
        let expected = ignored_fixture(&w.source);
        let source = w.endpoint(&w.remote_scratch);
        let dest = w.endpoint(&w.local_scratch);
        let plan = CarryPlan {
            include_ignored: true,
            ..w.origin_plan(
                &source,
                &dest,
                Landing::Branch,
                ("feature", "feature"),
                false,
                &["main"],
            )
        };
        let outcome = carry_repo(&source, &dest, &plan).unwrap();
        // A branch landing places nothing: there is no checkout yet.
        assert_eq!(outcome.applied.ignored, None);
        let worktree = w.root.join("wt-feature");
        git(
            &w.dest,
            &[
                "worktree",
                "add",
                "-q",
                worktree.to_str().unwrap(),
                "feature",
            ],
        );
        let finished = finish(&dest, &plan, &outcome, worktree.to_str().unwrap()).unwrap();
        assert_eq!(ignored_in(&worktree, &expected), expected, "{shell}");
        assert_eq!(
            finished.ignored.map(|p| p.placed),
            Some(expected.len() as u64)
        );
        // With the uncommitted changes left behind, the worktree is clean.
        let status = git(
            &worktree,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        assert_eq!(status, "", "{shell}");
    }
}

#[test]
fn placing_ignored_files_never_overwrites_or_follows_a_symlink() {
    for shell in shells() {
        let w = World::new(shell);
        let mut expected = ignored_fixture(&w.source);
        // Already there, and ignored on main too, so it isn't untracked.
        write(&w.dest, "debug.log", "theirs\n");
        // A symlink out of the checkout where a directory of theirs goes.
        let outside = w.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, w.dest.join("only-ignored")).unwrap();

        let source = w.endpoint(&w.local_scratch);
        let dest = w.endpoint(&w.remote_scratch);
        let plan = CarryPlan {
            include_ignored: true,
            ..w.origin_plan(
                &source,
                &dest,
                Landing::Checkout,
                ("feature", "feature"),
                true,
                &["main"],
            )
        };
        let outcome = carry_repo(&source, &dest, &plan).unwrap();

        assert_eq!(fs::read(w.dest.join("debug.log")).unwrap(), b"theirs\n");
        assert!(
            files_under(&outside).is_empty(),
            "{shell}: wrote through the symlink"
        );
        let placement = outcome.applied.ignored.unwrap();
        assert_eq!(placement.unignored, 0, "{shell}: {placement:?}");
        // debug.log, and both files under the symlink.
        assert_eq!(placement.in_the_way, 3, "{shell}: {placement:?}");
        assert!(placement.in_the_way_example.is_some());
        assert_eq!(placement.placed, expected.len() as u64 - 3, "{shell}");
        for path in ["debug.log", "only-ignored/a.tmp", "only-ignored/deep/b.log"] {
            expected.remove(path);
        }
        assert_eq!(ignored_in(&w.dest, &expected), expected, "{shell}");
        // The symlink was theirs, and is all that is untracked.
        let status = git(&w.dest, &["status", "--porcelain", "--untracked-files=all"]);
        let untracked: Vec<&str> = status.lines().filter(|l| l.starts_with("??")).collect();
        let source_untracked = git(
            &w.source,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        let mut wanted: Vec<&str> = source_untracked
            .lines()
            .filter(|l| l.starts_with("??"))
            .collect();
        wanted.push("?? only-ignored");
        wanted.sort();
        assert_eq!(untracked, wanted, "{shell}");
    }
}

#[test]
fn an_ignored_files_archive_that_changed_on_the_way_fails_the_finish() {
    for shell in shells() {
        let w = World::new(shell);
        ignored_fixture(&w.source);
        let source = w.endpoint(&w.remote_scratch);
        let dest = w.endpoint(&w.local_scratch);
        let plan = CarryPlan {
            include_ignored: true,
            ..w.plan(
                &source,
                Landing::Branch,
                ("feature", "feature"),
                true,
                vec![],
            )
        };
        let outcome = carry_repo(&source, &dest, &plan).unwrap();
        let archive = outcome.dest_ignored.clone().unwrap();
        let mut bytes = fs::read(&archive).unwrap();
        bytes.extend_from_slice(b"tampered");
        fs::write(&archive, bytes).unwrap();
        let worktree = w.root.join("wt-feature");
        git(
            &w.dest,
            &[
                "worktree",
                "add",
                "-q",
                worktree.to_str().unwrap(),
                "feature",
            ],
        );

        let err = finish(&dest, &plan, &outcome, worktree.to_str().unwrap()).unwrap_err();

        assert!(
            err.to_string()
                .contains("ignored files do not match their checksum"),
            "{shell}: {err}"
        );
        assert!(!worktree.join(".env").exists());
    }
}

/// Teardown archives into the backup whatever the plan carried: off, the
/// source's archive is the only copy; on, one unchanged since the carry is
/// not fetched again.
#[test]
fn backing_ignored_files_up_brings_them_here_once() {
    for shell in shells() {
        let w = World::new(shell);
        let expected = ignored_fixture(&w.source);
        let source = Recording::new(w.endpoint(&w.remote_scratch));
        let fetched = || {
            source
                .files()
                .iter()
                .filter(|f| f.starts_with("get "))
                .count()
        };
        let plan = w.plan(
            &source.inner,
            Landing::Branch,
            ("feature", "feature"),
            true,
            vec![],
        );
        let backup = super::super::carry::back_up_ignored(&source, &plan, None).unwrap();
        assert_eq!(backup.files, expected.len() as u64);
        assert_archive_holds(
            &plan.local_ignored(),
            &w.root.join("backup"),
            &expected,
            shell,
        );
        assert_eq!(fetched(), 1, "{shell}");

        // The same again, as the carry already brought it.
        let again = super::super::carry::back_up_ignored(&source, &plan, Some(&backup)).unwrap();
        assert_eq!(again, backup);
        assert_eq!(fetched(), 1, "{shell}: fetched it twice");

        // A file changed since: fetched again.
        write(&w.source, ".env", "SECRET=2\n");
        let changed = super::super::carry::back_up_ignored(&source, &plan, Some(&backup)).unwrap();
        assert_ne!(changed.archive, backup.archive);
        assert_eq!(fetched(), 2, "{shell}");
        let mut now = expected.clone();
        now.insert(".env".to_string(), b"SECRET=2\n".to_vec());
        assert_archive_holds(&plan.local_ignored(), &w.root.join("backup-2"), &now, shell);

        // Over the cap: nothing written or fetched, and what is here stays.
        let small = CarryPlan {
            ignored_cap: 1,
            ..plan.clone()
        };
        let over = super::super::carry::back_up_ignored(&source, &small, None).unwrap();
        assert!(over.over_cap);
        assert_eq!(fetched(), 2, "{shell}");
    }
}

/// Git at one end with nothing of this machine's: no system config, a global
/// config of `config` alone, and a home of its own — so the developer's
/// `~/.config/git/ignore` stays out, and each end ignores only what the test
/// says it does.
struct Isolated {
    vars: Vec<(&'static str, String)>,
}

impl Isolated {
    fn new(root: &Path, name: &str, config: &str) -> Self {
        let home = root.join(format!("home-{name}"));
        fs::create_dir_all(home.join(".config/git")).unwrap();
        let gitconfig = home.join(".gitconfig");
        fs::write(&gitconfig, config).unwrap();
        let path = |p: PathBuf| p.to_str().unwrap().to_string();
        Self {
            vars: vec![
                ("HOME", path(home.clone())),
                ("XDG_CONFIG_HOME", path(home.join(".config"))),
                ("GIT_CONFIG_GLOBAL", path(gitconfig)),
                ("GIT_CONFIG_NOSYSTEM", "1".to_string()),
            ],
        }
    }

    /// The default global excludes file, `$XDG_CONFIG_HOME/git/ignore`.
    fn global_ignore(&self) -> PathBuf {
        let xdg = &self
            .vars
            .iter()
            .find(|(k, _)| *k == "XDG_CONFIG_HOME")
            .unwrap()
            .1;
        Path::new(xdg).join("git/ignore")
    }

    fn endpoint(&self, endpoint: LocalEndpoint) -> LocalEndpoint {
        self.vars
            .iter()
            .fold(endpoint, |e, (k, v)| e.with_var(k, v))
    }

    fn git(&self, repo: &Path, args: &[&str]) -> String {
        let mut command = Command::new("git");
        command
            .args(["-c", "core.hooksPath=/dev/null", "-C"])
            .arg(repo)
            .args(args);
        strip_git_env(&mut command);
        command.envs(self.vars.iter().map(|(k, v)| (k, v)));
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn status(&self, repo: &Path) -> String {
        self.git(repo, &["status", "--porcelain", "--untracked-files=all"])
    }
}

fn append(path: &Path, text: &str) {
    let mut contents = fs::read(path).unwrap_or_default();
    contents.extend_from_slice(text.as_bytes());
    fs::write(path, contents).unwrap();
}

/// The preflight's report for one checkout.
fn preflight_of(endpoint: &LocalEndpoint, checkout: &Path) -> scripts::ScriptReport {
    let output = endpoint
        .run_script(
            scripts::PREFLIGHT_SCRIPT,
            &scripts::preflight_args([(checkout.to_str().unwrap(), "feature")]),
        )
        .unwrap();
    assert!(output.success, "{}{}", output.stdout, output.stderr);
    scripts::sections(&output.stdout).remove(0)
}

/// An ignored symlink, dangling or not, that git lists last used to end
/// the listing's batch with a failed test, which failed the whole listing:
/// every backup failed, and the preflight said nothing.
#[test]
fn an_ignored_symlink_listed_last_neither_fails_the_listing_nor_is_archived() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, ".gitignore", ".env\nbazel-*\nzz-*\n");
        commit(&w.source, "ignore links");
        write(&w.source, ".env", "SECRET=1\n");
        symlink("src", w.source.join("bazel-out")).unwrap();
        symlink("nowhere", w.source.join("zz-dangling")).unwrap();
        symlink(".env", w.source.join("zz-link")).unwrap();
        let listed = git(
            &w.source,
            &[
                "status",
                "--porcelain",
                "--ignored=matching",
                "--untracked-files=all",
            ],
        );
        assert_eq!(listed.lines().last(), Some("!! zz-link"), "{listed}");
        let endpoint = w.endpoint(&w.local_scratch);
        let path = w.local_scratch.join("x.ignored.tar");

        let ignored = archive(&endpoint, &w.source, &path, IGNORED_FILES_CAP).unwrap();

        let expected: Files = [(".env".to_string(), b"SECRET=1\n".to_vec())].into();
        assert_eq!((ignored.files, ignored.bytes), (1, 9), "{shell}");
        assert_archive_holds(&path, &w.root.join("unpacked"), &expected, shell);
        let report = preflight_of(&endpoint, &w.source);
        assert_eq!(report.get("ignored_files"), Some("1"), "{shell}");
        assert_eq!(report.get("ignored_unlisted"), None, "{shell}");
    }
}

/// A status that fails part way fails the listing, rather than pass for a
/// checkout without ignored files: the archive refuses, so the source is
/// kept, and the preflight says so.
#[test]
fn a_failing_status_fails_the_ignored_listing_and_the_preflight_says_so() {
    let real_git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real_git = String::from_utf8(real_git.stdout).unwrap();
    for shell in shells() {
        let w = World::new(shell);
        ignored_fixture(&w.source);
        // Only the listing's status — `-z` with `--ignored=matching` — fails,
        // after printing one record, as one cut off would.
        let shim = w.root.join("shim");
        fs::create_dir_all(&shim).unwrap();
        fs::write(
            shim.join("git"),
            format!(
                "#!/bin/sh\nz=0 i=0\nfor a; do case $a in -z) z=1 ;; --ignored=matching) i=1 ;; esac; done\n\
                 if [ $z = 1 ] && [ $i = 1 ]; then printf '!! .env\\000!! debu'; exit 128; fi\n\
                 exec {} \"$@\"\n",
                real_git.trim()
            ),
        )
        .unwrap();
        fs::set_permissions(shim.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
        let endpoint = w.endpoint(&w.local_scratch).with_var(
            "PATH",
            &format!(
                "{}:{}",
                shim.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        let path = w.local_scratch.join("x.ignored.tar");

        let err = archive(&endpoint, &w.source, &path, IGNORED_FILES_CAP).unwrap_err();

        assert!(
            err.contains("cannot list the ignored files"),
            "{shell}: {err}"
        );
        assert!(!path.exists());
        let report = preflight_of(&endpoint, &w.source);
        assert_eq!(report.get("ignored_unlisted"), Some("1"), "{shell}");
        assert_eq!(report.get("ignored_files"), None, "{shell}");
        // The counts that don't need the list still come.
        assert_eq!(report.get("ignored_dirs"), Some("4"), "{shell}");
    }
}

/// `.pytest_cache/` (or `.mypy_cache/`, or a uv `.venv/`) holds a `.gitignore`
/// of `*`: git lists each file in it, and none of them is ignored at a
/// destination until that `.gitignore` is there. A `.gitignore` that only a
/// change left behind ignores is taken back out, with what it ignored.
#[test]
fn a_directory_that_ignores_itself_arrives_whole() {
    for shell in shells() {
        for landing in [Landing::Checkout, Landing::Branch] {
            let w = World::new(shell);
            let source_env = Isolated::new(&w.root, "source", "");
            let dest_env = Isolated::new(&w.root, "dest", "");
            git(&w.source, &["checkout", "-q", "-b", "feature"]);
            write(&w.source, "src/tracked.txt", "tracked\n");
            commit(&w.source, "feature");
            let expected: Files = [
                (
                    ".pytest_cache/.gitignore",
                    "# Created by pytest automatically.\n*\n",
                ),
                (".pytest_cache/README.md", "# pytest cache directory #\n"),
                // In a directory the branch has.
                ("src/.cache/.gitignore", "*\n"),
                ("src/.cache/data", "data\n"),
            ]
            .into_iter()
            .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
            .collect();
            for (path, content) in &expected {
                write(&w.source, path, content);
            }
            // A directory in it is one `*` matches: an ignored folder,
            // never carried.
            write(&w.source, ".pytest_cache/v/cache/nodeids", "[]\n");
            // Ignored only by an uncommitted line, which stays behind.
            append(&w.source.join(".gitignore"), "gen/.gitignore\n");
            write(&w.source, "gen/.gitignore", "out.txt\n");
            write(&w.source, "gen/out.txt", "out\n");
            let (source, dest, checkout) = match landing {
                Landing::Checkout => (
                    source_env.endpoint(w.endpoint(&w.local_scratch)),
                    dest_env.endpoint(w.endpoint(&w.remote_scratch)),
                    w.dest.clone(),
                ),
                Landing::Branch => (
                    source_env.endpoint(w.endpoint(&w.remote_scratch)),
                    dest_env.endpoint(w.endpoint(&w.local_scratch)),
                    w.root.join("wt-feature"),
                ),
            };
            let plan = CarryPlan {
                include_ignored: true,
                ..w.origin_plan(
                    &source,
                    &dest,
                    landing,
                    ("feature", "feature"),
                    false,
                    &["main"],
                )
            };
            let outcome = carry_repo(&source, &dest, &plan).unwrap();
            let placement = match landing {
                Landing::Checkout => outcome.applied.ignored.clone(),
                Landing::Branch => {
                    git(
                        &w.dest,
                        &[
                            "worktree",
                            "add",
                            "-q",
                            checkout.to_str().unwrap(),
                            "feature",
                        ],
                    );
                    finish(&dest, &plan, &outcome, checkout.to_str().unwrap())
                        .unwrap()
                        .ignored
                }
            }
            .unwrap();

            let what = format!("{shell} {landing:?}");
            assert_eq!(ignored_in(&checkout, &expected), expected, "{what}");
            assert_eq!(
                placement.placed,
                expected.len() as u64,
                "{what}: {placement:?}"
            );
            assert_eq!(placement.excluded, 0, "{what}");
            assert_eq!(placement.unignored, 2, "{what}: {placement:?}");
            assert_eq!(
                placement.unignored_example.as_deref(),
                Some("gen/.gitignore"),
                "{what}"
            );
            assert!(!checkout.join("gen").exists(), "{what}: gen/ left behind");
            assert!(!checkout.join(".pytest_cache/v").exists(), "{what}");
            assert_eq!(dest_env.status(&checkout), "", "{what}");
            let staged = format!("{}.d", outcome.dest_ignored.as_deref().unwrap());
            assert!(!Path::new(&staged).exists(), "{what}: {staged} left");
        }
    }
}

/// Files only the source machine's own rules ignore — its clone's
/// `info/exclude`, its global excludes file — would show up as untracked at
/// a destination without those rules, so each gets an anchored entry in the
/// destination's `info/exclude` first, unless the destination ignores it
/// already.
#[test]
fn files_only_this_machines_excludes_ignore_arrive_with_an_entry_in_info_exclude() {
    let odd = "odd #1 [a]*?! x\\y.local";
    for shell in shells() {
        // To a fresh workspace clone, which has none of the source's rules.
        let w = World::new(shell);
        let mut expected = ignored_fixture(&w.source);
        let excludes_file = w.root.join("excludes");
        fs::write(&excludes_file, "**/.claude/settings.local.json\n*.swp\n").unwrap();
        let source_env = Isolated::new(
            &w.root,
            "source",
            &format!("[core]\n\texcludesFile = {}\n", excludes_file.display()),
        );
        let dest_env = Isolated::new(&w.root, "dest", "");
        append(&w.source.join(".git/info/exclude"), ".penpal\n*.local\n");
        let local: Files = [
            (".penpal", "pen\n"),
            (".claude/settings.local.json", "{}\n"),
            ("src/notes.swp", "swp\n"),
            (odd, "odd\n"),
        ]
        .into_iter()
        .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
        .collect();
        for (path, content) in &local {
            write(&w.source, path, content);
        }
        expected.extend(local.clone());
        let source = source_env.endpoint(w.endpoint(&w.local_scratch));
        let dest = dest_env.endpoint(w.endpoint(&w.remote_scratch));
        // The dialog counts them with the rest: they move too.
        let report = preflight_of(&source, &w.source);
        assert_eq!(
            report.get("ignored_files"),
            Some(expected.len().to_string().as_str()),
            "{shell}"
        );
        let plan = CarryPlan {
            include_ignored: true,
            ..w.origin_plan(
                &source,
                &dest,
                Landing::Checkout,
                ("feature", "feature"),
                true,
                &["main"],
            )
        };

        let outcome = carry_repo(&source, &dest, &plan).unwrap();

        let listed = fs::read_to_string(plan.local_ignored_excludes()).unwrap();
        let mut wanted: Vec<String> = local.keys().map(|p| format!("./{p}")).collect();
        wanted.sort();
        assert_eq!(listed.lines().collect::<Vec<_>>(), wanted, "{shell}");
        assert!(outcome.ignored.as_ref().unwrap().excludes.is_some());
        assert_eq!(ignored_in(&w.dest, &expected), expected, "{shell}");
        let placement = outcome.applied.ignored.clone().unwrap();
        assert_eq!(
            placement,
            IgnoredPlacement {
                placed: expected.len() as u64,
                excluded: local.len() as u64,
                ..IgnoredPlacement::default()
            },
            "{shell}"
        );
        let exclude = fs::read_to_string(w.dest.join(".git/info/exclude")).unwrap();
        for entry in [
            "/.penpal",
            "/.claude/settings.local.json",
            "/src/notes.swp",
            "/odd\\ \\#1\\ \\[a\\]\\*\\?\\!\\ x\\\\y.local",
        ] {
            assert!(
                exclude.lines().any(|l| l == entry),
                "{shell}: {entry} in {exclude}"
            );
        }
        // Nothing shows up as a change of the branch's own.
        assert_eq!(
            dest_env.status(&w.dest),
            source_env.status(&w.source),
            "{shell}"
        );

        // To a local clone whose rules already ignore some of them, finishing
        // in a new worktree: only the others get an entry, after a newline
        // the file didn't end with.
        let w = World::new(shell);
        ignored_fixture(&w.source);
        let excludes_file = w.root.join("excludes");
        fs::write(&excludes_file, "**/.claude/settings.local.json\n*.swp\n").unwrap();
        let source_env = Isolated::new(
            &w.root,
            "source",
            &format!("[core]\n\texcludesFile = {}\n", excludes_file.display()),
        );
        let dest_env = Isolated::new(&w.root, "dest", "");
        fs::write(dest_env.global_ignore(), "*.swp\n").unwrap();
        append(&w.source.join(".git/info/exclude"), ".penpal\n*.local\n");
        fs::write(w.dest.join(".git/info/exclude"), ".penpal").unwrap();
        for (path, content) in &local {
            write(&w.source, path, content);
        }
        let source = source_env.endpoint(w.endpoint(&w.remote_scratch));
        let dest = dest_env.endpoint(w.endpoint(&w.local_scratch));
        let plan = CarryPlan {
            include_ignored: true,
            ..w.origin_plan(
                &source,
                &dest,
                Landing::Branch,
                ("feature", "feature"),
                false,
                &["main"],
            )
        };
        let outcome = carry_repo(&source, &dest, &plan).unwrap();
        let worktree = w.root.join("wt-feature");
        git(
            &w.dest,
            &[
                "worktree",
                "add",
                "-q",
                worktree.to_str().unwrap(),
                "feature",
            ],
        );
        let finished = finish(&dest, &plan, &outcome, worktree.to_str().unwrap()).unwrap();

        assert_eq!(ignored_in(&worktree, &local), local, "{shell}");
        let placement = finished.ignored.unwrap();
        assert_eq!((placement.excluded, placement.unignored), (2, 0), "{shell}");
        assert_eq!(
            fs::read_to_string(w.dest.join(".git/info/exclude")).unwrap(),
            format!(
                ".penpal\n/.claude/settings.local.json\n{}\n",
                "/odd\\ \\#1\\ \\[a\\]\\*\\?\\!\\ x\\\\y.local"
            ),
            "{shell}"
        );
        assert_eq!(dest_env.status(&worktree), "", "{shell}");
    }
}
