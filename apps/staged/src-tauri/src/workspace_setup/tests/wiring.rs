//! That the triggers and checks are still called from where they should be.
//!
//! The behaviour of setup, the pollers' decisions and the session-start
//! check are tested against a faked workstation. What calls them is not:
//! the startup pass, the pollers' effects, the resume command and the
//! session and pipeline start paths all need a concrete app runtime to
//! drive. Deleting one of those calls would leave every behaviour test
//! passing and the bug back, so each call is pinned here by source text,
//! like the claim invariants in `session_commands.rs`.

const LIB: &str = include_str!("../../lib.rs");
const SETUP: &str = include_str!("../../workspace_setup.rs");
const HOST: &str = include_str!("../host.rs");
const CLONE: &str = include_str!("../clone.rs");
const POLL: &str = include_str!("../poll.rs");
const VERIFY: &str = include_str!("../verify.rs");
const BRANCHES: &str = include_str!("../../branches.rs");
const WEB_SERVER: &str = include_str!("../../web_server.rs");
const SESSION_COMMANDS: &str = include_str!("../../session_commands.rs");
const PR_CONTEXT: &str = include_str!("../../prs/context.rs");
const PR_COMMIT: &str = include_str!("../../prs/commit.rs");
const PR_GIT_PIPELINE: &str = include_str!("../../prs/git_pipeline.rs");
const PR_PUSH: &str = include_str!("../../prs/push.rs");
const PR_CREATE: &str = include_str!("../../prs/create_pr.rs");
const PR_PULL: &str = include_str!("../../prs/pull.rs");
const TRANSFER: &str = include_str!("../../project_transfer/orchestrator.rs");
const TRANSFER_COMMANDS: &str = include_str!("../../project_transfer/commands.rs");

/// The text of the function whose signature starts with `signature`, to the
/// closing brace at its own indentation.
fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
    let at = source
        .find(signature)
        .unwrap_or_else(|| panic!("no `{signature}`"));
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = source[line_start..]
        .chars()
        .take_while(|c| *c == ' ')
        .collect();
    let body = &source[at..];
    let end = body
        .find(&format!("\n{indent}}}\n"))
        .unwrap_or_else(|| panic!("no end of `{signature}`"));
    &body[..end]
}

/// The web server's dispatch arm for `command`, to the next arm.
fn web_arm(command: &str) -> &'static str {
    let start = format!("\n        \"{command}\" => {{");
    let at = WEB_SERVER
        .find(&start)
        .unwrap_or_else(|| panic!("no web arm for `{command}`"));
    let arm = &WEB_SERVER[at + 1..];
    let end = arm.find("\n        \"").expect("the next arm");
    &arm[..end]
}

/// `first` is in `body`, and before `then`.
#[track_caller]
fn assert_before(what: &str, body: &str, first: &str, then: &str) {
    let first_at = body
        .find(first)
        .unwrap_or_else(|| panic!("`{what}` no longer calls `{first}`"));
    let then_at = body
        .find(then)
        .unwrap_or_else(|| panic!("`{what}` has no `{then}`"));
    assert!(
        first_at < then_at,
        "`{what}` must call `{first}` before `{then}`"
    );
}

#[track_caller]
fn assert_calls(what: &str, body: &str, call: &str) {
    assert!(body.contains(call), "`{what}` no longer calls `{call}`");
}

// ── What triggers setup ─────────────────────────────────────────────────────

/// The startup pass runs at launch, after interrupted transfers are
/// recovered, and does something for both kinds of row it lists: sets up
/// the ones left `Starting`, sets the retry of the ones left `Error`.
#[test]
fn the_startup_pass_is_run_and_acts_on_what_it_finds() {
    assert_before(
        "lib.rs",
        LIB,
        "project_transfer::recover_interrupted_transfers(",
        "workspace_setup::resume_pending_setups(",
    );

    let pass = body_of(SETUP, "pub(crate) fn resume_pending_setups<");
    assert_calls(
        "resume_pending_setups",
        pass,
        "branches_awaiting_setup(store)",
    );
    assert_before(
        "resume_pending_setups",
        pass,
        "Some(StartupAction::SetUp) => {",
        "trigger_branch_setup(app_handle, store, &branch.id, false)",
    );
    assert_calls(
        "resume_pending_setups",
        pass,
        "Some(StartupAction::ArmRetry) => arm_retry(app_handle, store, &branch.id)",
    );
}

