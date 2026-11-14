//! Tests for the repo request resolver, against a fake GitHub.

use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// A GitHub that knows only what a test tells it, and records what it
/// was asked. Shared with the control request tests, which resolve through
/// it too.
#[derive(Default)]
pub(crate) struct FakeHost {
    /// Canonical names, found by any casing.
    repos: Vec<String>,
    repo_lookup_error: Option<String>,
    /// `(repo, subpath)` pairs that are directories.
    directories: HashSet<(String, String)>,
    pub(crate) module_counts: HashMap<String, u32>,
    module_count_error: Option<String>,
    pull_requests: HashMap<(String, u64), PullRequest>,
    /// Repositories whose pull request lookups fail, and with what.
    pull_request_errors: HashMap<String, String>,
    pub(crate) parents: HashMap<String, String>,
    parent_error: Option<String>,
    /// Open pull requests by `(repo, head branch name)`, in the order
    /// GitHub lists them. Forks that reuse the name are in the same list.
    pub(crate) branch_pull_requests: HashMap<(String, String), Vec<PullRequest>>,
    branch_pull_request_error: Option<String>,
    default_branches: HashMap<String, String>,
    /// `(repo, branch)` pairs that exist.
    branches: HashSet<(String, String)>,
    branch_lookup_error: Option<String>,
    calls: Mutex<Vec<String>>,
}

impl FakeHost {
    /// A host with `acme/widgets`, whose default branch is `main`.
    pub(crate) fn with_widgets() -> Self {
        let mut host = Self::default();
        host.add_repo("acme/widgets", "main");
        host
    }

    pub(crate) fn add_repo(&mut self, github_repo: &str, default_branch: &str) {
        self.repos.push(github_repo.to_string());
        self.default_branches
            .insert(github_repo.to_string(), default_branch.to_string());
        self.add_branch(github_repo, default_branch);
    }

    pub(crate) fn add_branch(&mut self, github_repo: &str, branch: &str) {
        self.branches
            .insert((github_repo.to_string(), branch.to_string()));
    }

    pub(crate) fn add_pull_request(&mut self, github_repo: &str, pr: PullRequest) {
        self.pull_requests
            .insert((github_repo.to_string(), pr.number), pr);
    }

    pub(crate) fn add_directory(&mut self, github_repo: &str, subpath: &str) {
        self.directories
            .insert((github_repo.to_string(), subpath.to_string()));
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    /// What was asked, sorted: lookups that run concurrently arrive in
    /// no fixed order.
    pub(crate) fn calls(&self) -> Vec<String> {
        let mut calls = self.calls.lock().unwrap().clone();
        calls.sort();
        calls
    }

    pub(crate) fn asked(&self, call: &str) -> bool {
        self.calls.lock().unwrap().iter().any(|c| c == call)
    }
}

impl RepoHost for FakeHost {
    fn canonical_repo(&self, owner: &str, repo: &str) -> Result<Option<String>, String> {
        let asked = format!("{owner}/{repo}");
        self.record(format!("repo {asked}"));
        if let Some(e) = &self.repo_lookup_error {
            return Err(e.clone());
        }
        Ok(self
            .repos
            .iter()
            .find(|known| known.eq_ignore_ascii_case(&asked))
            .cloned())
    }

    fn validate_subpath(&self, github_repo: &str, subpath: &str) -> Result<(), String> {
        self.record(format!("subpath {github_repo}:{subpath}"));
        if self
            .directories
            .contains(&(github_repo.to_string(), subpath.to_string()))
        {
            Ok(())
        } else {
            Err("Invalid path in repo".to_string())
        }
    }

    fn monorepo_module_count(&self, github_repo: &str) -> Result<u32, String> {
        self.record(format!("modules {github_repo}"));
        match &self.module_count_error {
            Some(e) => Err(e.clone()),
            None => Ok(self.module_counts.get(github_repo).copied().unwrap_or(0)),
        }
    }

    fn pull_request(&self, github_repo: &str, number: u64) -> Result<Option<PullRequest>, String> {
        self.record(format!("pr {github_repo}#{number}"));
        if let Some(e) = self.pull_request_errors.get(github_repo) {
            return Err(e.clone());
        }
        Ok(self
            .pull_requests
            .get(&(github_repo.to_string(), number))
            .cloned())
    }

