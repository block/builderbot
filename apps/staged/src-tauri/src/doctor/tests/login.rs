use super::*;

/// The tail numbers every line from zero and keeps only the newest
/// `LOGIN_OUTPUT_TAIL_LINES`, and its snapshot states both — a client merges
/// live lines against `next_seq`, so a snapshot whose `output` did not sit
/// exactly below it would show lines twice or lose them.
#[test]
fn login_output_tail_numbers_lines_and_keeps_the_newest() {
    let mut tail = LoginOutputTail::default();
    assert_eq!(tail.push("Opening browser to sign in…"), 0);
    assert_eq!(tail.push("If the browser didn't open, visit: https://x"), 1);
    assert_eq!(
        tail.status("run-1"),
        DoctorLoginStatus {
            running: true,
            run_id: Some("run-1".to_string()),
            output: vec![
                "Opening browser to sign in…".to_string(),
                "If the browser didn't open, visit: https://x".to_string(),
            ],
            next_seq: 2,
        }
    );

    for i in 2..(LOGIN_OUTPUT_TAIL_LINES as u64 + 10) {
        assert_eq!(tail.push(&format!("line {i}")), i);
    }
    let status = tail.status("run-1");
    assert_eq!(status.output.len(), LOGIN_OUTPUT_TAIL_LINES);
    assert_eq!(status.next_seq, LOGIN_OUTPUT_TAIL_LINES as u64 + 10);
    // `output[i]` carries seq `next_seq - output.len() + i`.
    let first_seq = status.next_seq - status.output.len() as u64;
    assert_eq!(status.output.first().unwrap(), &format!("line {first_seq}"));
    assert_eq!(
        status.output.last().unwrap(),
        &format!("line {}", status.next_seq - 1)
    );
}

/// A cancelled run is reported as cancelled and not as a failure; a run
/// that failed on its own keeps its error; and a cancel that only landed
/// after the fix had finished changes nothing about its result.
#[test]
fn login_done_event_tells_a_cancellation_from_a_failure() {
    let cancelled_by_runner =
        Err("Fix cancelled before finishing: claude-agent-acp --cli auth login".to_string());

    let done = login_done_event(
        "ai-agent-claude".into(),
        "run-1".into(),
        &cancelled_by_runner,
        true,
        3,
    );
    assert_eq!(
        done,
        DoctorLoginOutput {
            check_id: "ai-agent-claude".into(),
            run_id: "run-1".into(),
            line: None,
            seq: 3,
            done: true,
            error: None,
            cancelled: true,
        }
    );

    let failed = Err("Fix timed out after 10m without finishing: …".to_string());
    let done = login_done_event("ai-agent-claude".into(), "run-1".into(), &failed, false, 3);
    assert!(!done.cancelled);
    assert_eq!(
        done.error.as_deref(),
        Some("Fix timed out after 10m without finishing: …")
    );

    let done = login_done_event("ai-agent-claude".into(), "run-1".into(), &Ok(()), true, 0);
    assert!(done.done && !done.cancelled && done.error.is_none());
}

/// The start answer is what a client keys its whole record on, so its wire
/// shape is pinned: one tag field for the outcome and the run id beside it
/// in camelCase, for both variants.
#[test]
fn login_start_serializes_as_a_tagged_object_with_the_run_id() {
    assert_eq!(
        serde_json::to_value(LoginStart::Started {
            run_id: "run-1".into()
        })
        .unwrap(),
        serde_json::json!({ "outcome": "started", "runId": "run-1" })
    );
    assert_eq!(
        serde_json::to_value(LoginStart::AlreadyRunning {
            run_id: "run-1".into()
        })
        .unwrap(),
        serde_json::json!({ "outcome": "alreadyRunning", "runId": "run-1" })
    );
    // The status and the events spell it the same way.
    let status = serde_json::to_value(LoginOutputTail::default().status("run-1")).unwrap();
    assert_eq!(status["runId"], "run-1");
    assert_eq!(status["nextSeq"], 0);
    let idle = serde_json::to_value(DoctorLoginStatus::idle()).unwrap();
    assert_eq!(idle["runId"], serde_json::Value::Null);
    let done = serde_json::to_value(login_done_event(
        "ai-agent-claude".into(),
        "run-1".into(),
        &Ok(()),
        false,
        0,
    ))
    .unwrap();
    assert_eq!(done["runId"], "run-1");
}