/// A trigger is an `Observed` setup of that branch, and a retry timer is
/// one on the shared guard.
#[test]
fn a_trigger_runs_setup_and_a_retry_is_armed_on_the_shared_guard() {
    let trigger = body_of(SETUP, "pub(crate) fn trigger_branch_setup<");
    assert_calls("trigger_branch_setup", trigger, "SetupGuard::global()");
    assert_calls(
        "trigger_branch_setup",
        trigger,
        "SetupRequest::new(SetupTrigger::Observed)",
    );
    assert_calls(
        "trigger_branch_setup",
        trigger,
        "run_guarded(guard, &store, &host, &branch_id, request)",
    );
    let ensure = body_of(SETUP, "pub(crate) async fn ensure_branch_set_up<");
    assert_calls("ensure_branch_set_up", ensure, "run_guarded(");
    assert_calls("ensure_branch_set_up", ensure, "SetupGuard::global()");

    let arm = body_of(SETUP, "pub(crate) fn arm_retry<");
    assert_calls("arm_retry", arm, "retry::arm(");
    assert_calls("arm_retry", arm, "SetupGuard::global()");

    let failed = body_of(SETUP, "pub(crate) fn branch_failed<");
    assert_calls(
        "branch_failed",
        failed,
        "failures::set(app_handle, store, branch_id, Some(reason), user_asked)",
    );
    assert_calls(
        "branch_failed",
        failed,
        "arm_retry(app_handle, store, branch_id)",
    );
}

/// The real host announces failure reasons, runs the retry it is asked for,
/// and asks the reconcile service about a disconnected session.
#[test]
fn the_real_host_announces_retries_and_reconciles() {
    let reason = body_of(
        HOST,
        "    fn failure_changed(&self, branch_id: &str, error: Option<&str>, user_asked: bool) {\n        failures",
    );
    assert_calls(
        "BloxHost::failure_changed",
        reason,
        "failures::set(&self.app_handle, &self.store, branch_id, error, user_asked)",
    );

    let later = body_of(
        HOST,
        "    fn retry_later(&self, branch_id: &str, delay: Duration, ticket: u64) {",
    );
    assert_before(
        "BloxHost::retry_later",
        later,
        "tokio::time::sleep(delay).await",
        "retry::fire(SetupGuard::global(), &store, &host, &branch_id, ticket)",
    );

    let check = body_of(
        HOST,
        "    fn check_disconnected_session(&self, session_id: &str) {",
    );
    assert_calls(
        "BloxHost::check_disconnected_session",
        check,
        "session_runner::reconcile::reconcile_session(",
    );
}

/// Both pollers act through the app's effects, and those effects reach
/// setup: a branch seen `Starting` behind a running workstation is set up,
/// one seen in `Error` gets its retry, one the poll fails gets a reason.
#[test]
fn the_pollers_effects_reach_setup() {
    let effects = body_of(POLL, "fn app_effects<");
    for call in [
        "branches::workspace_status_written(app_handle, branch_id, changed, status)",
        "super::trigger_branch_setup(app_handle, store, branch_id, false)",
        "super::arm_retry(app_handle, store, branch_id)",
        "super::branch_failed(app_handle, store, branch_id, reason)",
    ] {
        assert_calls("app_effects", effects, call);
    }

    let single = body_of(POLL, "pub(crate) async fn poll_workspace_status(");
    assert_calls(
        "poll_workspace_status",
        single,
        "&app_effects(store, app_handle)",
    );
    let batch = body_of(POLL, "pub(crate) async fn poll_all_workspace_statuses(");
    assert_calls(
        "poll_all_workspace_statuses",
        batch,
        "&app_effects(store, app_handle)",
    );
}