    fn parent_repo(&self, github_repo: &str) -> Result<Option<String>, String> {
        self.record(format!("parent {github_repo}"));
        match &self.parent_error {
            Some(e) => Err(e.clone()),
            None => Ok(self.parents.get(github_repo).cloned()),
        }
    }

    fn open_pull_requests_for_branch(
        &self,
        github_repo: &str,
        branch_name: &str,
    ) -> Result<Vec<PullRequest>, String> {
        self.record(format!("branch-pr {github_repo}@{branch_name}"));
        match &self.branch_pull_request_error {
            Some(e) => Err(e.clone()),
            None => Ok(self
                .branch_pull_requests
                .get(&(github_repo.to_string(), branch_name.to_string()))
                .cloned()
                .unwrap_or_default()),
        }
    }

    fn default_branch(&self, github_repo: &str) -> Result<String, String> {
        self.record(format!("default-branch {github_repo}"));
        self.default_branches
            .get(github_repo)
            .cloned()
            .ok_or_else(|| "could not determine the default branch".to_string())
    }

    fn branch_exists(&self, github_repo: &str, branch_name: &str) -> Result<bool, String> {
        self.record(format!("branch {github_repo}@{branch_name}"));
        match &self.branch_lookup_error {
            Some(e) => Err(e.clone()),
            None => Ok(self
                .branches
                .contains(&(github_repo.to_string(), branch_name.to_string()))),
        }
    }
}

/// Pull request `number`, from `head_ref` in `head_repo` into `develop`.
pub(crate) fn pull_request(number: u64, head_ref: &str, head_repo: Option<&str>) -> PullRequest {
    PullRequest {
        number,
        title: format!("Change {number}"),
        body: format!("Why change {number} is needed."),
        author: "octocat".to_string(),
        base_ref: "develop".to_string(),
        head_ref: head_ref.to_string(),
        head_repo: head_repo.map(str::to_string),
        draft: false,
        updated_at: "2026-11-13T00:00:00Z".to_string(),
    }
}

fn request(github_repo: &str) -> RepoSpecRequest {
    RepoSpecRequest {
        github_repo: github_repo.to_string(),
        ..Default::default()
    }
}

fn resolved_pr(number: u64) -> ResolvedPullRequest {
    ResolvedPullRequest {
        number,
        title: format!("Change {number}"),
        body: format!("Why change {number} is needed."),
    }
}

// --- Name ---------------------------------------------------------

#[test]
fn a_bare_repo_resolves_to_its_default_branch_and_no_branch_name() {
    let host = FakeHost::with_widgets();

    let resolved = resolve_repo_spec(&host, request("acme/widgets")).unwrap();

    assert_eq!(
        resolved,
        ResolvedRepoSpec {
            github_repo: "acme/widgets".to_string(),
            subpath: None,
            branch_name: None,
            base: Some("main".to_string()),
            head_repo: None,
            pull_request: None,
        }
    );
    assert_eq!(
        host.calls(),
        [
            "default-branch acme/widgets",
            "modules acme/widgets",
            "repo acme/widgets",
        ]
    );
}

#[test]
fn the_repo_is_returned_as_github_spells_it() {
    let host = FakeHost::with_widgets();

    let resolved = resolve_repo_spec(&host, request("  Acme/Widgets ")).unwrap();

    assert_eq!(resolved.github_repo, "acme/widgets");
    // Everything after the name lookup asks about the canonical name.
    assert!(host.asked("repo Acme/Widgets"));
    assert!(host.asked("default-branch acme/widgets"));
    assert!(host.asked("modules acme/widgets"));
}

#[test]
fn a_repo_github_does_not_know_is_refused() {
    let host = FakeHost::with_widgets();

    let err = resolve_repo_spec(&host, request("acme/gizmos")).unwrap_err();

    assert_eq!(
        err,
        "Repository 'acme/gizmos' was not found on GitHub, or you don't have access to it."
    );
    assert_eq!(host.calls(), ["repo acme/gizmos"]);
}

#[test]
fn a_failed_repo_lookup_is_reported_with_its_cause() {
    let host = FakeHost {
        repo_lookup_error: Some("gh: not logged in".to_string()),
        ..FakeHost::with_widgets()
    };

    let err = resolve_repo_spec(&host, request("acme/widgets")).unwrap_err();

    assert_eq!(
        err,
        "Could not look up repository 'acme/widgets': gh: not logged in"
    );
}

#[test]
fn a_name_that_is_not_owner_slash_repo_is_refused_before_any_lookup() {
    let host = FakeHost::with_widgets();

    for name in [
        "",
        "widgets",
        "acme/",
        "/widgets",
        "acme/widgets/extra",
        "acme/..",
        "acme/wid gets",
        "https://github.com/acme/widgets",
        "acme/widgets?per_page=1",
    ] {
        let err = resolve_repo_spec(&host, request(name)).unwrap_err();
        assert_eq!(
            err,
            format!("Repository must be given as owner/repo (got '{name}')."),
        );
    }
    assert!(host.calls().is_empty());
}

// --- Subpath ------------------------------------------------------

#[test]
fn a_subpath_is_normalised_then_checked_in_the_repo() {
    let mut host = FakeHost::with_widgets();
    host.add_directory("acme/widgets", "services/auth");

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            subpath: Some(" /services/auth// ".to_string()),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(resolved.subpath.as_deref(), Some("services/auth"));
    assert!(host.asked("subpath acme/widgets:services/auth"));
}

#[test]
fn a_blank_subpath_is_the_repo_root_and_is_not_checked() {
    let host = FakeHost::with_widgets();

    for blank in ["", "  ", "/", " // "] {
        let resolved = resolve_repo_spec(
            &host,
            RepoSpecRequest {
                subpath: Some(blank.to_string()),
                ..request("acme/widgets")
            },
        )
        .unwrap();
        assert_eq!(resolved.subpath, None, "subpath {blank:?}");
    }
    assert!(!host.calls().iter().any(|call| call.starts_with("subpath")));
}

#[test]
fn a_subpath_that_is_not_a_directory_in_the_repo_is_refused() {
    let host = FakeHost::with_widgets();

    let err = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            subpath: Some("services/missing".to_string()),
            ..request("acme/widgets")
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        "Subpath 'services/missing' can't be used in 'acme/widgets': Invalid path in repo"
    );
}

