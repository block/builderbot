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
//!
//! The commands that are still plain `fn`s run on the main thread, where any
//! wait freezes the window. `synchronous_commands_touch_only_in_process_state`
//! holds them to in-process state: each is listed by name, and the same walk
//! looks for the store, a child process or the filesystem in what it runs.
//!
//! What these tests do not cover: a store call made inline on the runtime by
//! an `async fn` command or a `dispatch` arm that is not in
//! `OFFLOADED_COMMANDS`. The walk looks for child-process waits only, and the
//! handoff-only rule is applied by name to the commands that used to be
//! synchronous. So the guards keep the store off the main thread; they do not
//! keep it off the runtime workers. Commands that were `async` before those
//! conversions still call the store inline (`ensure_review`, `find_review`
//! and `add_comment` in `review_commands.rs` are three, and the
//! `ensure_repo_badges`, `list_repos_for_home` and `list_all_repo_actions`
//! arms do the same), where a slow holder of the store lock parks a worker.
//! A new `async fn` command that does so passes every test here unless it is
//! added to `OFFLOADED_COMMANDS`.

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
        for (name, is_async, _, body) in command_fns(code) {
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
///
/// Not every async command that reaches the store: the ones that were
/// already `async` are not listed, and some of them call the store inline.
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
    // The writes sent on every note save, note open, chat open or close and
    // queued follow-up. The frontend keeps the order of the ones it does not
    // await (the `writeOrder` queue in `commands.ts`).
    ("note_commands.rs", "create_note"),
    ("note_commands.rs", "update_note"),
    ("note_commands.rs", "set_note_read"),
    ("session_commands.rs", "mark_session_chat_read"),
    ("session_commands.rs", "queue_session_message"),
    // Every other command that reached the store, a child process or the
    // filesystem from the main thread. What is still a plain `fn` is in
    // `SYNCHRONOUS_COMMANDS` below.
    //
    // Store reset and quit: file deletes and a schema build, and the read of
    // the sessions a quit would stop.
    ("lib.rs", "confirm_reset_store"),
    ("app_lifecycle.rs", "quit_app"),
    ("launch_resume/mod.rs", "resume_interrupted_sessions"),
    // Repos, badges and pins.
    ("lib.rs", "list_recent_repos"),
    ("lib.rs", "get_suggested_repos"),
    ("lib.rs", "clear_project_repo_reason"),
    ("lib.rs", "set_primary_project_repo"),
    ("lib.rs", "update_project_repo_branch_name"),
    ("lib.rs", "delete_repo_badge"),
    ("lib.rs", "update_repo_badge"),
    ("lib.rs", "unpin_repo"),
    ("lib.rs", "reorder_pinned_repos"),
    ("lib.rs", "set_repo_default_branch"),
    // `git remote show origin` or `gh repo view`.
    ("lib.rs", "detect_default_branch"),
    // Actions. The two lists insert the action context on first use.
    ("lib.rs", "list_project_actions"),
    ("lib.rs", "update_project_action"),
    ("lib.rs", "delete_project_action"),
    ("lib.rs", "list_action_contexts"),
    ("lib.rs", "list_repo_actions"),
    ("lib.rs", "create_repo_action"),
    ("lib.rs", "delete_all_repo_actions"),
    ("lib.rs", "delete_action_context"),
    // Branches. `create_branch` asks GitHub for the default branch.
    ("branches.rs", "get_branch"),
    ("branches.rs", "create_branch"),
    // Images: store rows, and whole files read and base64-encoded.
    ("image_commands.rs", "get_image_path"),
    ("image_commands.rs", "list_branch_images"),
    ("image_commands.rs", "get_image_data"),
    ("image_commands/tool_results.rs", "read_image_file"),
    // Notes.
    ("note_commands.rs", "get_branch_note_by_session"),
    ("note_commands.rs", "get_note"),
    ("note_commands.rs", "list_child_notes"),
    ("note_commands.rs", "create_project_note"),
    ("note_commands.rs", "get_project_note"),
    ("note_commands.rs", "get_project_note_by_session"),
    ("note_commands.rs", "set_project_note_read"),
    ("note_transfer.rs", "get_note_repo"),
    // Pull requests.
    ("prs/commands.rs", "update_branch_pr"),
    ("prs/commands.rs", "clear_branch_pr_status"),
    ("prs/commands.rs", "cancel_deferred_push"),
    // Sessions. `queue_branch_session` can discover the installed agents
    // through login shells.
    ("session_commands.rs", "list_child_sessions"),
    ("session_commands.rs", "get_session_acp_metadata_messages"),
    (
        "session_commands.rs",
        "get_session_acp_metadata_messages_since",
    ),
    ("session_commands.rs", "get_tool_image"),
    ("session_commands.rs", "get_tool_call_body"),
    ("session_commands.rs", "get_session_acp_initialization"),
    ("session_commands.rs", "count_assistant_messages_after"),
    ("session_commands.rs", "list_queued_session_messages"),
    ("session_commands.rs", "delete_queued_session_message"),
    ("session_commands.rs", "get_session_live_delivery"),
    ("session_commands.rs", "queue_branch_session"),
    // Tags: one store transaction each.
    ("tag_commands.rs", "get_tag_state"),
    ("tag_commands.rs", "create_tag_group"),
    ("tag_commands.rs", "rename_tag_group"),
    ("tag_commands.rs", "set_icon_tag_group"),
    ("tag_commands.rs", "set_tag_group_sidebar"),
    ("tag_commands.rs", "reorder_tag_groups"),
    ("tag_commands.rs", "delete_tag_group"),
    ("tag_commands.rs", "create_tag"),
    ("tag_commands.rs", "update_tag"),
    ("tag_commands.rs", "move_tag"),
    ("tag_commands.rs", "delete_tag"),
    ("tag_commands.rs", "set_project_group_tag"),
    ("tag_commands.rs", "set_project_tag_assigned"),
    // Connected apps: grant rows in the store.
    ("control_server/commands.rs", "list_control_grants"),
    ("control_server/commands.rs", "revoke_control_grant"),
    // The opener spawns `open`; the other two read the filesystem.
    ("util_commands.rs", "open_url"),
    ("util_commands.rs", "read_text_file"),
    ("util_commands.rs", "resolve_path_aliases"),
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

/// [`OFFLOADED_COMMANDS`] entries `dispatch` refuses in web mode (or leaves
/// to the browser), so their arm has nothing to hand off.
const ARMS_WITH_NOTHING_TO_OFFLOAD: &[&str] = &[
    "confirm_reset_store",
    "quit_app",
    "resume_interrupted_sessions",
    "list_control_grants",
    "revoke_control_grant",
    "open_url",
];

/// The walk above only looks for child-process waits, and only in commands
/// that are already async. It passes a command that goes back to a plain
/// `fn`, which runs on the main thread again, and one that calls the store
/// inline, where a lock wait parks a runtime worker.
///
/// This holds the [`OFFLOADED_COMMANDS`] entries to the handoff, and only
/// them. An async command outside the list is not checked for inline store
/// calls by any test (see the module docs).
#[test]
fn offloaded_commands_run_only_the_handoff_on_the_runtime() {
    let src = Crate::load();
    for (path, command) in OFFLOADED_COMMANDS {
        let code = &src.files[src.file(path)].1;
        let (_, is_async, _, body) = command_fns(code)
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
        if ARMS_WITH_NOTHING_TO_OFFLOAD.contains(command) {
            assert!(
                !offloads && stray.is_empty(),
                "the `{command}` dispatch arm now does work; remove it from \
                 ARMS_WITH_NOTHING_TO_OFFLOAD: {stray:?}"
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
    for command in ARMS_NOT_OFFLOADED
        .iter()
        .chain(ARMS_WITH_NOTHING_TO_OFFLOAD)
    {
        assert!(
            OFFLOADED_COMMANDS.iter().any(|(_, name)| name == command),
            "`{command}` is listed as an arm exception but is not an OFFLOADED_COMMANDS \
             entry; remove it"
        );
    }
}

/// The commands that stay plain `fn`s, as (file, name). A plain `fn` command
/// runs on the main thread, so the window is frozen for as long as it takes.
/// Each of these only reads or flips state held in memory: a registry or
/// scheduler behind its own short-lived mutex, an atomic flag, an environment
/// variable, a path computed without touching the disk.
///
/// A new command belongs here only if that is all it does. One that reads or
/// writes the store, waits on a child process or touches the filesystem is
/// an `async fn` that hands its body to `spawn_blocking` (see
/// [`OFFLOADED_COMMANDS`]). The walk cannot see through method calls, so
/// read what a new entry's methods do before listing it.
const SYNCHRONOUS_COMMANDS: &[(&str, &str)] = &[
    // The running-action registry and executor. Stop sends one signal and
    // leaves the escalation to a thread.
    ("actions/commands.rs", "stop_branch_action"),
    ("actions/commands.rs", "get_running_branch_actions"),
    ("actions/commands.rs", "get_all_running_actions"),
    ("actions/commands.rs", "get_action_output_buffer"),
    ("actions/commands.rs", "clear_action_execution"),
    // An environment variable.
    ("branches.rs", "get_blox_env"),
    // The pending connection requests, held in memory. An approval is only
    // recorded here; the grant row is written when the CLI collects it.
    (
        "control_server/commands.rs",
        "list_pending_control_authorizations",
    ),
    ("control_server/commands.rs", "answer_control_authorization"),
    // The launch's offer and the store-incompatibility flag.
    ("launch_resume/mod.rs", "get_interrupted_sessions"),
    ("lib.rs", "get_store_status"),
    // Paths joined onto the home directory; nothing is opened.
    ("lib.rs", "get_repo_clone_path"),
    ("util_commands.rs", "preferences_store_path"),
    // Hints for the PR poll loop, which does its own store reads.
    ("pr_poll_scheduler.rs", "set_foreground_project"),
    ("pr_poll_scheduler.rs", "set_focus"),
    ("pr_poll_scheduler.rs", "set_branch_pending"),
    ("pr_poll_scheduler.rs", "refresh_now"),
    ("pr_poll_scheduler.rs", "disconnect_client"),
    // The transfer and preflight registries: a flag set, a list cloned.
    ("project_transfer/commands.rs", "cancel_project_transfer"),
    ("project_transfer/commands.rs", "list_transferring_projects"),
    (
        "project_transfer/preflight_scan.rs",
        "cancel_project_transfer_preflight",
    ),
    // The session registry's hold snapshot, and the reconciler's row states.
    // `reconcile_session` claims the row and runs the attempt on a thread.
    ("session_commands.rs", "get_session_background_hold"),
    ("session_runner/reconcile.rs", "reconcile_session"),
    (
        "session_runner/reconcile.rs",
        "get_session_reconcile_states",
    ),
    // Per-window seeds and the updater claim.
    ("window_commands.rs", "take_window_seed"),
    ("window_commands.rs", "claim_updater_ownership"),
];

/// Plain `fn` commands that do reach the store, a child process or the
/// filesystem and stay on the main thread anyway, as (file, name), each with
/// why. Keep this short: every entry is a window freeze waiting for a slow
/// holder of the store.
const SYNCHRONOUS_BLOCKING_ALLOWED: &[(&str, &str)] = &[
    // Stop. For a running, registered turn it only fires the session's
    // cancellation token in the registry. The store is reached when no runner
    // owns the row (queued, paused, disconnected, or already finished): the
    // row is read, and if it is still live it is written `cancelled`, its
    // pipeline dependents are skipped and the queue is kicked. That is
    // several store lock waits on the main thread, each normally short.
    //
    // It stays synchronous for its ordering. The registry and that fallback
    // write are keyed by session id alone, not by turn. On the main thread a
    // Stop finishes before any command sent after it is dispatched, so it can
    // only end the turn that was live when the user pressed it. The case
    // that matters is a turn that ended by itself as Stop was pressed, with
    // the next message sent straight after: on the pool, a Stop that lost
    // the store lock to that resume would cancel the new turn in the
    // registry, or, between the resume's claim of the row and its
    // registration, write `cancelled` over a turn that is starting.
    // Converting it needs a turn token in the registry and the store write,
    // or the frontend sending Stop and every turn-starting command for a
    // session in order.
    ("session_commands.rs", "cancel_session"),
];

/// Calls whose argument list runs off the main thread: the [`OFFLOADED`]
/// ones, and tasks spawned onto the runtime.
const OFF_MAIN_THREAD: &[&str] = &[
    "spawn_blocking(",
    "thread::spawn(",
    "blocking_json(",
    "blocking_store_json(",
    "run_blox_blocking(",
    "async_runtime::spawn(",
    "tokio::spawn(",
];

/// The store: its type, the handle lookups, and a call on a `store` value.
/// `store::` paths (models, `now_timestamp`) are not the database.
const STORE_MARKERS: &[&str] = &["Store", "get_store", "store_of", "store."];

/// The filesystem and the processes the [`BLOCKING`] list does not cover.
const FILESYSTEM_AND_OPENER_MARKERS: &[&str] = &[
    "fs::",
    "File::",
    "canonicalize(",
    "read_dir(",
    ".exists()",
    ".is_file()",
    ".is_dir()",
    ".metadata()",
    // The preferences file, loaded and saved through the store plugin.
    "preferences_store(",
    // `open`, spawned by the opener plugin.
    ".opener()",
];

/// A plain `fn` command runs on the main thread, and the window is frozen
/// until it returns. The freezes this file's other tests keep away came from
/// such commands waiting on the store lock, on `gh`, on a login shell or on
/// the disk. So every plain `fn` command is listed, and none may reach the
/// store, a child process or the filesystem: in its own signature and body,
/// or in the crate fns it calls inline (the walk of the async check, here
/// for the main thread).
///
/// The walk does not follow method calls, so a registry method that took the
/// store would pass it. That is why the list is by name: a new synchronous
/// command fails here until someone has read what it calls.
#[test]
fn synchronous_commands_touch_only_in_process_state() {
    let src = Crate::load();
    let mut offences = Vec::new();
    let mut synchronous = BTreeSet::new();
    for (file, (path, code)) in src.files.iter().enumerate() {
        for (name, is_async, params, body) in command_fns(code) {
            if is_async {
                continue;
            }
            let reached = src.main_thread_reach(file, &format!("{params} {body}"));
            let entry = (path.as_str(), name.as_str());
            let listed = SYNCHRONOUS_COMMANDS.contains(&entry);
            let allowed = SYNCHRONOUS_BLOCKING_ALLOWED.contains(&entry);
            let reaches = reached.iter().cloned().collect::<Vec<_>>().join("; ");
            match (listed, allowed, reached.is_empty()) {
                (true, true, _) => offences.push(format!(
                    "`{name}` ({path}) is in both SYNCHRONOUS_COMMANDS and \
                     SYNCHRONOUS_BLOCKING_ALLOWED"
                )),
                (true, false, true) | (false, true, false) => {}
                (true, false, false) => offences.push(format!(
                    "`{name}` ({path}) is listed as in-process but reaches {reaches}. Make it \
                     an `async fn` that runs its body under `spawn_blocking` and move it to \
                     OFFLOADED_COMMANDS"
                )),
                (false, true, true) => offences.push(format!(
                    "`{name}` ({path}) no longer reaches the store, a process or the \
                     filesystem; move it from SYNCHRONOUS_BLOCKING_ALLOWED to \
                     SYNCHRONOUS_COMMANDS"
                )),
                (false, false, false) => offences.push(format!(
                    "`{name}` ({path}) is a plain `fn` command, so it runs on the main thread, \
                     and it reaches {reaches}. Make it an `async fn` that runs its body under \
                     `spawn_blocking` and add it to OFFLOADED_COMMANDS"
                )),
                (false, false, true) => offences.push(format!(
                    "`{name}` ({path}) is a new plain `fn` command, so it runs on the main \
                     thread. The scan found nothing that blocks, but it does not follow \
                     method calls: if every call it makes stays in memory, add it to \
                     SYNCHRONOUS_COMMANDS; otherwise make it an `async fn` that runs its \
                     body under `spawn_blocking`"
                )),
            }
            synchronous.insert((path.clone(), name));
        }
    }
    for (path, name) in SYNCHRONOUS_COMMANDS
        .iter()
        .chain(SYNCHRONOUS_BLOCKING_ALLOWED)
    {
        if !synchronous.contains(&(path.to_string(), name.to_string())) {
            offences.push(format!(
                "`{name}` ({path}) is listed as a synchronous command but is no longer a \
                 plain `fn` command there; remove the entry"
            ));
        }
    }
    assert!(
        offences.is_empty(),
        "synchronous commands must touch in-process state only:\n{}",
        offences.join("\n")
    );
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
    params: String,
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

    /// What code starting at `text`, in `file`, reaches on the calling thread
    /// that can hold it up: the store, a child process, the filesystem. Each
    /// is reported with the chain of crate fns that led to it. `text` is a
    /// command's parameter list and body; the fns it calls inline are walked
    /// the same way, up to `MAX_DEPTH` deep.
    fn main_thread_reach(&self, file: usize, text: &str) -> BTreeSet<String> {
        let mut reached = BTreeSet::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([(file, text.to_string(), Vec::new())]);
        while let Some((file, text, via)) = queue.pop_front() {
            let code = cut_offloaded(&text, OFF_MAIN_THREAD);
            let markers = (STORE_MARKERS.iter())
                .chain(FILESYSTEM_AND_OPENER_MARKERS)
                .filter(|marker| names(&code, marker))
                .chain(BLOCKING.iter().filter(|marker| code.contains(**marker)));
            for marker in markers {
                reached.insert(format!("`{marker}`{}", self.describe(&via)));
            }
            for call in calls(&code) {
                let callees = self.resolve(file, &call);
                if callees.is_empty()
                    && BLOCKING_MODULES.contains(&call.qualifier().unwrap_or_default())
                {
                    reached.insert(format!("`{}(`{}", call.path, self.describe(&via)));
                }
                if via.len() == MAX_DEPTH {
                    continue;
                }
                for callee in callees {
                    if seen.insert(callee) {
                        let f = &self.fns[callee];
                        queue.push_back((
                            f.file,
                            format!("{} {}", f.params, f.body),
                            [via.as_slice(), &[callee]].concat(),
                        ));
                    }
                }
            }
        }
        reached
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
                params: params.to_string(),
                body: body.to_string(),
            })
        })
        .filter(|f| !f.name.is_empty())
        .collect()
}

/// Every `#[tauri::command]` fn in `code` as (name, is `async`, parameter
/// list, body).
fn command_fns(code: &str) -> Vec<(String, bool, String, String)> {
    code.match_indices("#[tauri::command")
        .filter_map(|(at, _)| {
            let fn_at = at + code[at..].find("fn ")?;
            let is_async = code[at..fn_at].split_whitespace().any(|w| w == "async");
            let (name, after_name) = ident_at(code, fn_at + "fn ".len());
            let (params, body) = fn_params_and_body(code, after_name)?;
            Some((
                name.to_string(),
                is_async,
                params.to_string(),
                body.to_string(),
            ))
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

/// Whether `code` names `marker`: as text, and where the marker starts with
/// an identifier, not as the tail of a longer one (`Store` is not in
/// `StoreChange`, nor `store.` in `needs_restore.`).
fn names(code: &str, marker: &str) -> bool {
    let whole_word = marker.starts_with(is_ident_char);
    code.match_indices(marker).any(|(at, _)| {
        let after = &code[at + marker.len()..];
        let ends = !marker.ends_with(is_ident_char) || !after.starts_with(is_ident_char);
        ends && !(whole_word && code[..at].ends_with(is_ident_char))
    })
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