/// A resume that worked leaves its branches `Starting` and triggers setup
/// for each, as the user's ask; one that failed gives each its reason and
/// its retry.
#[test]
fn a_resume_triggers_setup_or_says_why_it_failed() {
    let resume = body_of(BRANCHES, "pub(crate) async fn resume_workspace_impl(");
    assert_calls(
        "resume_workspace_impl",
        resume,
        "crate::workspace_setup::trigger_branch_setup(app_handle, store, branch_id, true)",
    );
    assert_calls(
        "resume_workspace_impl",
        resume,
        "crate::workspace_setup::branch_failed(app_handle, store, branch_id, &reason)",
    );
    assert!(
        !resume.contains("WorkspaceStatus::Running"),
        "a resume must not say `Running`: setup does, after the checkout"
    );
}

/// The command and its web arm only ask for setup: neither waits for the
/// run, whose outcome reaches a client through the row and the reason event.
#[test]
fn start_workspace_only_asks_for_setup() {
    let command = body_of(BRANCHES, "pub async fn start_workspace(");
    assert_calls("start_workspace", command, "Result<(), String>");
    assert_calls(
        "start_workspace",
        command,
        "crate::workspace_setup::request_branch_setup(&app_handle, &store, &branch_id)",
    );
    let arm = web_arm("start_workspace");
    assert_calls(
        "the start_workspace web arm",
        arm,
        "crate::workspace_setup::request_branch_setup(app_handle, &store, &branch_id)",
    );
    for (what, body) in [("start_workspace", command), ("its web arm", arm)] {
        assert!(!body.contains(".await"), "{what} must not wait on setup");
    }

    let request = body_of(SETUP, "pub(crate) fn request_setup(");
    assert_calls(
        "request_setup",
        request,
        "SetupRequest::new(SetupTrigger::Requested)",
    );
    assert_calls("request_setup", request, "start_run(");
    assert!(!request.contains(".await"), "a request must not wait");

    assert_calls(
        "the invoke handler",
        LIB,
        "workspace_setup::failures::get_workspace_setup_errors",
    );
    assert_calls(
        "the get_workspace_setup_errors web arm",
        web_arm("get_workspace_setup_errors"),
        "crate::workspace_setup::failures::get_workspace_setup_errors()",
    );
}

// ── What writes `Running` ───────────────────────────────────────────────────

/// The text from each call of `write` in `source` to the end of its
/// statement.
fn status_writes<'a>(source: &'a str, write: &str) -> Vec<&'a str> {
    source
        .match_indices(write)
        .map(|(at, _)| {
            let call = &source[at..];
            &call[..call.find(';').unwrap_or(call.len())]
        })
        .collect()
}

/// Outside setup, nothing in the commands, the pollers, the session starts
/// or the web server writes a literal `Running`. Setup writes it in two
/// places: after a verified checkout, and when it puts back a `Running`
/// row whose repair was blocked by something outside the clone.
#[test]
fn only_setup_writes_running() {
    for (name, source) in [
        ("branches.rs", BRANCHES),
        ("workspace_setup/poll.rs", POLL),
        ("workspace_setup/verify.rs", VERIFY),
        ("session_commands.rs", SESSION_COMMANDS),
        ("web_server.rs", WEB_SERVER),
    ] {
        let production = source.split("#[cfg(test)]").next().unwrap();
        for write in [
            "update_branch_workspace_status(",
            "update_workspace_status_by_workspace_name(",
        ] {
            for call in status_writes(production, write) {
                assert!(
                    !call.contains("WorkspaceStatus::Running"),
                    "{name} writes `Running` outside setup: {call}"
                );
            }
        }
    }

    let running_writes = SETUP
        .matches("update_branch_workspace_status(branch_id, &WorkspaceStatus::Running)")
        .count();
    assert_eq!(running_writes, 2);
    let set_up = body_of(SETUP, "pub(crate) async fn set_up_branch(");
    assert_before(
        "set_up_branch",
        set_up,
        "provision_and_check_out(",
        "update_branch_workspace_status(branch_id, &WorkspaceStatus::Running)",
    );
    let check_out = body_of(CLONE, "pub(super) async fn check_out_in_clone(");
    assert_before(
        "check_out_in_clone",
        check_out,
        "host.check_out(checkout).await?",
        "verify::confirm_checked_out(host, checkout)",
    );
}