// --- Monorepo -----------------------------------------------------

fn monorepo_host(module_count: u32) -> FakeHost {
    let mut host = FakeHost::with_widgets();
    host.module_counts
        .insert("acme/widgets".to_string(), module_count);
    host.add_directory("acme/widgets", "services/auth");
    host
}

#[test]
fn the_root_of_a_monorepo_is_refused() {
    let host = monorepo_host(MONOREPO_MODULE_THRESHOLD);

    let err = resolve_repo_spec(&host, request("acme/widgets")).unwrap_err();

    assert_eq!(
        err,
        "'acme/widgets' appears to be a monorepo (20 modules in MODULES.yaml). \
         Give the subpath of the service or package to work in, \
         or pass --allow-monorepo-root to use the repository root."
    );
}

#[test]
fn a_repo_just_under_the_module_threshold_is_not_a_monorepo() {
    let host = monorepo_host(MONOREPO_MODULE_THRESHOLD - 1);

    let resolved = resolve_repo_spec(&host, request("acme/widgets")).unwrap();

    assert_eq!(resolved.subpath, None);
}

#[test]
fn the_override_admits_the_root_of_a_monorepo_without_counting_modules() {
    let host = monorepo_host(400);

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            allow_monorepo_root: true,
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(resolved.subpath, None);
    assert!(!host.asked("modules acme/widgets"));
}

#[test]
fn a_subpath_in_a_monorepo_is_admitted_without_counting_modules() {
    let host = monorepo_host(400);

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            subpath: Some("services/auth".to_string()),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(resolved.subpath.as_deref(), Some("services/auth"));
    assert!(!host.asked("modules acme/widgets"));
}

#[test]
fn a_failed_module_count_does_not_block_the_repo() {
    let host = FakeHost {
        module_count_error: Some("HTTP 502".to_string()),
        ..monorepo_host(400)
    };

    let resolved = resolve_repo_spec(&host, request("acme/widgets")).unwrap();

    assert_eq!(resolved.github_repo, "acme/widgets");
}

