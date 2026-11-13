//! No async Tauri command or `dispatch` arm may wait on `gh`, git or any
//! other child process on the tokio runtime. Tauri runs `async fn` commands
//! on the shared runtime and `dispatch` runs inside axum on the same one. A
//! worker parked in a synchronous wait stops polling the timer and I/O
//! drivers, so a single slow `gh` call froze every view, poll and session
//! stream for its whole duration. Such calls go through `spawn_blocking`
//! (`blocking_json` in `dispatch`, `run_blox_blocking` for Blox).
//!
//! Source-level like the other `dispatch` arm checks: a unit test has no way
//! to build the `AppHandle` either transport needs. Every fn in `src/` is
//! indexed by file and name. From each async command and arm the check walks,
//! up to `MAX_DEPTH` calls deep, into the crate fns the body calls inline:
//! plain and path-qualified calls, calls to imported fns, and awaited method
//! calls. Each body has the argument lists of the `OFFLOADED` calls cut out
//! first; what remains must not name a `BLOCKING` primitive, other than the
//! `ALLOWED` ones. `git` and `blox` fns are walked like any other, so a pure
//! helper there passes and one that spawns git fails. Plain method calls are
//! not followed, which keeps store reads and other in-process work out of it.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::Path;

use super::tests::{dispatch_arms, extract_generate_handler_commands};

/// Calls whose argument list runs off the runtime.
const OFFLOADED: &[&str] = &[
    "spawn_blocking(",
    "thread::spawn(",
    "blocking_json(",
    "blocking_store_json(",
    "run_blox_blocking(",
];

/// Waits on a child process, a thread or the shell-env capture.
const BLOCKING: &[&str] = &[
    "std::process::Command",
    "Command::new(",
    ".wait()",
    ".join()",
    "thread::scope(",
    "thread::sleep(",
    "block_on(",
    "get_blocking(",
    // `acp_client` resolves agent binaries through a login shell.
    "discover_providers(",
    "find_acp_agent",
];

/// Modules a call into is taken to shell out when the scan can't resolve it
/// to a crate fn: the `blox_cli` crate, and `git` / `blox` items re-exported
/// under another name (`git::cli_run`). An awaited one is async.
const BLOCKING_MODULES: &[&str] = &["git", "blox", "blox_cli"];

/// Inline calls the walk flags that stay, as (file, the fn making the call,
/// the call), each with why.
const ALLOWED: &[(&str, &str, &str)] = &[
    // A closure that `pull_or_queue_branch_with` runs under `spawn_blocking`.
    (
        "prs/pull.rs",
        "pull_or_queue_branch_for_branch",
        "pull_branch_ff_only_impl(",
    ),
    // Only on the quit path, for a pipeline cancelled before its first step.
    (
        "session_runner.rs",
        "start_pipeline_session",
        "resolve_pipeline_artifacts_without_ai(",
    ),
    // Only builds the `sq blox ws exec` argv; `execute_remote` spawns it.
    (
        "actions/commands.rs",
        "run_branch_action_impl",
        "ws_exec_command(",
    ),
    // Known gap: offloading it (and `AcpDriver::new`) needs an async `start_session`.
    ("session_runner.rs", "start_session", "discover_providers("),
];

/// How many calls deep the walk follows crate fns from a command or arm.
const MAX_DEPTH: usize = 12;

