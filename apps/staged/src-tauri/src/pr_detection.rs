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

use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::git;
use crate::store::Store;

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

/// A PR that matched on branch name but failed at least one check.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RejectedPrCandidate {
    pub pr_number: u64,
    pub pr_url: String,
    pub reasons: Vec<PrRejectionReason>,
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
        rejected.push(RejectedPrCandidate {
            pr_number: pr.number,
            pr_url: pr.url.clone(),
            reasons,
        });
    }

    if rejected.is_empty() {
        PrDetectionResult::NotFound
    } else {
        PrDetectionResult::Rejected {
            candidates: rejected,
        }
    }
}

/// Look up PRs on GitHub whose head is this branch and link the first one
/// that belongs to it.
///
/// Shared by the `detect_branch_pr` command and the web-mode `dispatch()`
/// arm. Errors if the branch already has a PR: the UI only offers detection
/// when it doesn't, so reaching here means the caller's view is stale.
pub(crate) async fn detect_branch_pr_impl(
    store: Arc<Store>,
    branch_id: String,
) -> Result<PrDetectionResult, String> {
    let branch = store
        .get_branch(&branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    if let Some(pr_number) = branch.pr_number {
        return Err(format!(
            "Branch is already linked to PR #{pr_number}; nothing to detect"
        ));
    }

    let project = store
        .get_project(&branch.project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Project not found: {}", branch.project_id))?;

    let (repo_slug, _) = crate::prs::resolve_branch_repo_and_subpath(&store, &project, &branch)?;

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

    let branch_name = branch.branch_name.clone();
    let lookup_slug = repo_slug.clone();
    let candidates = tauri::async_runtime::spawn_blocking(move || {
        git::list_prs_for_head_branch(&lookup_slug, &branch_name)
    })
    .await
    .map_err(|e| format!("detect_branch_pr task failed: {e}"))?
    .map_err(|e| e.to_string())?;

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
    fn head_repo_comparison_ignores_case() {
        let candidates = [pr(3, "OPEN", "main", Some("Acme/Widgets"))];

        assert!(matches!(
            classify_candidates(&candidates, &expectations()),
            PrDetectionResult::Found { pr_number: 3, .. }
        ));
    }

    #[test]
    fn reports_every_reason_for_each_rejected_candidate() {
        let candidates = [
            pr(4, "CLOSED", "develop", Some("acme/widgets")),
            pr(5, "OPEN", "main", Some("fork-owner/widgets")),
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
                        reasons: vec![
                            PrRejectionReason::NotOpen {
                                state: "closed".to_string()
                            },
                            PrRejectionReason::WrongBase {
                                expected: "main".to_string(),
                                actual: "develop".to_string(),
                            },
                        ],
                    },
                    RejectedPrCandidate {
                        pr_number: 5,
                        pr_url: "https://github.com/acme/widgets/pull/5".to_string(),
                        reasons: vec![PrRejectionReason::WrongHeadRepo {
                            expected: "acme/widgets".to_string(),
                            actual: Some("fork-owner/widgets".to_string()),
                        }],
                    },
                    RejectedPrCandidate {
                        pr_number: 6,
                        pr_url: "https://github.com/acme/widgets/pull/6".to_string(),
                        reasons: vec![PrRejectionReason::WrongHeadRepo {
                            expected: "acme/widgets".to_string(),
                            actual: None,
                        }],
                    },
                ],
            }
        );
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
            }],
        };
        assert_eq!(
            serde_json::to_value(&rejected).unwrap(),
            serde_json::json!({
                "kind": "rejected",
                "candidates": [{
                    "prNumber": 4,
                    "prUrl": "https://github.com/acme/widgets/pull/4",
                    "reasons": [
                        { "kind": "notOpen", "state": "merged" },
                        { "kind": "wrongBase", "expected": "main", "actual": "develop" },
                        { "kind": "wrongHeadRepo", "expected": "acme/widgets", "actual": null },
                    ],
                }],
            })
        );
    }
}