// --- Pull request -------------------------------------------------

#[test]
fn a_pull_request_brings_its_branch_base_and_description() {
    let mut host = FakeHost::with_widgets();
    host.add_pull_request(
        "acme/widgets",
        pull_request(12, "fix-login", Some("acme/widgets")),
    );

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            pr_number: Some(12),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(
        resolved,
        ResolvedRepoSpec {
            github_repo: "acme/widgets".to_string(),
            subpath: None,
            branch_name: Some("fix-login".to_string()),
            base: Some("develop".to_string()),
            // The head is in the repository itself.
            head_repo: None,
            pull_request: Some(resolved_pr(12)),
        }
    );
    // The pull request settles the branch and the base, so neither the
    // branch's pull request nor the default branch is asked for.
    assert_eq!(
        host.calls(),
        [
            "modules acme/widgets",
            "pr acme/widgets#12",
            "repo acme/widgets",
        ]
    );
}

#[test]
fn a_pull_request_from_a_fork_records_where_its_head_lives() {
    let mut host = FakeHost::with_widgets();
    host.add_pull_request(
        "acme/widgets",
        pull_request(12, "fix-login", Some("forker/widgets")),
    );

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            pr_number: Some(12),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(resolved.github_repo, "acme/widgets");
    assert_eq!(resolved.head_repo.as_deref(), Some("forker/widgets"));
}

#[test]
fn a_head_repo_that_differs_only_in_case_is_the_repo_itself() {
    let mut host = FakeHost::with_widgets();
    host.add_pull_request(
        "acme/widgets",
        pull_request(12, "fix-login", Some("Acme/Widgets")),
    );

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            pr_number: Some(12),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(resolved.head_repo, None);
}

/// `forker/widgets` is a fork of `acme/widgets`, and pull request 12 is
/// open against the parent.
fn fork_host(pr_head_repo: Option<&str>) -> FakeHost {
    let mut host = FakeHost::with_widgets();
    host.add_repo("forker/widgets", "trunk");
    host.parents
        .insert("forker/widgets".to_string(), "acme/widgets".to_string());
    host.add_pull_request("acme/widgets", pull_request(12, "fix-login", pr_head_repo));
    host
}

fn fork_pr_request() -> RepoSpecRequest {
    RepoSpecRequest {
        pr_number: Some(12),
        subpath: Some("services/auth".to_string()),
        ..request("forker/widgets")
    }
}

#[test]
fn a_pull_request_missing_from_a_fork_is_found_on_its_parent() {
    let mut host = fork_host(Some("forker/widgets"));
    // Only the parent has the directory: the subpath must be checked
    // against the repository that will be cloned.
    host.add_directory("acme/widgets", "services/auth");

    let resolved = resolve_repo_spec(&host, fork_pr_request()).unwrap();

    assert_eq!(
        resolved,
        ResolvedRepoSpec {
            github_repo: "acme/widgets".to_string(),
            subpath: Some("services/auth".to_string()),
            branch_name: Some("fix-login".to_string()),
            base: Some("develop".to_string()),
            head_repo: Some("forker/widgets".to_string()),
            pull_request: Some(resolved_pr(12)),
        }
    );
    assert_eq!(
        host.calls(),
        [
            "parent forker/widgets",
            "pr acme/widgets#12",
            "pr forker/widgets#12",
            "repo forker/widgets",
            "subpath acme/widgets:services/auth",
        ]
    );
}

#[test]
fn a_parent_pull_request_with_an_unknown_head_repo_keeps_the_named_fork() {
    let mut host = fork_host(None);
    host.add_directory("acme/widgets", "services/auth");

    let resolved = resolve_repo_spec(&host, fork_pr_request()).unwrap();

    assert_eq!(resolved.github_repo, "acme/widgets");
    assert_eq!(resolved.head_repo.as_deref(), Some("forker/widgets"));
}

#[test]
fn a_parent_pull_request_headed_in_the_parent_has_no_head_repo() {
    let mut host = fork_host(Some("acme/widgets"));
    host.add_directory("acme/widgets", "services/auth");

    let resolved = resolve_repo_spec(&host, fork_pr_request()).unwrap();

    assert_eq!(resolved.github_repo, "acme/widgets");
    assert_eq!(resolved.head_repo, None);
}