/// Build an entry the way `claim_login` does, without touching the global
/// map — for tests of the lookup logic alone.
fn active_login(run_id: &str) -> ActiveLogin {
    let (writer, _stdin) = FixStdin::pipe();
    let (cancel, _cancellation) = FixCancellation::token();
    ActiveLogin {
        run_id: run_id.to_string(),
        writer,
        cancel,
        output: Arc::new(Mutex::new(LoginOutputTail::default())),
    }
}

/// A command names a run, and reaches it only while that run holds the
/// check's slot. Nothing running is one kind of stale, a *different* run
/// holding the slot is another, and the refusal for a code says which —
/// and, for the second, that the code did not go to the newer login.
#[test]
fn find_login_run_tells_a_finished_run_from_the_one_that_replaced_it() {
    let mut logins = HashMap::new();
    assert_eq!(
        find_login_run(&logins, "ai-agent-claude", "run-1").err(),
        Some(StaleLoginRun::Ended)
    );

    logins.insert("ai-agent-claude".to_string(), active_login("run-2"));
    let found = find_login_run(&logins, "ai-agent-claude", "run-2").expect("the live run");
    assert_eq!(found.run_id, "run-2");
    assert_eq!(
        find_login_run(&logins, "ai-agent-claude", "run-1").err(),
        Some(StaleLoginRun::Superseded)
    );
    // The slot is per check: another check's run id never matches.
    assert_eq!(
        find_login_run(&logins, "ai-agent-codex", "run-2").err(),
        Some(StaleLoginRun::Ended)
    );

    let ended = StaleLoginRun::Ended.code_refusal("ai-agent-claude");
    assert!(ended.contains("has ended"), "{ended}");
    assert!(
        ended.contains("no login is running for ai-agent-claude"),
        "{ended}"
    );
    let superseded = StaleLoginRun::Superseded.code_refusal("ai-agent-claude");
    assert!(superseded.contains("has ended"), "{superseded}");
    assert!(
        superseded.contains("a newer login is running for ai-agent-claude"),
        "{superseded}"
    );
    assert!(superseded.contains("was not delivered"), "{superseded}");
}

/// Every claim mints its own id, and ids are not recycled: two runs of one
/// check, and two checks' runs, never share one.
#[test]
fn login_run_ids_are_unique() {
    let ids: std::collections::HashSet<String> = (0..64).map(|_| new_login_run_id()).collect();
    assert_eq!(ids.len(), 64);
    assert!(ids.iter().all(|id| !id.is_empty()));
}

/// A slot releases only the run it was claimed for. Its `Drop` cannot
/// normally run while a later run holds the entry — the later claim needs
/// the entry gone first — but the release is keyed to the run id so that
/// stays true by construction rather than by call order.
///
/// Uses `ai-agent-cursor`: `ACTIVE_LOGINS` is process-global and the other
/// login tests in this module claim other checks.
#[test]
fn login_slot_releases_only_its_own_run() {
    let check_id = "ai-agent-cursor";
    let LoginClaim::Claimed(login) = claim_login(check_id).expect("cursor has a login fix") else {
        panic!("the slot is free");
    };
    let live_run_id = login.run_id.clone();

    // A stale slot for a run that no longer holds the entry.
    drop(LoginSlot {
        check_id: check_id.to_string(),
        run_id: "an-earlier-run".to_string(),
    });
    assert_eq!(
        active_logins()
            .lock()
            .unwrap()
            .get(check_id)
            .map(|login| login.run_id.clone()),
        Some(live_run_id),
        "a stale slot's drop leaves the live run's entry alone"
    );

    drop(login);
    assert!(
        !active_logins().lock().unwrap().contains_key(check_id),
        "the run's own slot releases it"
    );
}