#[test]
fn async_commands_and_arms_keep_blocking_calls_off_the_runtime() {
    let src = Crate::load();

    let mut targets = Vec::new();
    let mut defined = BTreeSet::new();
    for (file, (path, code)) in src.files.iter().enumerate() {
        for (name, is_async, body) in command_fns(code) {
            if is_async {
                targets.push((
                    format!("`{name}` command ({path})"),
                    file,
                    name.clone(),
                    body,
                ));
            }
            defined.insert(name);
        }
    }
    let unscanned: Vec<String> = extract_generate_handler_commands(include_str!("../lib.rs"))
        .difference(&defined)
        .cloned()
        .collect();
    assert!(
        unscanned.is_empty(),
        "the scan should find every registered command: {unscanned:#?}"
    );
    let web_server = src.file("web_server.rs");
    for (commands, body) in dispatch_arms(include_str!("../web_server.rs")) {
        let target = format!("`{}` dispatch arm", commands.join(" | "));
        targets.push((target, web_server, "dispatch".to_string(), code_only(&body)));
    }
    for expected in [
        "`setup_worktree` command (branches.rs)",
        "`setup_worktree` dispatch arm",
    ] {
        assert!(
            targets.iter().any(|(target, ..)| target == expected),
            "the scan should reach the {expected}"
        );
    }

    let scans: Vec<Scan> = src
        .fns
        .iter()
        .map(|f| src.scan(f.file, &f.name, &f.body))
        .collect();
    let mut offences = BTreeSet::new();
    for (target, file, owner, body) in &targets {
        let target_scan = src.scan(*file, owner, body);
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([(&target_scan, Vec::new())]);
        while let Some((scan, via)) = queue.pop_front() {
            for call in &scan.blocking {
                offences.insert(format!("{target}{}: {call}", src.describe(&via)));
            }
            if via.len() == MAX_DEPTH {
                continue;
            }
            for &callee in &scan.callees {
                if seen.insert(callee) {
                    queue.push_back((&scans[callee], [via.as_slice(), &[callee]].concat()));
                }
            }
        }
    }
    assert!(
        offences.is_empty(),
        "these async commands and dispatch arms wait on a child process on the tokio \
         runtime; run the call through `spawn_blocking` (`blocking_json` in `dispatch`), \
         or add it to ALLOWED with why it stays:\n{}",
        offences.into_iter().collect::<Vec<_>>().join("\n")
    );
    let used = src.used.borrow();
    let dead: Vec<String> = (ALLOWED.iter())
        .filter(|entry| !used.contains(entry))
        .map(|(file, owner, call)| format!("{file}: `{call}` in `{owner}`"))
        .collect();
    assert!(
        dead.is_empty(),
        "these ALLOWED entries no longer match a call their fn makes; remove them:\n{}",
        dead.join("\n")
    );
}

/// Commands that were synchronous and froze the window: on the main thread
/// they waited on the store, on `gh` or on a login shell. Each is now an
/// `async fn` whose whole body runs under `spawn_blocking`, as (file, name).
const OFFLOADED_COMMANDS: &[(&str, &str)] = &[
    ("util_commands.rs", "is_sq_available"),
    ("lib.rs", "create_project"),
    ("lib.rs", "pin_repo"),
    // The deletes: a media scan per image, or a cascade over every message
    // of a session.
    ("note_commands.rs", "delete_note"),
    ("note_commands.rs", "delete_project_note"),
    ("session_commands.rs", "delete_session"),
    ("timeline.rs", "delete_review"),
    ("timeline.rs", "delete_pending_commit"),
    ("image_commands.rs", "delete_image"),
    // The reads the frontend sends at launch, on timers and on every store
    // event, which waited behind whatever held the store.
    ("lib.rs", "list_projects"),
    ("lib.rs", "list_project_repos"),
    ("lib.rs", "get_all_repo_badges"),
    ("branches.rs", "list_branches_for_project"),
    ("session_commands.rs", "poll_session"),
    ("session_commands.rs", "get_session"),
    ("session_commands.rs", "get_session_messages"),
    ("session_commands.rs", "get_session_messages_since"),
    ("session_commands.rs", "get_active_sessions"),
    ("note_commands.rs", "list_project_notes"),
    ("project_search.rs", "get_project_search_index"),
];

/// What an [`OFFLOADED_COMMANDS`] body may call on the runtime: the store
/// and registry handle lookups, and the mapping of the join error.
const OFFLOADED_COMMAND_INLINE: &[&str] =
    &["get_store", "crate::get_store", "Arc::clone", "map_err"];

/// What the `dispatch` arm of an [`OFFLOADED_COMMANDS`] entry may call on
/// the runtime: the handle lookups, argument parsing, the caller's project
/// scope, and the handling of a bare `spawn_blocking`'s result.
const OFFLOADED_ARM_INLINE: &[&str] = &[
    "get_store",
    "arg",
    "opt_arg",
    "unwrap_or_default",
    "Arc::clone",
    "clone",
    "control_scope::project_filter",
    "map",
    "map_err",
    "to_string",
    "serde_json::to_value",
    "unwrap",
];

