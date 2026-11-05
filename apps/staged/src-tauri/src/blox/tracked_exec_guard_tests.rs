//! Every `ws exec` Staged makes either lands in the workstation's Blox
//! command history (`ExecMode::Tracked`) or doesn't (`ExecMode::Transient`).
//! Tracked entries are never cleaned up, so Staged's own plumbing flooded it
//! with over a thousand git reads in an afternoon. Transient is the default
//! for plumbing, and a tracked call is a reviewed exception: one that can
//! outlast the transient path's 60 s limit, streams output, or runs a
//! command the user wrote.
//!
//! Source-level: each `ExecMode::Tracked` in `src/` is attributed to the fn it
//! appears in and must match `TRACKED`, so adding one means adding it here.

use std::collections::BTreeMap;
use std::path::Path;

/// Each (file, fn) that passes `ExecMode::Tracked`, how many times, and why.
const TRACKED: &[(&str, &str, usize)] = &[
    // The user's own repo action: streams output and may run for minutes.
    ("actions/commands.rs", "run_branch_action_impl", 1),
    // `git clone` and both setup fetches, for a repo added to a workspace.
    ("branches.rs", "clone_repo_into_workspace", 3),
    // The setup fetch probing whether the branch exists on the remote; a
    // killed probe would silently branch from base instead.
    ("branches.rs", "start_workspace", 1),
    // Pipeline steps: push runs pre-push hooks and a rebase can take minutes.
    ("session_runner.rs", "run_remote_pipeline_command", 1),
    // Project transfer scripts hash working trees, write bundles and clone;
    // their timeout grows with the payload, well past 60 s.
    ("project_transfer/endpoint.rs", "run_script_sized", 1),
    // The `start_workspace` arm's copy of the setup fetch.
    ("web_server.rs", "dispatch", 1),
];

#[test]
fn tracked_blox_execs_match_the_allowlist() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeMap::<(String, String), usize>::new();
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);

    for path in files {
        let rel = path
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel.ends_with("tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let mut current_fn = String::from("<module>");
        for line in non_test_lines(&text) {
            if let Some(name) = fn_name(line) {
                current_fn = name.to_string();
            }
            let hits = line.matches("ExecMode::Tracked").count();
            if hits > 0 {
                *found.entry((rel.clone(), current_fn.clone())).or_default() += hits;
            }
        }
    }

    let expected: BTreeMap<(String, String), usize> = TRACKED
        .iter()
        .map(|(file, func, count)| (((*file).to_string(), (*func).to_string()), *count))
        .collect();
    assert_eq!(
        found, expected,
        "a Blox exec switched to or from ExecMode::Tracked. Tracked execs stay in the \
         workstation's command history forever; use ExecMode::Transient unless the \
         call can outlast 60 s, streams output, or runs a user-written command, and \
         update TRACKED with the reason"
    );
}

/// `ws exec` argv is built only by `blox-cli`, so every call must pick a mode.
#[test]
fn ws_exec_argv_is_built_only_by_blox_cli() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    let offenders: Vec<String> = files
        .iter()
        .filter(|path| !path.ends_with("blox/tracked_exec_guard_tests.rs"))
        .filter(|path| {
            let text = std::fs::read_to_string(path).unwrap();
            text.contains("\"exec\"") || text.contains("\"--transient\"")
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "build `sq blox ws exec` argv with blox_cli::ws_exec_command: {offenders:?}"
    );
}

fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The file's lines up to its trailing inline `#[cfg(test)] mod … {` block.
fn non_test_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut lines = text.lines().peekable();
    std::iter::from_fn(move || {
        let line = lines.next()?;
        if line.trim() == "#[cfg(test)]"
            && lines.peek().is_some_and(|next| {
                let next = next.trim();
                next.starts_with("mod ") && next.ends_with('{')
            })
        {
            return None;
        }
        Some(line)
    })
}

/// The name a line declares with `fn`, if it declares one.
fn fn_name(line: &str) -> Option<&str> {
    let rest = line.split_once("fn ")?;
    let before = rest.0.trim();
    let declares = before.is_empty()
        || before.split_whitespace().all(|word| {
            matches!(word, "pub" | "async" | "const" | "unsafe") || word.starts_with("pub(")
        });
    if !declares {
        return None;
    }
    let name_end = rest
        .1
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.1.len());
    (name_end > 0).then(|| &rest.1[..name_end])
}
