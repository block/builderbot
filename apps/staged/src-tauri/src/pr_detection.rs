//! Manual "Detect existing PR" lookup.
//!
//! Staged only learns a branch's PR number when it drove the PR creation or
//! the branch was created from a PR. A PR opened any other way (a session
//! running `gh pr create`, "Compare & pull request" in the browser, a
//! terminal) leaves `branches.pr_number` null. This module backs the
//! user-triggered lookup that closes that gap: it lists PRs on GitHub whose
//! head is the branch name, adopts the first one that belongs to this branch,
//! and otherwise explains why each candidate was passed over so the user can
//! tell "no PR" apart from "a PR exists but targets the wrong base".
//!
//! A base mismatch is the one check the user may legitimately want to
//! override (the PR was retargeted on GitHub, or the stored base is stale
//! after a stack restructure), so candidates rejected on that alone are
//! flagged `linkable` and `link_branch_pr` adopts one on request. It re-runs
//! the same checks so a closed PR or a fork's PR can't be linked by hand.

use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::git;
use crate::store::{Branch, Store};

/// Why a PR with the right head branch name was not adopted.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PrRejectionReason {
    /// Only an open PR is linked; `state` is the lower-cased GitHub state
    /// (`closed` or `merged`).
    NotOpen { state: String },
    /// The PR targets a different base than the branch is based on.
    WrongBase { expected: String, actual: String },
    /// The PR's head lives in a different repository than this branch pushes
    /// to — typically a fork with the same branch name. `actual` is `None`
    /// when the head repository has been deleted.
    WrongHeadRepo {
        expected: String,
        actual: Option<String>,
    },
}

impl PrRejectionReason {
    /// Whether the user may link the PR despite this reason. Only a base
    /// mismatch is soft: a closed or merged PR is not this branch's PR any
    /// more, and a PR whose head is in another repository never was.
    fn is_overridable(&self) -> bool {
        matches!(self, PrRejectionReason::WrongBase { .. })
    }

    /// Sentence for error messages; the frontend has its own copy for the
    /// dialog list.
    fn describe(&self) -> String {
        match self {
            PrRejectionReason::NotOpen { state } => format!("it is {state}"),
            PrRejectionReason::WrongBase { expected, actual } => {
                format!("it targets {actual}, not {expected}")
            }
            PrRejectionReason::WrongHeadRepo {
                expected,
                actual: Some(actual),
            } => format!("its head branch lives in {actual}, not {expected}"),
            PrRejectionReason::WrongHeadRepo {
                expected,
                actual: None,
            } => format!("its head repository no longer exists (this branch pushes to {expected})"),
        }
    }
}

/// A PR that matched on branch name but failed at least one check.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RejectedPrCandidate {
    pub pr_number: u64,
    pub pr_url: String,
    /// GitHub login of the PR author, so the user can tell a teammate's
    /// same-named PR apart from their own.
    pub author: String,
    pub draft: bool,
    pub reasons: Vec<PrRejectionReason>,
    /// True when every reason is overridable, so the UI can offer to link
    /// the PR anyway via `link_branch_pr`.
    pub linkable: bool,
}

impl RejectedPrCandidate {
    fn new(pr: &git::HeadBranchPr, reasons: Vec<PrRejectionReason>) -> Self {
        let linkable = !reasons.is_empty() && reasons.iter().all(PrRejectionReason::is_overridable);
        RejectedPrCandidate {
            pr_number: pr.number,
            pr_url: pr.url.clone(),
            author: pr.author.clone(),
            draft: pr.draft,
            reasons,
            linkable,
        }
    }
}

/// A PR linked by explicit user choice via `link_branch_pr`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinkedBranchPr {
    pub pr_number: u64,
    pub pr_url: String,
}

/// Outcome of a detection pass, as returned to the frontend.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PrDetectionResult {
    /// A matching open PR was found and persisted as the branch's PR.
    #[serde(rename_all = "camelCase")]
    Found { pr_number: u64, pr_url: String },
    /// No PR on GitHub has this branch as its head.
    NotFound,
    /// PRs with this head branch exist, but none belongs to this branch.
    Rejected {
        candidates: Vec<RejectedPrCandidate>,
    },
}