#[test]
fn a_pull_request_that_cannot_be_found_is_refused() {
    let host = FakeHost::with_widgets();

    let err = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            pr_number: Some(99),
            ..request("acme/widgets")
        },
    )
    .unwrap_err();

    assert_eq!(err, "Pull request #99 was not found in 'acme/widgets'.");
    // Nothing else is asked once the pull request is known to be missing.
    assert_eq!(
        host.calls(),
        [
            "parent acme/widgets",
            "pr acme/widgets#99",
            "repo acme/widgets",
        ]
    );
}

#[test]
fn a_pull_request_missing_from_a_fork_and_its_parent_is_refused() {
    let host = fork_host(Some("forker/widgets"));

    let err = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            pr_number: Some(99),
            ..request("forker/widgets")
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        "Pull request #99 was not found in 'forker/widgets' or in 'acme/widgets', \
         the repository it was forked from."
    );
}

/// The fault the review found: any failure of the first lookup used to send
/// the request to the fork's parent. A network blip on a fork would then
/// swap the repository for the parent and adopt the parent's pull request of
/// the same number, which is other code than the caller asked for.
#[test]
fn a_failed_pull_request_lookup_on_a_fork_does_not_fall_back_to_its_parent() {
    let mut host = fork_host(Some("forker/widgets"));
    host.add_directory("acme/widgets", "services/auth");
    // The fork has its own pull request 12. The parent's 12 is another
    // change entirely.
    host.add_pull_request(
        "forker/widgets",
        pull_request(12, "experiment", Some("forker/widgets")),
    );
    host.pull_request_errors.insert(
        "forker/widgets".to_string(),
        "error connecting to api.github.com".to_string(),
    );

    let err = resolve_repo_spec(&host, fork_pr_request()).unwrap_err();

    assert_eq!(
        err,
        "Could not look up pull request #12 in 'forker/widgets': \
         error connecting to api.github.com"
    );
    assert_eq!(
        host.calls(),
        ["pr forker/widgets#12", "repo forker/widgets"],
        "the parent is asked nothing"
    );
}

/// The fallback itself is unchanged: the fork answered that it has no such
/// pull request, and the parent's lookup failing is reported as that.
#[test]
fn a_failed_pull_request_lookup_on_the_parent_is_reported_with_its_cause() {
    let mut host = fork_host(Some("forker/widgets"));
    host.pull_request_errors
        .insert("acme/widgets".to_string(), "HTTP 502".to_string());

    let err = resolve_repo_spec(&host, fork_pr_request()).unwrap_err();

    assert_eq!(
        err,
        "Pull request #12 was not found in 'forker/widgets', and it could not be looked up \
         in 'acme/widgets', the repository it was forked from: HTTP 502"
    );
}

#[test]
fn a_failed_pull_request_lookup_on_a_repo_that_is_no_fork_is_not_a_missing_pull_request() {
    let mut host = FakeHost::with_widgets();
    host.pull_request_errors.insert(
        "acme/widgets".to_string(),
        "API rate limit exceeded".to_string(),
    );

    let err = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            pr_number: Some(7),
            ..request("acme/widgets")
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        "Could not look up pull request #7 in 'acme/widgets': API rate limit exceeded"
    );
    assert!(!host.asked("parent acme/widgets"));
}

/// The pull request is known to be missing from the named repository. Whether
/// that repository is a fork could not be learned, and the refusal says so.
#[test]
fn a_failed_parent_lookup_leaves_the_pull_request_not_found() {
    let host = FakeHost {
        parent_error: Some("HTTP 502".to_string()),
        ..fork_host(Some("forker/widgets"))
    };

    let err = resolve_repo_spec(&host, fork_pr_request()).unwrap_err();

    assert_eq!(
        err,
        "Pull request #12 was not found in 'forker/widgets', and Staged could not check \
         whether that repository is a fork: HTTP 502"
    );
}