/// [`OFFLOADED_COMMANDS`] entries whose `dispatch` arm still calls the store
/// on the runtime, each with why.
const ARMS_NOT_OFFLOADED: &[&str] = &[
    // Not the Tauri command's body: no `sq` check and no prerun actions. It
    // offloads its default-branch lookup and makes its store writes inline.
    "create_project",
];

/// The walk above only looks for child-process waits, and only in commands
/// that are already async. It passes a command that goes back to a plain
/// `fn`, which runs on the main thread again, and one that calls the store
/// inline, where a lock wait parks a runtime worker.
#[test]
fn offloaded_commands_run_only_the_handoff_on_the_runtime() {
    let src = Crate::load();
    for (path, command) in OFFLOADED_COMMANDS {
        let code = &src.files[src.file(path)].1;
        let (_, is_async, body) = command_fns(code)
            .into_iter()
            .find(|(name, ..)| name == command)
            .unwrap_or_else(|| panic!("{path} no longer defines the `{command}` command"));
        assert!(
            is_async,
            "`{command}` must stay an `async fn`: a plain `fn` command runs on the main \
             thread and freezes the window while it waits"
        );
        let (offloads, stray) =
            left_on_runtime(&body, &["spawn_blocking("], OFFLOADED_COMMAND_INLINE);
        assert!(
            offloads,
            "`{command}` must run its body through `spawn_blocking`"
        );
        assert!(
            stray.is_empty(),
            "`{command}` runs these on a runtime worker; move them into its \
             `spawn_blocking` closure: {stray:?}"
        );
    }
}

/// `dispatch` runs on the same runtime as the async commands, so the arm of
/// a command that had to leave the main thread has the same rule: the store
/// call goes through `blocking_json`, `blocking_store_json` or
/// `spawn_blocking`, and only the handoff stays inline.
#[test]
fn offloaded_commands_dispatch_arms_run_only_the_handoff_on_the_runtime() {
    let arms = dispatch_arms(include_str!("../web_server.rs"));
    for (_, command) in OFFLOADED_COMMANDS {
        let (_, body) = arms
            .iter()
            .find(|(commands, _)| commands.iter().any(|name| name == command))
            .unwrap_or_else(|| panic!("`dispatch` no longer has a `{command}` arm"));
        let (offloads, stray) = left_on_runtime(&code_only(body), OFFLOADED, OFFLOADED_ARM_INLINE);
        if ARMS_NOT_OFFLOADED.contains(command) {
            assert!(
                !stray.is_empty(),
                "the `{command}` dispatch arm now runs only the handoff on the runtime; \
                 remove it from ARMS_NOT_OFFLOADED"
            );
            continue;
        }
        assert!(
            offloads,
            "the `{command}` dispatch arm must run its body through `blocking_json`, \
             `blocking_store_json` or `spawn_blocking`"
        );
        assert!(
            stray.is_empty(),
            "the `{command}` dispatch arm runs these on a runtime worker; move them into \
             its blocking closure: {stray:?}"
        );
    }
    for command in ARMS_NOT_OFFLOADED {
        assert!(
            OFFLOADED_COMMANDS.iter().any(|(_, name)| name == command),
            "`{command}` in ARMS_NOT_OFFLOADED is not an OFFLOADED_COMMANDS entry; remove it"
        );
    }
}

/// What `body` runs on the runtime once the argument list of every
/// `offloaders` call is cut out: whether there was such a call at all, and
/// the calls left over that `inline` does not allow.
fn left_on_runtime(body: &str, offloaders: &[&str], inline: &[&str]) -> (bool, Vec<String>) {
    let kept = cut_offloaded(body, offloaders);
    let stray = calls(&kept)
        .into_iter()
        .map(|call| call.path)
        .filter(|path| !inline.contains(&path.as_str()))
        .collect();
    (kept != body, stray)
}

