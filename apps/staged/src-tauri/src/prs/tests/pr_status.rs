//! Collecting a project's per-branch PR status refreshes.

use super::*;

fn branch_task(outcome: BranchRefresh) -> tokio::task::JoinHandle<Result<BranchRefresh, String>> {
    tokio::spawn(async move { Ok(outcome) })
}

#[tokio::test]
async fn collect_branch_refresh_results_tolerates_partial_task_failure() {
    let tasks = vec![
        branch_task(BranchRefresh::Refreshed),
        tokio::spawn(async {
            panic!("simulated branch task panic");
            #[allow(unreachable_code)]
            Ok::<BranchRefresh, String>(BranchRefresh::Refreshed)
        }),
        branch_task(BranchRefresh::Failed),
    ];

    let refreshed_count = collect_branch_refresh_results(tasks).await.unwrap();

    assert_eq!(refreshed_count, 1);
}

#[tokio::test]
async fn collect_branch_refresh_results_fails_when_all_tasks_fail() {
    let tasks = vec![
        tokio::spawn(async {
            Err::<BranchRefresh, String>("simulated semaphore failure".to_string())
        }),
        tokio::spawn(async {
            panic!("simulated branch task panic");
            #[allow(unreachable_code)]
            Ok::<BranchRefresh, String>(BranchRefresh::Refreshed)
        }),
    ];

    let err = collect_branch_refresh_results(tasks).await.unwrap_err();

    assert!(err.contains("all PR status refresh tasks failed"));
    assert!(err.contains("simulated semaphore failure"));
}

#[tokio::test]
async fn collect_branch_refresh_results_fails_when_all_branches_fail() {
    let tasks = vec![
        branch_task(BranchRefresh::Failed),
        branch_task(BranchRefresh::Failed),
    ];

    let err = collect_branch_refresh_results(tasks).await.unwrap_err();

    assert!(err.contains("all PR status refresh tasks failed"));
    assert!(err.contains("2 branch refreshes failed"));
}

#[tokio::test]
async fn collect_branch_refresh_results_does_not_fail_a_skipped_write() {
    // Every PR branch relinked or unlinked mid-poll: nothing refreshed,
    // but nothing failed either, so the poll scheduler records a success.
    let tasks = vec![
        branch_task(BranchRefresh::Skipped),
        branch_task(BranchRefresh::Skipped),
    ];
    assert_eq!(collect_branch_refresh_results(tasks).await.unwrap(), 0);

    // Alongside a refresh it isn't counted as one.
    let tasks = vec![
        branch_task(BranchRefresh::Refreshed),
        branch_task(BranchRefresh::Skipped),
    ];
    assert_eq!(collect_branch_refresh_results(tasks).await.unwrap(), 1);

    // Alongside a failure, only the failure is reported.
    let tasks = vec![
        branch_task(BranchRefresh::Skipped),
        branch_task(BranchRefresh::Failed),
    ];
    let err = collect_branch_refresh_results(tasks).await.unwrap_err();
    assert!(err.contains("1 branch refreshes failed"));
}