#[test]
fn a_branch_given_with_a_pull_request_must_be_the_pull_requests_branch() {
    let mut host = FakeHost::with_widgets();
    host.add_pull_request(
        "acme/widgets",
        pull_request(12, "fix-login", Some("acme/widgets")),
    );
    let with_branch = |branch: &str| RepoSpecRequest {
        pr_number: Some(12),
        branch_name: Some(branch.to_string()),
        ..request("acme/widgets")
    };

    // The desktop forms send the head branch along with the number.
    let resolved = resolve_repo_spec(&host, with_branch("fix-login")).unwrap();
    assert_eq!(resolved.branch_name.as_deref(), Some("fix-login"));
    assert_eq!(resolved.pull_request, Some(resolved_pr(12)));

    let err = resolve_repo_spec(&host, with_branch("something-else")).unwrap_err();
    assert_eq!(
        err,
        "Pull request #12's branch is 'fix-login', not 'something-else'. \
         Give the pull request or the branch, not both."
    );
}

// --- Branch -------------------------------------------------------

fn branch_request(branch: &str) -> RepoSpecRequest {
    RepoSpecRequest {
        branch_name: Some(branch.to_string()),
        ..request("acme/widgets")
    }
}

#[test]
fn a_branch_with_an_open_pull_request_adopts_it() {
    let mut host = FakeHost::with_widgets();
    host.branch_pull_requests.insert(
        ("acme/widgets".to_string(), "fix-login".to_string()),
        vec![pull_request(12, "fix-login", Some("acme/widgets"))],
    );

    let resolved = resolve_repo_spec(&host, branch_request(" fix-login ")).unwrap();

    assert_eq!(
        resolved,
        ResolvedRepoSpec {
            github_repo: "acme/widgets".to_string(),
            subpath: None,
            branch_name: Some("fix-login".to_string()),
            // The pull request's base, not the default branch.
            base: Some("develop".to_string()),
            head_repo: None,
            pull_request: Some(resolved_pr(12)),
        }
    );
    assert!(host.asked("branch-pr acme/widgets@fix-login"));
}

/// `gh pr list --head <name>` matches the name alone. On a public repository
/// anyone can open a pull request from a fork's branch called `test`, and
/// the newest of those is listed first. Found live: `--branch test` on
/// `octocat/Hello-World` checked out a stranger's fork instead of the
/// repository's own `test` branch, which has an open pull request of its own.
#[test]
fn a_branch_adopts_its_own_pull_request_and_not_a_forks_of_the_same_name() {
    let mut host = FakeHost::with_widgets();
    host.branch_pull_requests.insert(
        ("acme/widgets".to_string(), "test".to_string()),
        vec![
            pull_request(4518, "test", Some("stranger/widgets")),
            pull_request(1051, "test", None),
            // GitHub's spelling of the repository, in another case.
            pull_request(504, "test", Some("Acme/Widgets")),
        ],
    );

    let resolved = resolve_repo_spec(&host, branch_request("test")).unwrap();

    assert_eq!(resolved.branch_name.as_deref(), Some("test"));
    assert_eq!(resolved.pull_request, Some(resolved_pr(504)));
    assert_eq!(resolved.base.as_deref(), Some("develop"));
    // The head is the repository itself, so no head repo is recorded.
    assert_eq!(resolved.head_repo, None);
}

#[test]
fn a_branch_whose_name_only_forks_have_pull_requests_for_adopts_none() {
    let mut host = FakeHost::with_widgets();
    host.branch_pull_requests.insert(
        ("acme/widgets".to_string(), "test".to_string()),
        vec![
            pull_request(4518, "test", Some("stranger/widgets")),
            // A deleted fork: GitHub no longer knows the head repository.
            pull_request(1051, "test", None),
        ],
    );

    let resolved = resolve_repo_spec(&host, branch_request("test")).unwrap();

    // The repository's own branch, from the default branch like any other
    // branch with no pull request: no fork, no number, no description.
    assert_eq!(
        resolved,
        ResolvedRepoSpec {
            github_repo: "acme/widgets".to_string(),
            subpath: None,
            branch_name: Some("test".to_string()),
            base: Some("main".to_string()),
            head_repo: None,
            pull_request: None,
        }
    );
}

#[test]
fn a_branch_without_a_pull_request_starts_from_the_default_branch() {
    let host = FakeHost::with_widgets();

    let resolved = resolve_repo_spec(&host, branch_request("new-idea")).unwrap();

    assert_eq!(resolved.branch_name.as_deref(), Some("new-idea"));
    assert_eq!(resolved.base.as_deref(), Some("main"));
    assert_eq!(resolved.pull_request, None);
    assert_eq!(resolved.head_repo, None);
}