/// The crate's source, test code removed, and every fn in it.
struct Crate {
    /// Each file's path under `src/`, and its code (see `code_only`).
    files: Vec<(String, String)>,
    fns: Vec<FnItem>,
    /// Module name → the files whose fns a path through it reaches.
    modules: BTreeMap<String, Vec<usize>>,
    /// Per file: the name each `use` binds → the path it imports.
    imports: Vec<BTreeMap<String, String>>,
    /// The `ALLOWED` entries a scan has matched, so a dead one can fail.
    used: RefCell<BTreeSet<(&'static str, &'static str, &'static str)>>,
}

struct FnItem {
    file: usize,
    name: String,
    is_async: bool,
    /// Takes `self`, so it is reachable only as a method call.
    is_method: bool,
    body: String,
}

/// What a body runs inline that blocks, and the crate fns it calls inline.
struct Scan {
    blocking: Vec<String>,
    callees: Vec<usize>,
}

impl Crate {
    fn load() -> Self {
        let mut files = Vec::new();
        read_sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            "",
            &mut files,
        );
        let mut src = Crate {
            fns: Vec::new(),
            modules: BTreeMap::new(),
            imports: files.iter().map(|(_, code)| imports(code)).collect(),
            files,
            used: RefCell::default(),
        };
        for (file, (path, code)) in src.files.iter().enumerate() {
            for module in module_names(path) {
                src.modules.entry(module).or_default().push(file);
            }
            src.fns.extend(fn_items(code).into_iter().map(|mut f| {
                f.file = file;
                f
            }));
        }
        src
    }

    fn file(&self, path: &str) -> usize {
        self.files
            .iter()
            .position(|(p, _)| p == path)
            .unwrap_or_else(|| panic!("{path} should be under src/"))
    }

    /// `owner` names the fn `body` belongs to, for `ALLOWED`.
    fn scan(&self, file: usize, owner: &str, body: &str) -> Scan {
        let allowed = |call: &str| {
            let entry = (self.files[file].0.as_str(), owner, call);
            let hit = ALLOWED.iter().find(|&&allowed| allowed == entry);
            if let Some(&allowed) = hit {
                self.used.borrow_mut().insert(allowed);
            }
            hit.is_some()
        };
        // A file that imports tokio's `Command` spawns async children, and
        // one that uses `tokio::process` at all waits on them asynchronously.
        let tokio_command =
            (self.imports[file].get("Command")).is_some_and(|path| path.starts_with("tokio::"));
        let tokio_child = self.files[file].1.contains("tokio::process");
        let code = cut_offloaded(body, OFFLOADED).replace("tokio::process::Command::new(", "");
        let mut scan = Scan {
            blocking: BLOCKING
                .iter()
                .filter(|&&marker| code.contains(marker) && !allowed(marker))
                .filter(|&&marker| match marker {
                    "Command::new(" => !tokio_command,
                    ".wait()" => !tokio_child,
                    _ => true,
                })
                .map(|marker| format!("`{marker}`"))
                .collect(),
            callees: Vec::new(),
        };
        for call in calls(&code) {
            let name = call.path.rsplit("::").next().unwrap_or_default();
            if allowed(&format!("{name}(")) {
                continue;
            }
            let callees = self.resolve(file, &call);
            if callees.is_empty()
                && !call.awaited
                && BLOCKING_MODULES.contains(&call.qualifier().unwrap_or_default())
            {
                scan.blocking.push(format!("`{}(`", call.path));
            }
            scan.callees.extend(callees);
        }
        scan
    }

    /// The fns `call`, made in `file`, can reach. An awaited method call
    /// reaches every async method of that name; a plain one is not followed.
    fn resolve(&self, file: usize, call: &Call) -> Vec<usize> {
        let name = call.path.rsplit("::").next().unwrap_or_default();
        let matching = |keep: &dyn Fn(&FnItem) -> bool| -> Vec<usize> {
            (self.fns.iter().enumerate())
                .filter(|(_, f)| f.name == name && keep(f))
                .map(|(i, _)| i)
                .collect()
        };
        let in_files = |files: &[usize]| matching(&|f| !f.is_method && files.contains(&f.file));
        if call.is_method {
            if !call.awaited {
                return Vec::new();
            }
            return matching(&|f| f.is_method && f.is_async);
        }
        match call.qualifier() {
            None => match self.imports[file].get(name) {
                Some(path) if path.contains("::") && in_files(&[file]).is_empty() => {
                    let imported = Call {
                        path: path.clone(),
                        ..call.clone()
                    };
                    self.resolve(file, &imported)
                }
                _ => in_files(&[file]),
            },
            Some("self" | "Self") => in_files(&[file]),
            Some(type_name) if type_name.starts_with(char::is_uppercase) => Vec::new(),
            Some(module) => {
                let module = match module {
                    "super" => parent_module(&self.files[file].0),
                    other => other.to_string(),
                };
                in_files(
                    self.modules
                        .get(&module)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                )
            }
        }
    }