/// The slot lifecycle end to end: a claim holds the check under a run id, a
/// second claim reports that id rather than an error, the status names it,
/// the cancel and code commands reach the run only under it, dropping the
/// claim releases the slot, and the next claim gets a new id and a token the
/// earlier cancel does not refuse.
///
/// Uses `ai-agent-codex` because `ACTIVE_LOGINS` is process-global and the
/// other login tests in this module claim other checks.
#[tokio::test]
async fn login_slot_is_released_on_drop_and_the_cancel_reaches_the_run() {
    let check_id = "ai-agent-codex";
    let LoginClaim::Claimed(login) = claim_login(check_id).expect("codex has a login fix") else {
        panic!("the first claim takes the slot");
    };
    let run_id = login.run_id.clone();
    assert_eq!(
        login.slot.run_id, run_id,
        "the slot releases the run it was claimed for"
    );
    match claim_login(check_id).expect("still a valid check") {
        LoginClaim::AlreadyRunning { run_id: running } => assert_eq!(
            running, run_id,
            "a second claim names the run that holds the slot"
        ),
        LoginClaim::Claimed(_) => panic!("the slot is held"),
    }

    assert_eq!(
        doctor_login_status(check_id.into()).await,
        DoctorLoginStatus {
            running: true,
            run_id: Some(run_id.clone()),
            output: Vec::new(),
            next_seq: 0,
        }
    );
    // What the run's `on_line` records is what a late client is shown.
    login
        .output
        .lock()
        .unwrap()
        .push("Opening browser to sign in…");
    assert_eq!(
        doctor_login_status(check_id.into()).await.output,
        vec!["Opening browser to sign in…".to_string()]
    );

    // A code for a run that isn't the one holding the slot is refused —
    // and never reaches this run's pipe. The right id queues it (the fix
    // has not spawned here, so `send_line` buffers it for the replay).
    let refused = send_doctor_login_code(check_id.into(), "an-earlier-run".into(), "abc".into())
        .await
        .expect_err("a code for another run is refused");
    assert!(
        refused.contains("a newer login is running for ai-agent-codex"),
        "{refused}"
    );
    send_doctor_login_code(check_id.into(), run_id.clone(), "abc".into())
        .await
        .expect("a code for the live run reaches its pipe");

    assert!(!login.cancel.is_cancelled());
    assert!(
        !cancel_doctor_login(check_id.into(), "an-earlier-run".into()).await,
        "a cancel for another run is not this run's"
    );
    assert!(!login.cancel.is_cancelled());
    assert!(cancel_doctor_login(check_id.into(), run_id.clone()).await);
    assert!(
        login.cancel.is_cancelled(),
        "the command reaches the token the run was given"
    );
    assert!(
        cancel_doctor_login(check_id.into(), run_id.clone()).await,
        "repeating it is a no-op that still reports the running login"
    );

    drop(login);
    assert_eq!(
        doctor_login_status(check_id.into()).await,
        DoctorLoginStatus::idle()
    );
    assert!(
        !cancel_doctor_login(check_id.into(), run_id.clone()).await,
        "nothing running: nothing to cancel"
    );
    let refused = send_doctor_login_code(check_id.into(), run_id.clone(), "abc".into())
        .await
        .expect_err("a code for a run that has ended is refused");
    assert!(
        refused.contains("no login is running for ai-agent-codex"),
        "{refused}"
    );

    let LoginClaim::Claimed(again) = claim_login(check_id).expect("still a valid check") else {
        panic!("a released slot can be claimed again");
    };
    assert_ne!(
        again.run_id, run_id,
        "each run gets its own id, so the earlier run's events and commands can't be taken for this one's"
    );
    assert!(
        !again.cancel.is_cancelled(),
        "each run gets a fresh token, so the earlier cancel can't refuse the retry"
    );
}

#[test]
fn claim_login_refuses_a_check_with_no_login_fix() {
    let err = claim_login("ai-agent-goose").expect_err("goose has no auth command");
    assert!(
        err.contains("No login fix available for ai-agent-goose"),
        "{err}"
    );
    assert!(!active_logins()
        .lock()
        .unwrap()
        .contains_key("ai-agent-goose"));
}