#[test]
fn a_failed_pull_request_lookup_for_a_branch_is_not_an_error() {
    let host = FakeHost {
        branch_pull_request_error: Some("HTTP 502".to_string()),
        ..FakeHost::with_widgets()
    };

    let resolved = resolve_repo_spec(&host, branch_request("fix-login")).unwrap();

    assert_eq!(resolved.branch_name.as_deref(), Some("fix-login"));
    assert_eq!(resolved.base.as_deref(), Some("main"));
    assert_eq!(resolved.pull_request, None);
}

#[test]
fn a_blank_branch_is_left_to_be_inferred_and_is_not_looked_up() {
    let host = FakeHost::with_widgets();

    let resolved = resolve_repo_spec(&host, branch_request("   ")).unwrap();

    assert_eq!(resolved.branch_name, None);
    assert!(!host
        .calls()
        .iter()
        .any(|call| call.starts_with("branch-pr")));
}

// --- Base ---------------------------------------------------------

#[test]
fn an_explicit_base_wins_and_nothing_is_detected() {
    let mut host = FakeHost::with_widgets();
    host.add_branch("acme/widgets", "release/2");

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            base: Some(" release/2 ".to_string()),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(resolved.base.as_deref(), Some("release/2"));
    assert!(!host.asked("default-branch acme/widgets"));
    assert!(host.asked("branch acme/widgets@release/2"));
}

/// The forms offer only the repository's real branches as a base. A typed
/// one that does not exist used to write every row and then fail in the
/// background worktree setup.
#[test]
fn an_explicit_base_the_repo_does_not_have_is_refused() {
    let host = FakeHost::with_widgets();

    let err = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            base: Some("mian".to_string()),
            ..request("acme/widgets")
        },
    )
    .unwrap_err();

    assert_eq!(err, "Base branch 'mian' was not found in 'acme/widgets'.");
}

/// Unlike the best-effort lookups, the base check must succeed: the caller
/// chose where the branch starts, and a check that could not be made has
/// not shown that the base exists.
#[test]
fn a_base_check_that_fails_refuses_the_request() {
    let host = FakeHost {
        branch_lookup_error: Some("HTTP 502".to_string()),
        ..FakeHost::with_widgets()
    };

    let err = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            base: Some("main".to_string()),
            ..request("acme/widgets")
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        "Could not check that base branch 'main' exists in 'acme/widgets': HTTP 502"
    );
}

/// The shared bodies read `origin/<branch>` as `<branch>` on origin, so the
/// check does too, and the base is passed on as it was typed.
#[test]
fn an_origin_prefixed_base_is_checked_without_the_prefix() {
    let host = FakeHost::with_widgets();

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            base: Some("origin/main".to_string()),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(resolved.base.as_deref(), Some("origin/main"));
    assert!(host.asked("branch acme/widgets@main"));
}

/// After a fork fallback the base is checked in the parent, which is the
/// repository that will be cloned.
#[test]
fn a_base_is_checked_in_the_repo_that_will_be_cloned() {
    let mut host = fork_host(Some("forker/widgets"));
    host.add_directory("acme/widgets", "services/auth");

    // `trunk` is the fork's default branch, and the parent has no such
    // branch.
    let err = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            base: Some("trunk".to_string()),
            ..fork_pr_request()
        },
    )
    .unwrap_err();

    assert_eq!(err, "Base branch 'trunk' was not found in 'acme/widgets'.");
}