    /// `, via `a` → `b` (file.rs)` for a chain of fns, empty for none.
    fn describe(&self, via: &[usize]) -> String {
        let Some(&last) = via.last() else {
            return String::new();
        };
        let names: Vec<String> = via
            .iter()
            .map(|&f| format!("`{}`", self.fns[f].name))
            .collect();
        let file = &self.files[self.fns[last].file].0;
        format!(", via {} ({file})", names.join(" → "))
    }
}

/// Every `.rs` file under `dir` but test-only ones, as (path under `src/`,
/// code with test items removed).
fn read_sources(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for entry in entries {
        let name = entry.file_name().unwrap().to_string_lossy().into_owned();
        let stem = name.trim_end_matches(".rs");
        if entry.is_dir() {
            if name != "tests" {
                read_sources(&entry, &format!("{prefix}{name}/"), out);
            }
        } else if name.ends_with(".rs")
            && !(stem == "tests" || stem.ends_with("_tests") || stem == "test_utils")
        {
            let source = std::fs::read_to_string(&entry).unwrap();
            out.push((
                format!("{prefix}{name}"),
                without_test_items(&code_only(&source)),
            ));
        }
    }
}

/// The module names a path can reach `path`'s fns through: its own, and its
/// directory's, since `git/mod.rs` and friends re-export their submodules.
fn module_names(path: &str) -> Vec<String> {
    let mut parts: Vec<&str> = path.trim_end_matches(".rs").split('/').collect();
    if parts.last() == Some(&"mod") {
        parts.pop();
    }
    if parts == ["lib"] {
        return vec!["crate".to_string()];
    }
    parts.iter().rev().take(2).map(|p| p.to_string()).collect()
}

/// The module `super::` names in `path`.
fn parent_module(path: &str) -> String {
    let mut parts: Vec<&str> = path.trim_end_matches(".rs").split('/').collect();
    if parts.last() == Some(&"mod") {
        parts.pop();
    }
    parts.pop();
    parts.last().unwrap_or(&"crate").to_string()
}

/// A call in a body: the path written before its `(`.
#[derive(Clone)]
struct Call {
    path: String,
    is_method: bool,
    awaited: bool,
}

impl Call {
    /// The path segment before the fn's name, if any.
    fn qualifier(&self) -> Option<&str> {
        self.path.rsplit("::").nth(1)
    }
}

/// Every call in `code` to a lowercase name: fns and methods, not tuple
/// structs, variants or macros.
fn calls(code: &str) -> Vec<Call> {
    code.match_indices('(')
        .filter_map(|(open, _)| {
            let end = before_turbofish(code, open);
            let start = code[..end]
                .trim_end_matches(|c: char| is_ident_char(c) || c == ':')
                .len();
            let path = &code[start..end];
            let name = path.rsplit("::").next()?;
            let before = code[..start].trim_end();
            let is_definition =
                before.ends_with("fn") && !before[..before.len() - 2].ends_with(is_ident_char);
            if !name.starts_with(|c: char| c.is_lowercase() || c == '_') || is_definition {
                return None;
            }
            let after = &code[closing_bracket(code, open) + 1..];
            Some(Call {
                path: path.to_string(),
                is_method: code[..start].ends_with('.'),
                awaited: after.trim_start().starts_with(".await"),
            })
        })
        .collect()
}

/// Where the path of the call whose `(` is at `open` ends, stepping back over
/// a turbofish (`collect::<Vec<_>>(`).
fn before_turbofish(code: &str, open: usize) -> usize {
    if !code[..open].ends_with('>') {
        return open;
    }
    let mut depth = 0;
    for (i, c) in code[..open].char_indices().rev() {
        match c {
            '>' if !code[..i].ends_with('-') => depth += 1,
            '<' => {
                depth -= 1;
                if depth == 0 {
                    return code[..i].strip_suffix("::").map_or(open, str::len);
                }
            }
            _ => {}
        }
    }
    open
}