/// A transfer that resumed a source workspace does not mark its rows
/// `Running` wholesale. It marks the ones its own inspection read as on
/// their branch, after that inspection, and leaves the rest `Starting`.
/// Setup refuses a transferring branch, so the command asks for it once
/// the transfer has ended without moving the project, before anything held
/// for the transfer is released. A transfer that stops between the resume
/// and that inspection (a cancel, an inspection that fails) read nothing,
/// and leaves every row of a workspace that came back `Starting`.
#[test]
fn a_transfer_marks_running_only_the_rows_it_read_on_their_branch() {
    assert!(
        !TRANSFER.contains("update_workspace_status_by_workspace_name("),
        "a transfer must not set a whole workspace's rows at once"
    );
    let mark = body_of(TRANSFER, "    fn mark_resumed_rows_running(");
    assert_before(
        "mark_resumed_rows_running",
        mark,
        "let status = if row.is_on_its_branch() {\n                    WorkspaceStatus::Running\n                } else {\n                    WorkspaceStatus::Starting\n                };",
        "update_branch_workspace_status(&source.branch.id, &status)",
    );
    assert_eq!(mark.matches("WorkspaceStatus::Running").count(), 1);
    let transfer = body_of(
        TRANSFER_COMMANDS,
        "pub(crate) async fn transfer_project_impl(",
    );
    assert_before(
        "transfer_project_impl",
        transfer,
        "crate::workspace_setup::set_up_branches_left_starting(&app_handle, &store, &branch_ids);",
        "crate::session_queue::release_held_work(&app_handle, &store, &branch_ids)",
    );
    let left_starting = body_of(SETUP, "pub(crate) fn set_up_branches_left_starting<");
    assert_calls(
        "set_up_branches_left_starting",
        left_starting,
        "trigger_branch_setup(app_handle, store, branch_id, false);",
    );
    // The one other status write: `Starting`, on the two ways out between
    // the resume and the inspection that follows it.
    let starting = body_of(TRANSFER, "    fn mark_resumed_rows_starting(");
    assert_calls(
        "mark_resumed_rows_starting",
        starting,
        "update_branch_workspace_status(&source.branch.id, &WorkspaceStatus::Starting)",
    );
    assert!(!starting.contains("WorkspaceStatus::Running"));
    let inspect = body_of(
        TRANSFER,
        "    fn inspect(&self) -> Result<Inspection, Failure> {",
    );
    assert_eq!(
        inspect
            .matches("self.mark_resumed_rows_starting(&inspection, &resumed);")
            .count(),
        2,
        "a cancel after a resume and a failed second inspection both leave the rows Starting"
    );
    assert_eq!(
        TRANSFER.matches("update_branch_workspace_status(").count(),
        2,
        "a new status write in the transfer needs its own reading of the clone"
    );
    assert_eq!(
        TRANSFER.matches("self.mark_resumed_rows_running(").count(),
        1
    );
}

// ── What checks the clone before an agent or a pipeline starts ──────────────