/// A base nobody chose is not checked: the pull request's and the detected
/// default are GitHub's own answers.
#[test]
fn only_an_explicit_base_is_checked() {
    let mut host = FakeHost::with_widgets();
    host.add_pull_request(
        "acme/widgets",
        pull_request(12, "fix-login", Some("acme/widgets")),
    );

    resolve_repo_spec(&host, request("acme/widgets")).unwrap();
    resolve_repo_spec(
        &host,
        RepoSpecRequest {
            pr_number: Some(12),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert!(
        !host.calls().iter().any(|call| call.starts_with("branch ")),
        "{:?}",
        host.calls()
    );
}

// --- Branch names -------------------------------------------------

/// A name git refuses used to write every row and fail in the background.
/// It is refused before GitHub is asked anything, the leading `-` that git
/// would read as an option included.
#[test]
fn a_branch_name_git_would_refuse_is_refused_before_any_lookup() {
    for (name, rule) in [
        ("-x", "it starts with '-'"),
        ("--upload-pack=evil", "it starts with '-'"),
        (
            "fix login",
            "it contains a space, a control character or one of ~ ^ : ? * [ \\",
        ),
        ("fix..login", "it contains '..'"),
        ("fix.lock", "a part of it ends with '.lock'"),
        ("HEAD", "'HEAD' is reserved"),
    ] {
        let host = FakeHost::with_widgets();

        let err = resolve_repo_spec(&host, branch_request(name)).unwrap_err();

        assert_eq!(err, format!("'{name}' is not a valid branch name: {rule}."));
        assert!(host.calls().is_empty(), "{name}: {:?}", host.calls());
    }
}

#[test]
fn a_base_git_would_refuse_is_refused_before_any_lookup() {
    for base in ["-main", "origin/-main", "ma in", "origin/"] {
        let host = FakeHost::with_widgets();

        let err = resolve_repo_spec(
            &host,
            RepoSpecRequest {
                base: Some(base.to_string()),
                ..request("acme/widgets")
            },
        )
        .unwrap_err();

        assert!(
            err.starts_with(&format!("'{base}' is not a valid branch name: ")),
            "{base}: {err}"
        );
        assert!(host.calls().is_empty(), "{base}: {:?}", host.calls());
    }
}

#[test]
fn an_ordinary_branch_name_passes_the_name_check() {
    let host = FakeHost::with_widgets();

    for name in ["fix-login", "user/fix_login.2", "caf\u{e9}"] {
        let resolved = resolve_repo_spec(&host, branch_request(name)).unwrap();
        assert_eq!(resolved.branch_name.as_deref(), Some(name));
    }
}

#[test]
fn an_explicit_base_wins_over_a_pull_requests_base() {
    let mut host = FakeHost::with_widgets();
    host.add_pull_request(
        "acme/widgets",
        pull_request(12, "fix-login", Some("acme/widgets")),
    );
    host.branch_pull_requests.insert(
        ("acme/widgets".to_string(), "fix-login".to_string()),
        vec![pull_request(12, "fix-login", Some("acme/widgets"))],
    );
    host.add_branch("acme/widgets", "release/2");
    let base = Some("release/2".to_string());

    let by_number = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            pr_number: Some(12),
            base: base.clone(),
            ..request("acme/widgets")
        },
    )
    .unwrap();
    let by_branch = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            base,
            ..branch_request("fix-login")
        },
    )
    .unwrap();

    assert_eq!(by_number.base.as_deref(), Some("release/2"));
    assert_eq!(by_branch.base.as_deref(), Some("release/2"));
}

#[test]
fn a_blank_base_is_no_base() {
    let host = FakeHost::with_widgets();

    let resolved = resolve_repo_spec(
        &host,
        RepoSpecRequest {
            base: Some("  ".to_string()),
            ..request("acme/widgets")
        },
    )
    .unwrap();

    assert_eq!(resolved.base.as_deref(), Some("main"));
}

#[test]
fn a_default_branch_that_cannot_be_detected_leaves_the_base_open() {
    let mut host = FakeHost::with_widgets();
    host.default_branches.clear();

    let resolved = resolve_repo_spec(&host, request("acme/widgets")).unwrap();

    assert_eq!(resolved.base, None);
}

// --- Helpers ------------------------------------------------------

#[test]
fn the_monorepo_threshold_is_twenty_modules() {
    assert!(!is_monorepo(0));
    assert!(!is_monorepo(19));
    assert!(is_monorepo(20));
    assert!(is_monorepo(21));
}

#[test]
fn a_gh_failure_reads_without_the_git_label() {
    assert_eq!(
        gh_error(GitError::CommandFailed("HTTP 404: Not Found".to_string())),
        "HTTP 404: Not Found"
    );
    assert_eq!(
        gh_error(GitError::GitNotFound),
        "git not found - is git installed?"
    );
}