/// What a PR must satisfy to be adopted as the branch's PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrExpectations {
    /// Bare base branch name (no `origin/` prefix).
    pub base_ref: String,
    /// `owner/repo` the branch is pushed to: the fork's `head_repo` when the
    /// project repo is a fork setup, otherwise the repo itself.
    pub head_repo: String,
}

fn rejection_reasons(pr: &git::HeadBranchPr, expected: &PrExpectations) -> Vec<PrRejectionReason> {
    let mut reasons = Vec::new();

    if !pr.state.eq_ignore_ascii_case("open") {
        reasons.push(PrRejectionReason::NotOpen {
            state: pr.state.to_ascii_lowercase(),
        });
    }

    if pr.base_ref != expected.base_ref {
        reasons.push(PrRejectionReason::WrongBase {
            expected: expected.base_ref.clone(),
            actual: pr.base_ref.clone(),
        });
    }

    // GitHub treats owner/repo names case-insensitively, so a slug typed
    // with different casing is still the same repository.
    let same_head_repo = pr
        .head_repo
        .as_deref()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(&expected.head_repo));
    if !same_head_repo {
        reasons.push(PrRejectionReason::WrongHeadRepo {
            expected: expected.head_repo.clone(),
            actual: pr.head_repo.clone(),
        });
    }

    reasons
}

/// Pick the first candidate that satisfies every expectation; when none does,
/// report every candidate with its reasons (in the order GitHub returned
/// them) so the UI can explain what it saw.
pub(crate) fn classify_candidates(
    candidates: &[git::HeadBranchPr],
    expected: &PrExpectations,
) -> PrDetectionResult {
    let mut rejected = Vec::new();

    for pr in candidates {
        let reasons = rejection_reasons(pr, expected);
        if reasons.is_empty() {
            return PrDetectionResult::Found {
                pr_number: pr.number,
                pr_url: pr.url.clone(),
            };
        }
        rejected.push(RejectedPrCandidate::new(pr, reasons));
    }

    if rejected.is_empty() {
        PrDetectionResult::NotFound
    } else {
        PrDetectionResult::Rejected {
            candidates: rejected,
        }
    }
}

/// Find `pr_number` among the PRs whose head is `branch_name` and check it
/// may be linked by hand: every failed check must be overridable.
///
/// The candidate list is re-fetched by the caller rather than trusted from
/// the earlier detection pass, so a PR that was closed in between is caught.
pub(crate) fn pick_candidate_to_link<'a>(
    candidates: &'a [git::HeadBranchPr],
    pr_number: u64,
    expected: &PrExpectations,
    branch_name: &str,
) -> Result<&'a git::HeadBranchPr, String> {
    let pr = candidates
        .iter()
        .find(|pr| pr.number == pr_number)
        .ok_or_else(|| format!("PR #{pr_number} no longer has {branch_name} as its head branch"))?;

    let blocking: Vec<String> = rejection_reasons(pr, expected)
        .iter()
        .filter(|reason| !reason.is_overridable())
        .map(PrRejectionReason::describe)
        .collect();

    if blocking.is_empty() {
        Ok(pr)
    } else {
        Err(format!(
            "PR #{pr_number} cannot be linked to this branch: {}",
            blocking.join("; ")
        ))
    }
}

/// Resolve the repo a branch's PRs are listed in and what one must satisfy
/// to be adopted.
fn branch_pr_expectations(
    store: &Arc<Store>,
    branch: &Branch,
) -> Result<(String, PrExpectations), String> {
    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let (repo_slug, _) = crate::prs::resolve_branch_repo_and_subpath(store, &project, branch)?;

    // For fork setups the branch is pushed to `head_repo`, and that is where
    // GitHub reports the PR's head as living.
    let head_repo = match &branch.project_repo_id {
        Some(repo_id) => store
            .get_project_repo(repo_id)
            .map_err(|e| e.to_string())?
            .and_then(|repo| repo.head_repo),
        None => None,
    }
    .unwrap_or_else(|| repo_slug.clone());

    let expected = PrExpectations {
        base_ref: git::branch_name_without_origin(&branch.base_branch).to_string(),
        head_repo,
    };

    Ok((repo_slug, expected))
}