/// Every way a session starts on a branch reads the clone first, and keeps
/// the clone ([`super::super::verify::CheckoutHold`]) until its row is
/// live: the check comes before the claim or the launch, and its hold is
/// bound to a name, since `let _ =` would let go of it at once.
#[test]
fn every_session_start_checks_the_clone_before_it_launches() {
    let starts = [
        (
            "pub(crate) async fn resume_session_for_store(",
            "let _checkout = ensure_resume_checkout(&app_handle, &store, &plan, &session_id).await?;",
            "launch_resume(",
        ),
        (
            "pub(crate) async fn resume_claimed_session_for_store(",
            "Ok(plan) => ensure_resume_checkout(&app_handle, &store, &plan, &session.id)",
            "launch_resume(",
        ),
        (
            "async fn start_requeued_session_for_branch(",
            "let _checkout = ensure_resume_checkout(&app_handle, &store, &plan, &session_id).await?;",
            "claim.claim(&store)",
        ),
        (
            "async fn start_queued_session_for_branch(",
            "let _checkout = crate::workspace_setup::verify::ensure_session_checkout(",
            "claim.claim(&store)",
        ),
    ];
    for (start, check, launch) in starts {
        assert_before(start, body_of(SESSION_COMMANDS, start), check, launch);
    }
    // The claimed resume carries its hold out of the `match` by name.
    let claimed = body_of(
        SESSION_COMMANDS,
        "pub(crate) async fn resume_claimed_session_for_store(",
    );
    assert_before(
        "resume_claimed_session_for_store",
        claimed,
        "Ok((plan, _checkout)) => plan,",
        "launch_resume(",
    );

    // A direct start, whose session row does not exist yet.
    let prepare = body_of(SESSION_COMMANDS, "async fn prepare_branch_session_start(");
    assert_calls(
        "prepare_branch_session_start",
        prepare,
        "crate::workspace_setup::verify::ensure_session_checkout(",
    );

    let resume_check = body_of(SESSION_COMMANDS, "async fn ensure_resume_checkout(");
    assert_calls(
        "ensure_resume_checkout",
        resume_check,
        "crate::workspace_setup::verify::ensure_session_checkout(app_handle, store, &branch, session_id)",
    );
    let check = body_of(VERIFY, "pub(crate) async fn ensure_session_checkout<");
    assert_calls(
        "ensure_session_checkout",
        check,
        "verify_session_checkout(SetupGuard::global(), store, &host, branch, session_id)",
    );
}

/// The pipelines that act on HEAD (commit, rebase, squash, pull) run git
/// in the clone without an agent, and get the same check before their first
/// step: before the row is claimed or inserted as running. A push does not
/// make it: it names its local branch in an explicit refspec, and used to
/// fail while a sibling branch's session had the clone. Neither does the
/// pull request pipeline, whose steps and prompts name the local branch and
/// whose first step stops it when the clone has no such branch.
#[test]
fn every_pipeline_checks_the_clone_before_its_git_steps() {
    let starts = [
        (
            PR_COMMIT,
            "pub(crate) async fn start_or_queue_commit_pipeline_for_branch(",
            "insert_running_commit_pipeline_session(",
        ),
        (
            PR_COMMIT,
            "pub(crate) async fn start_queued_commit_pipeline_for_branch(",
            "claim.claim(&store)",
        ),
        (
            PR_GIT_PIPELINE,
            "pub(crate) async fn start_queued_git_pipeline_for_branch(",
            "claim.claim(&store)",
        ),
    ];
    for (source, start, launch) in starts {
        let body = body_of(source, start);
        assert_before(
            start,
            body,
            "ensure_pipeline_checkout(&app_handle, &store, &ctx.branch, ",
            launch,
        );
        assert!(
            body.contains("let _checkout =") && !body.contains("let _ = ensure_pipeline_checkout"),
            "`{start}` must keep the clone until its pipeline is in flight"
        );
    }

    // A push is not held up by where the clone is, run now or from the
    // queue, and the pull request pipeline does not look.
    let push = body_of(
        PR_PUSH,
        "pub(crate) async fn start_or_queue_push_pipeline_for_branch(",
    );
    assert!(
        !push.contains("ensure_pipeline_checkout(&"),
        "a push must not check the clone's HEAD"
    );
    let queued = body_of(
        PR_GIT_PIPELINE,
        "pub(crate) async fn start_queued_git_pipeline_for_branch(",
    );
    assert_before(
        "start_queued_git_pipeline_for_branch",
        queued,
        "Some(PipelineKind::Push) => crate::workspace_setup::verify::CheckoutHold::none(),",
        "ensure_pipeline_checkout(&app_handle, &store, &ctx.branch, Some(&session.id)).await?",
    );
    let create_pr = body_of(
        PR_CREATE,
        "pub(crate) async fn start_create_pr_pipeline_for_branch(",
    );
    assert!(
        !PR_CREATE.contains("ensure_pipeline_checkout(")
            && !PR_CREATE.contains("ensure_session_checkout(")
            && !create_pr.contains("CheckoutHold"),
        "the pull request pipeline must not check or repair the clone's HEAD"
    );
    // What stands in for the check: the step that stops the pipeline when
    // the clone has no local branch comes before every other step.
    let steps = body_of(PR_CREATE, "pub(super) fn build_create_pr_pipeline_steps(");
    assert_before(
        "build_create_pr_pipeline_steps",
        steps,
        "command: check_local_branch_command(local_branch),\n            on_failure: FailureStrategy::Abort { marker: None },",
        "label: \"Fetch latest base\".to_string(),",
    );

    // The pull: checked in the task that runs it, before the pull itself.
    let pull = body_of(
        PR_PULL,
        "pub(crate) async fn pull_or_queue_branch_for_branch(",
    );
    assert_calls(
        "pull_or_queue_branch_for_branch",
        pull,
        "ensure_pipeline_checkout(",
    );
    let pull_with = body_of(PR_PULL, "pub(super) async fn pull_or_queue_branch_with<V>(");
    assert_before(
        "pull_or_queue_branch_with",
        pull_with,
        "verify(session_id.clone()).await",
        "spawn_blocking(pull)",
    );

    let check = body_of(PR_CONTEXT, "pub(crate) async fn ensure_pipeline_checkout(");
    assert_calls(
        "ensure_pipeline_checkout",
        check,
        "crate::workspace_setup::verify::ensure_session_checkout(",
    );
}