/// The path each name a `use` in `code` binds imports:
/// `use crate::git::{self, strip_git_env as strip};` binds `strip` to
/// `crate::git::strip_git_env` (and `self` to `crate::git::self`).
fn imports(code: &str) -> BTreeMap<String, String> {
    fn flatten(prefix: &str, tree: &str, out: &mut BTreeMap<String, String>) {
        let tree = tree.trim();
        if let Some(open) = tree.find('{') {
            let prefix = format!("{prefix}{}", &tree[..open]);
            let inner = &tree[open + 1..closing_bracket(tree, open)];
            let mut depth = 0;
            let mut item_start = 0;
            for (i, c) in inner.char_indices().chain([(inner.len(), ',')]) {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    ',' if depth == 0 => {
                        flatten(&prefix, &inner[item_start..i], out);
                        item_start = i + 1;
                    }
                    _ => {}
                }
            }
        } else if !tree.is_empty() {
            let (path, name) = match tree.split_once(" as ") {
                Some((path, name)) => (path.trim(), name.trim()),
                None => (tree, tree.rsplit("::").next().unwrap_or(tree)),
            };
            out.insert(name.to_string(), format!("{prefix}{path}"));
        }
    }

    let mut out = BTreeMap::new();
    for (at, marker) in code.match_indices("use ") {
        if code[..at].ends_with(is_ident_char) {
            continue;
        }
        if let Some(end) = code[at..].find(';') {
            flatten("", &code[at + marker.len()..at + end], &mut out);
        }
    }
    out
}

/// Every fn with a body in `code`. `file` is left for the caller to fill in.
fn fn_items(code: &str) -> Vec<FnItem> {
    code.match_indices("fn ")
        .filter(|(at, _)| !code[..*at].ends_with(is_ident_char))
        .filter_map(|(at, marker)| {
            let (name, after_name) = ident_at(code, at + marker.len());
            let (params, body) = fn_params_and_body(code, after_name)?;
            let first_param = params
                .split(',')
                .next()
                .unwrap_or_default()
                .replace('&', " ");
            let receiver = first_param
                .split_whitespace()
                .find(|word| !word.starts_with('\'') && *word != "mut");
            Some(FnItem {
                file: 0,
                name: name.to_string(),
                is_async: code[..at].trim_end().ends_with("async"),
                is_method: receiver.is_some_and(|r| r == "self" || r.starts_with("self:")),
                body: body.to_string(),
            })
        })
        .filter(|f| !f.name.is_empty())
        .collect()
}

/// Every `#[tauri::command]` fn in `code` as (name, is `async`, body).
fn command_fns(code: &str) -> Vec<(String, bool, String)> {
    code.match_indices("#[tauri::command")
        .filter_map(|(at, _)| {
            let fn_at = at + code[at..].find("fn ")?;
            let is_async = code[at..fn_at].split_whitespace().any(|w| w == "async");
            let (name, after_name) = ident_at(code, fn_at + "fn ".len());
            let (_, body) = fn_params_and_body(code, after_name)?;
            Some((name.to_string(), is_async, body.to_string()))
        })
        .collect()
}

/// The parameter list and the body, braces included, of the `fn` whose name
/// ends at `after_name`; `None` for a declaration without a body.
fn fn_params_and_body(code: &str, after_name: usize) -> Option<(&str, &str)> {
    let mut at = after_name;
    if code[at..].starts_with('<') {
        let mut depth = 0;
        for (i, c) in code[at..].char_indices() {
            match c {
                '<' => depth += 1,
                '>' if !code[..at + i].ends_with('-') => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                at += i + 1;
                break;
            }
        }
    }
    let open_params = at + code[at..].find('(')?;
    let close_params = closing_bracket(code, open_params);
    let rest = close_params + 1;
    let open = rest + code[rest..].find(['{', ';'])?;
    (code.as_bytes()[open] == b'{').then(|| {
        (
            &code[open_params + 1..close_params],
            &code[open..=closing_bracket(code, open)],
        )
    })
}

/// `code` with the argument list of every call to one of `offloaders`
/// cut out, since that code runs on another thread.
fn cut_offloaded(code: &str, offloaders: &[&str]) -> String {
    let mut code = code.to_string();
    for offloader in offloaders {
        while let Some(at) = code.find(offloader) {
            let close = closing_bracket(&code, at + offloader.len() - 1);
            code.replace_range(at..=close, "");
        }
    }
    code
}