/// Load an unlinked branch. Errors if it already has a PR: the UI only
/// offers detection and linking when it doesn't, so reaching here means the
/// caller's view is stale.
fn load_unlinked_branch(store: &Store, branch_id: &str, action: &str) -> Result<Branch, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if let Some(pr_number) = branch.pr_number {
        return Err(format!(
            "Branch is already linked to PR #{pr_number}; nothing to {action}"
        ));
    }

    Ok(branch)
}

async fn list_head_branch_prs(
    repo_slug: String,
    branch_name: String,
    state: git::PrListState,
    task: &str,
) -> Result<Vec<git::HeadBranchPr>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::list_prs_for_head_branch(&repo_slug, &branch_name, state)
    })
    .await
    .map_err(|e| format!("{task} task failed: {e}"))?
    .map_err(|e| e.to_string())
}

/// Open PRs first, then the newest closed/merged ones.
///
/// `gh pr list --head` matches the branch name across every fork, newest
/// first, so with `--state=all` a branch's own open PR can fall outside the
/// limit when the name has a long history (fork contributors opening PRs
/// from `main` or `patch-1`). The open list is fetched on its own so what
/// can be adopted or linked never depends on that history; the all-states
/// list is only consulted when `need_history(open)` says the open PRs alone
/// don't settle the outcome, and contributes just the non-open ones.
async fn list_head_branch_prs_with_history(
    repo_slug: &str,
    branch_name: &str,
    task: &str,
    need_history: impl FnOnce(&[git::HeadBranchPr]) -> bool,
) -> Result<Vec<git::HeadBranchPr>, String> {
    let open = list_head_branch_prs(
        repo_slug.to_string(),
        branch_name.to_string(),
        git::PrListState::Open,
        task,
    )
    .await?;

    if !need_history(&open) {
        return Ok(open);
    }

    let all = list_head_branch_prs(
        repo_slug.to_string(),
        branch_name.to_string(),
        git::PrListState::All,
        task,
    )
    .await?;

    Ok(merge_open_with_history(open, all))
}

/// Append the non-open PRs from `all` to `open`, keeping each list's order.
/// Open PRs are taken from `open` alone: it is the complete list, while
/// `all` is a capped sample that may have missed some of them.
fn merge_open_with_history(
    mut open: Vec<git::HeadBranchPr>,
    all: Vec<git::HeadBranchPr>,
) -> Vec<git::HeadBranchPr> {
    open.extend(
        all.into_iter()
            .filter(|pr| !pr.state.eq_ignore_ascii_case("open")),
    );
    open
}

/// Look up PRs on GitHub whose head is this branch and link the first one
/// that belongs to it.
///
/// Shared by the `detect_branch_pr` command and the web-mode `dispatch()`
/// arm.
pub(crate) async fn detect_branch_pr_impl(
    store: Arc<Store>,
    branch_id: String,
) -> Result<PrDetectionResult, String> {
    let branch = load_unlinked_branch(&store, &branch_id, "detect")?;
    let (repo_slug, expected) = branch_pr_expectations(&store, &branch)?;

    // Closed and merged PRs are never adopted, so they are only fetched when
    // no open PR satisfies the checks and the user needs to be told why.
    let candidates = list_head_branch_prs_with_history(
        &repo_slug,
        &branch.branch_name,
        "detect_branch_pr",
        |open| {
            !matches!(
                classify_candidates(open, &expected),
                PrDetectionResult::Found { .. }
            )
        },
    )
    .await?;

    let result = classify_candidates(&candidates, &expected);

    if let PrDetectionResult::Found { pr_number, .. } = &result {
        store
            .update_branch_pr_number(&branch_id, Some(*pr_number))
            .map_err(|e| e.to_string())?;
        log::info!(
            "detect_branch_pr: linked PR #{} in {} to branch_id={}",
            pr_number,
            repo_slug,
            branch_id
        );
    } else {
        log::info!(
            "detect_branch_pr: no matching PR for branch_id={} in {} ({} candidate(s))",
            branch_id,
            repo_slug,
            candidates.len()
        );
    }

    Ok(result)
}