// ── What holds a branch's queue behind a start that could not check out ─────

/// A queued start that fails over where the clone is spends none of the
/// row's attempts: the drain breaks off when the start left the branch out
/// of `Running`, and when a sibling branch's session has the clone, before
/// it counts a failure. A held row is asked about again from the database,
/// before it is started.
#[test]
fn the_queue_is_held_not_drained_behind_a_start_that_could_not_check_out() {
    let drain = body_of(SESSION_COMMANDS, "pub(crate) async fn drain_branch_queue(");
    let still_queued = &drain[drain
        .rfind("FailedQueuedStart::StillQueued => {")
        .expect("the pre-claim failure arm")..];
    assert_before(
        "drain_branch_queue",
        still_queued,
        "if branch_session_start_waits_for_provisioning(&store, &branch_id)? {",
        "retry_or_fail_queued_start_before_claim(",
    );
    assert_before(
        "drain_branch_queue",
        still_queued,
        "hold_queue_behind_sibling_session(",
        "retry_or_fail_queued_start_before_claim(",
    );
    assert_before(
        "drain_branch_queue",
        drain,
        "kicker.start_attempts().is_held(&branch_id, &session.id)\n                && hold_queue_behind_sibling_session(",
        "start_queued_session_for_branch(",
    );
    // The hold tells the user why the row waits and has a `disconnected`
    // holder looked at, on the first look and on each recheck alike: both
    // drain call sites go through the one function.
    let hold = body_of(SESSION_COMMANDS, "fn hold_queue_behind_sibling_session<");
    for call in [
        "sibling_session_hold(store, kicker, branch, session_id)",
        "crate::workspace_setup::failures::notify_queue_held(app_handle, &branch.id, notice)",
        "crate::session_runner::reconcile::reconcile_session(holder.clone())",
    ] {
        assert_calls("hold_queue_behind_sibling_session", hold, call);
    }
    let look = body_of(SESSION_COMMANDS, "fn sibling_session_hold(");
    assert_calls(
        "sibling_session_hold",
        look,
        "crate::workspace_setup::clone::sibling_session_in_clone(store, branch)",
    );
}