/// `source` with its comments dropped and its string and char literals
/// emptied, so a marker can't match prose and brackets can be matched by
/// plain counting. Lifetimes are kept.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut code = String::with_capacity(source.len());
    let mut i = 0;
    while i < chars.len() {
        let rest = &chars[i..];
        let after_ident = i > 0 && is_ident_char(chars[i - 1]);
        if rest.starts_with(&['/', '/']) {
            i += rest.iter().position(|&c| c == '\n').unwrap_or(rest.len());
        } else if rest.starts_with(&['/', '*']) {
            i += rest
                .windows(2)
                .position(|w| w == ['*', '/'])
                .map_or(rest.len(), |at| at + 2);
        } else if let Some(len) = string_literal_len(rest, after_ident) {
            code.push_str("\"\"");
            i += len;
        } else if let Some(len) = char_literal_len(rest) {
            code.push_str("' '");
            i += len;
        } else {
            code.push(rest[0]);
            i += 1;
        }
    }
    code
}

/// The length of the string literal, raw or not, that `rest` starts with.
fn string_literal_len(rest: &[char], after_ident: bool) -> Option<usize> {
    match rest {
        ['"', ..] => {
            let mut i = 1;
            while i < rest.len() && rest[i] != '"' {
                i += if rest[i] == '\\' { 2 } else { 1 };
            }
            Some(i + 1)
        }
        ['r', ..] if !after_ident => {
            let hashes = rest[1..].iter().take_while(|&&c| c == '#').count();
            if rest.get(1 + hashes) != Some(&'"') {
                return None;
            }
            let close: Vec<char> = std::iter::once('"')
                .chain(std::iter::repeat_n('#', hashes))
                .collect();
            let body = 2 + hashes;
            let end = rest[body..]
                .windows(close.len())
                .position(|w| w == close.as_slice())?;
            Some(body + end + close.len())
        }
        _ => None,
    }
}

/// The length of the char literal `rest` starts with; `None` for a lifetime.
fn char_literal_len(rest: &[char]) -> Option<usize> {
    match rest {
        ['\'', '\\', ..] => Some(3 + rest.get(3..)?.iter().position(|&c| c == '\'')? + 1),
        ['\'', _, '\'', ..] => Some(3),
        _ => None,
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The index of the bracket that closes the one at `open`, in code
/// `code_only` has already cleared of literals and comments.
fn closing_bracket(code: &str, open: usize) -> usize {
    let bytes = code.as_bytes();
    let close = match bytes[open] {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        other => panic!("no bracket at {open}: {:?}", other as char),
    };
    let mut depth = 0usize;
    for (i, &byte) in bytes.iter().enumerate().skip(open) {
        if byte == bytes[open] {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
    }
    panic!("unbalanced {:?} at {open}", bytes[open] as char)
}

/// `code` without its `#[cfg(test)]` modules, fns, impls and `use`s.
fn without_test_items(code: &str) -> String {
    const CFG_TEST: &str = "#[cfg(test)]";
    let mut code = code.to_string();
    let mut from = 0;
    while let Some(at) = code[from..].find(CFG_TEST).map(|i| from + i) {
        // Step over any further attributes to the item itself.
        let mut item = at + CFG_TEST.len();
        loop {
            item = code.len() - code[item..].trim_start().len();
            if !code[item..].starts_with("#[") {
                break;
            }
            item = closing_bracket(&code, item + 1) + 1;
        }
        let keyword = code[item..]
            .split_whitespace()
            .find(|word| !word.starts_with("pub") && !["async", "unsafe"].contains(word));
        let end = code[item..].find(['{', ';']).map(|i| item + i);
        match (keyword, end) {
            (Some("mod" | "fn" | "impl" | "use"), Some(end)) => {
                let end = match code.as_bytes()[end] {
                    b'{' => closing_bracket(&code, end),
                    _ => end,
                };
                code.replace_range(at..=end, "");
                from = at;
            }
            _ => from = at + 1,
        }
    }
    code
}

/// The identifier starting at `at`, and the index just past it.
fn ident_at(code: &str, at: usize) -> (&str, usize) {
    let end = code[at..]
        .find(|c: char| !is_ident_char(c))
        .map_or(code.len(), |i| at + i);
    (&code[at..end], end)
}