/// User-triggered lookup of a PR Staged didn't create. Persists the PR number
/// on a match; the frontend renders the outcome.
#[tauri::command(rename_all = "camelCase")]
pub async fn detect_branch_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
) -> Result<PrDetectionResult, String> {
    let store = crate::get_store(&store)?;
    detect_branch_pr_impl(store, branch_id).await
}

/// Link a PR that `detect_branch_pr` reported as `linkable`, at the user's
/// request. The PR is looked up again and the checks re-run; only
/// overridable failures (a base mismatch) are waved through.
///
/// Shared by the `link_branch_pr` command and the web-mode `dispatch()` arm.
pub(crate) async fn link_branch_pr_impl(
    store: Arc<Store>,
    branch_id: String,
    pr_number: u64,
) -> Result<LinkedBranchPr, String> {
    let branch = load_unlinked_branch(&store, &branch_id, "link")?;
    let (repo_slug, expected) = branch_pr_expectations(&store, &branch)?;

    // Only an open PR can be linked, but a PR closed since detection should
    // be refused as "closed" rather than "no longer has this head", so the
    // history is consulted when the number isn't among the open PRs.
    let candidates = list_head_branch_prs_with_history(
        &repo_slug,
        &branch.branch_name,
        "link_branch_pr",
        |open| !open.iter().any(|pr| pr.number == pr_number),
    )
    .await?;

    let pr = pick_candidate_to_link(&candidates, pr_number, &expected, &branch.branch_name)?;

    store
        .update_branch_pr_number(&branch_id, Some(pr.number))
        .map_err(|e| e.to_string())?;
    log::info!(
        "link_branch_pr: linked PR #{} in {} to branch_id={} (PR base {}, branch base {})",
        pr.number,
        repo_slug,
        branch_id,
        pr.base_ref,
        expected.base_ref
    );

    Ok(LinkedBranchPr {
        pr_number: pr.number,
        pr_url: pr.url.clone(),
    })
}

/// User-chosen "Link anyway" for a PR detection passed over on its base
/// branch alone. Persists the PR number; the frontend renders the outcome.
#[tauri::command(rename_all = "camelCase")]
pub async fn link_branch_pr(
    store: tauri::State<'_, Mutex<Option<Arc<Store>>>>,
    branch_id: String,
    pr_number: u64,
) -> Result<LinkedBranchPr, String> {
    let store = crate::get_store(&store)?;
    link_branch_pr_impl(store, branch_id, pr_number).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(number: u64, state: &str, base_ref: &str, head_repo: Option<&str>) -> git::HeadBranchPr {
        git::HeadBranchPr {
            number,
            url: format!("https://github.com/acme/widgets/pull/{number}"),
            state: state.to_string(),
            base_ref: base_ref.to_string(),
            head_repo: head_repo.map(str::to_string),
            draft: false,
            author: "octocat".to_string(),
        }
    }

    fn expectations() -> PrExpectations {
        PrExpectations {
            base_ref: "main".to_string(),
            head_repo: "acme/widgets".to_string(),
        }
    }

    #[test]
    fn empty_candidate_list_is_not_found() {
        assert_eq!(
            classify_candidates(&[], &expectations()),
            PrDetectionResult::NotFound
        );
    }

    #[test]
    fn adopts_first_open_pr_matching_base_and_head_repo() {
        let candidates = [
            pr(7, "MERGED", "main", Some("acme/widgets")),
            pr(9, "OPEN", "main", Some("acme/widgets")),
            pr(11, "OPEN", "main", Some("acme/widgets")),
        ];

        assert_eq!(
            classify_candidates(&candidates, &expectations()),
            PrDetectionResult::Found {
                pr_number: 9,
                pr_url: "https://github.com/acme/widgets/pull/9".to_string(),
            }
        );
    }

    #[test]
    fn history_merge_keeps_open_prs_from_the_open_list_only() {
        // The all-states list is a newest-first sample: it repeats one open
        // PR, missed another (#2), and carries the closed/merged history.
        let open = vec![
            pr(40, "OPEN", "develop", Some("acme/widgets")),
            pr(2, "OPEN", "main", Some("fork/widgets")),
        ];
        let all = vec![
            pr(41, "CLOSED", "main", Some("acme/widgets")),
            pr(40, "OPEN", "develop", Some("acme/widgets")),
            pr(39, "MERGED", "main", Some("acme/widgets")),
        ];

        let merged: Vec<u64> = merge_open_with_history(open, all)
            .iter()
            .map(|pr| pr.number)
            .collect();

        assert_eq!(merged, vec![40, 2, 41, 39]);
    }

    #[test]
    fn head_repo_comparison_ignores_case() {
        let candidates = [pr(3, "OPEN", "main", Some("Acme/Widgets"))];

        assert!(matches!(
            classify_candidates(&candidates, &expectations()),
            PrDetectionResult::Found { pr_number: 3, .. }
        ));
    }

    #[test]
    fn reports_every_reason_for_each_rejected_candidate() {
        // The author and draft flag are passed through untouched so the
        // dialog can show whose PR each candidate is.
        let fork_pr = git::HeadBranchPr {
            author: "fork-owner".to_string(),
            draft: true,
            ..pr(5, "OPEN", "main", Some("fork-owner/widgets"))
        };
        let candidates = [
            pr(4, "CLOSED", "develop", Some("acme/widgets")),
            fork_pr,
            pr(6, "OPEN", "main", None),
        ];

        let result = classify_candidates(&candidates, &expectations());

        assert_eq!(
            result,
            PrDetectionResult::Rejected {
                candidates: vec![
                    RejectedPrCandidate {
                        pr_number: 4,
                        pr_url: "https://github.com/acme/widgets/pull/4".to_string(),
                        author: "octocat".to_string(),
                        draft: false,
                        reasons: vec![
                            PrRejectionReason::NotOpen {
                                state: "closed".to_string()
                            },
                            PrRejectionReason::WrongBase {
                                expected: "main".to_string(),
                                actual: "develop".to_string(),
                            },
                        ],
                        linkable: false,
                    },
                    RejectedPrCandidate {
                        pr_number: 5,
                        pr_url: "https://github.com/acme/widgets/pull/5".to_string(),
                        author: "fork-owner".to_string(),
                        draft: true,
                        reasons: vec![PrRejectionReason::WrongHeadRepo {
                            expected: "acme/widgets".to_string(),
                            actual: Some("fork-owner/widgets".to_string()),
                        }],
                        linkable: false,
                    },
                    RejectedPrCandidate {
                        pr_number: 6,
                        pr_url: "https://github.com/acme/widgets/pull/6".to_string(),
                        author: "octocat".to_string(),
                        draft: false,
                        reasons: vec![PrRejectionReason::WrongHeadRepo {
                            expected: "acme/widgets".to_string(),
                            actual: None,
                        }],
                        linkable: false,
                    },
                ],
            }
        );
    }

    #[test]
    fn only_a_base_mismatch_leaves_a_candidate_linkable() {
        let candidates = [
            pr(4, "OPEN", "develop", Some("acme/widgets")),
            pr(5, "CLOSED", "develop", Some("acme/widgets")),
            pr(6, "OPEN", "develop", Some("fork-owner/widgets")),
        ];

        let PrDetectionResult::Rejected { candidates } =
            classify_candidates(&candidates, &expectations())
        else {
            panic!("expected every candidate to be rejected");
        };

        let linkable: Vec<(u64, bool)> = candidates
            .iter()
            .map(|c| (c.pr_number, c.linkable))
            .collect();
        assert_eq!(linkable, vec![(4, true), (5, false), (6, false)]);
    }

    #[test]
    fn link_accepts_an_open_pr_that_only_differs_on_base() {
        let candidates = [
            pr(4, "OPEN", "develop", Some("acme/widgets")),
            pr(9, "OPEN", "main", Some("acme/widgets")),
        ];

        let picked = pick_candidate_to_link(&candidates, 4, &expectations(), "feature").unwrap();
        assert_eq!(picked.number, 4);
    }

    #[test]
    fn link_also_accepts_a_pr_that_detection_would_have_adopted() {
        // Linking by hand after a stale detection pass should not fail just
        // because GitHub now matches; the PR is simply linked.
        let candidates = [pr(9, "OPEN", "main", Some("acme/widgets"))];

        assert!(pick_candidate_to_link(&candidates, 9, &expectations(), "feature").is_ok());
    }

    #[test]
    fn link_refuses_closed_and_foreign_head_prs() {
        let candidates = [
            pr(4, "MERGED", "develop", Some("acme/widgets")),
            pr(5, "OPEN", "develop", Some("fork-owner/widgets")),
            pr(6, "OPEN", "main", None),
        ];

        let err = pick_candidate_to_link(&candidates, 4, &expectations(), "feature").unwrap_err();
        assert_eq!(err, "PR #4 cannot be linked to this branch: it is merged");

        let err = pick_candidate_to_link(&candidates, 5, &expectations(), "feature").unwrap_err();
        assert_eq!(
            err,
            "PR #5 cannot be linked to this branch: its head branch lives in fork-owner/widgets, not acme/widgets"
        );

        let err = pick_candidate_to_link(&candidates, 6, &expectations(), "feature").unwrap_err();
        assert_eq!(
            err,
            "PR #6 cannot be linked to this branch: its head repository no longer exists (this branch pushes to acme/widgets)"
        );
    }

    #[test]
    fn link_refuses_a_pr_whose_head_is_no_longer_this_branch() {
        let candidates = [pr(4, "OPEN", "develop", Some("acme/widgets"))];

        let err = pick_candidate_to_link(&candidates, 42, &expectations(), "feature").unwrap_err();
        assert_eq!(err, "PR #42 no longer has feature as its head branch");
    }

    #[test]
    fn fork_setup_expects_the_fork_as_head_repo() {
        let expected = PrExpectations {
            base_ref: "main".to_string(),
            head_repo: "fork-owner/widgets".to_string(),
        };
        let candidates = [
            pr(1, "OPEN", "main", Some("acme/widgets")),
            pr(2, "OPEN", "main", Some("fork-owner/widgets")),
        ];

        assert!(matches!(
            classify_candidates(&candidates, &expected),
            PrDetectionResult::Found { pr_number: 2, .. }
        ));
    }

    /// The frontend switches on `kind` and reads camelCase fields; lock the
    /// wire shape so a serde attribute change can't silently break it.
    #[test]
    fn serializes_with_kind_tags_and_camel_case_fields() {
        let found = PrDetectionResult::Found {
            pr_number: 9,
            pr_url: "https://github.com/acme/widgets/pull/9".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&found).unwrap(),
            serde_json::json!({
                "kind": "found",
                "prNumber": 9,
                "prUrl": "https://github.com/acme/widgets/pull/9",
            })
        );

        assert_eq!(
            serde_json::to_value(PrDetectionResult::NotFound).unwrap(),
            serde_json::json!({ "kind": "notFound" })
        );

        let rejected = PrDetectionResult::Rejected {
            candidates: vec![RejectedPrCandidate {
                pr_number: 4,
                pr_url: "https://github.com/acme/widgets/pull/4".to_string(),
                author: "octocat".to_string(),
                draft: true,
                reasons: vec![
                    PrRejectionReason::NotOpen {
                        state: "merged".to_string(),
                    },
                    PrRejectionReason::WrongBase {
                        expected: "main".to_string(),
                        actual: "develop".to_string(),
                    },
                    PrRejectionReason::WrongHeadRepo {
                        expected: "acme/widgets".to_string(),
                        actual: None,
                    },
                ],
                linkable: false,
            }],
        };
        assert_eq!(
            serde_json::to_value(&rejected).unwrap(),
            serde_json::json!({
                "kind": "rejected",
                "candidates": [{
                    "prNumber": 4,
                    "prUrl": "https://github.com/acme/widgets/pull/4",
                    "author": "octocat",
                    "draft": true,
                    "reasons": [
                        { "kind": "notOpen", "state": "merged" },
                        { "kind": "wrongBase", "expected": "main", "actual": "develop" },
                        { "kind": "wrongHeadRepo", "expected": "acme/widgets", "actual": null },
                    ],
                    "linkable": false,
                }],
            })
        );

        let linked = LinkedBranchPr {
            pr_number: 4,
            pr_url: "https://github.com/acme/widgets/pull/4".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&linked).unwrap(),
            serde_json::json!({
                "prNumber": 4,
                "prUrl": "https://github.com/acme/widgets/pull/4",
            })
        );
    }
}
