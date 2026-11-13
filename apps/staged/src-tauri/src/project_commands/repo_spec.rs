//! Resolving what a caller says about a repository — a name, perhaps a
//! subpath, a branch or a pull request — into the arguments the shared
//! create-project and add-repo bodies take.
//!
//! The desktop forms do this work as the user fills them in: the repo comes
//! from a search result, the subpath is validated on submit, a monorepo is
//! badged, and `BranchPicker` fetches the pull request and its base. A caller
//! with no form (the control surface `stagedctl` talks to) sends what was
//! typed, so the same rules run here instead.

use crate::git::{self, GitError, PullRequest};

/// A repository with at least this many modules in its `MODULES.yaml` is
/// treated as a monorepo: one where a caller is expected to say which
/// service or package to work in.
const MONOREPO_MODULE_THRESHOLD: u32 = 20;

/// Whether a `MODULES.yaml` module count (`git::check_monorepo_modules`)
/// marks a monorepo.
pub(crate) fn is_monorepo(module_count: u32) -> bool {
    module_count >= MONOREPO_MODULE_THRESHOLD
}

/// The GitHub lookups [`resolve_repo_spec`] makes, behind a trait so its
/// rules can be tested with no network. Every method blocks, and several may
/// be called at once from different threads.
pub(crate) trait RepoHost: Sync {
    /// The repository's name as GitHub spells it, or `None` when it does not
    /// exist or the user cannot see it.
    fn canonical_repo(&self, owner: &str, repo: &str) -> Result<Option<String>, String>;

    /// `Ok` when `subpath` is a directory in the repository.
    fn validate_subpath(&self, github_repo: &str, subpath: &str) -> Result<(), String>;

    /// How many modules the repository's `MODULES.yaml` lists; 0 without one.
    fn monorepo_module_count(&self, github_repo: &str) -> Result<u32, String>;

    /// The pull request with this number, whatever state it is in.
    fn pull_request(&self, github_repo: &str, number: u64) -> Result<PullRequest, String>;

    /// The repository this one was forked from, or `None` when it is not a
    /// fork.
    fn parent_repo(&self, github_repo: &str) -> Result<Option<String>, String>;

    /// Every open pull request whose head branch has this name. GitHub
    /// matches the name alone, so the list includes pull requests opened
    /// from forks that reuse it.
    fn open_pull_requests_for_branch(
        &self,
        github_repo: &str,
        branch_name: &str,
    ) -> Result<Vec<PullRequest>, String>;

    /// The repository's default branch, without an `origin/` prefix.
    fn default_branch(&self, github_repo: &str) -> Result<String, String>;
}

/// [`RepoHost`] answered by the `gh` CLI.
pub(crate) struct GitHubCli;

impl RepoHost for GitHubCli {
    fn canonical_repo(&self, owner: &str, repo: &str) -> Result<Option<String>, String> {
        git::fetch_github_repo(owner, repo)
            .map(|found| found.map(|repo| repo.name_with_owner))
            .map_err(gh_error)
    }

    fn validate_subpath(&self, github_repo: &str, subpath: &str) -> Result<(), String> {
        git::validate_subpath_in_repo(github_repo, subpath).map_err(gh_error)
    }

    fn monorepo_module_count(&self, github_repo: &str) -> Result<u32, String> {
        git::check_monorepo_modules(github_repo).map_err(gh_error)
    }

    fn pull_request(&self, github_repo: &str, number: u64) -> Result<PullRequest, String> {
        git::github::get_pr_for_repo(github_repo, number).map_err(gh_error)
    }

    fn parent_repo(&self, github_repo: &str) -> Result<Option<String>, String> {
        git::github::get_parent_repo(github_repo).map_err(gh_error)
    }

    fn open_pull_requests_for_branch(
        &self,
        github_repo: &str,
        branch_name: &str,
    ) -> Result<Vec<PullRequest>, String> {
        git::github::list_open_prs_for_branch_for_repo(github_repo, branch_name).map_err(gh_error)
    }

    fn default_branch(&self, github_repo: &str) -> Result<String, String> {
        git::detect_default_branch_for_repo(github_repo).map_err(gh_error)
    }
}

/// A `gh` failure as the user should read it: without the "git command
/// failed" label [`GitError::CommandFailed`] puts in front of `gh`'s own
/// words. `SubpathInput` strips the same label for the desktop form.
fn gh_error(err: GitError) -> String {
    match err {
        GitError::CommandFailed(message) => message,
        other => other.to_string(),
    }
}

/// What a caller says about the repository to create a project with or to
/// add to one. Not persisted: it is an argument list, like
/// [`super::CreateProjectRequest`].
#[derive(Debug, Clone, Default)]
pub(crate) struct RepoSpecRequest {
    /// `owner/repo`, in whatever case the caller typed it.
    pub github_repo: String,
    /// A directory in the repository to work in. Surrounding slashes and
    /// whitespace are dropped; blank means the repository root.
    pub subpath: Option<String>,
    /// An existing remote branch or a new name. Blank leaves the name to be
    /// inferred from the project's.
    pub branch_name: Option<String>,
    /// A pull request to check out. It brings its own branch, so a
    /// `branch_name` given with it must be that branch.
    pub pr_number: Option<u64>,
    /// The branch a new branch starts from, when the caller chooses one.
    pub base: Option<String>,
    /// Accept the root of a monorepo, which is refused otherwise.
    pub allow_monorepo_root: bool,
}

/// A [`RepoSpecRequest`] checked against GitHub and filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedRepoSpec {
    /// The repository to clone, as GitHub spells it. For a pull request
    /// asked for on a fork but opened against the fork's parent, this is the
    /// parent: `refs/pull/<n>/head` only exists there.
    pub github_repo: String,
    /// The subpath, normalised, and known to be a directory in the
    /// repository.
    pub subpath: Option<String>,
    /// The pull request's head branch, else the caller's branch name. `None`
    /// leaves the name to be inferred from the project's.
    pub branch_name: Option<String>,
    /// The branch to start from: the caller's, else the pull request's base,
    /// else the repository's default branch. `None` when none was given and
    /// the default could not be detected, which leaves the shared bodies'
    /// own fallback (`git::resolve_default_branch`) to decide.
    pub base: Option<String>,
    /// The repository the pull request's head lives in, when that is not
    /// `github_repo`.
    pub head_repo: Option<String>,
    /// The pull request the branch belongs to: the one asked for by number,
    /// or the open one whose head is the caller's branch of `github_repo`.
    pub pull_request: Option<ResolvedPullRequest>,
}

/// What the callers of [`resolve_repo_spec`] need of a pull request: its
/// number for the branch row, and its title and description for the project
/// note the desktop forms write (`PR #<n>: <title>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedPullRequest {
    pub number: u64,
    pub title: String,
    pub body: String,
}

/// Check a repo request against GitHub and fill in what the caller left out.
///
/// The rules, each the one a desktop form applies before it submits:
///
/// - **Name.** The repository must exist, and is returned as GitHub spells
///   it, so a hand-typed `Acme/Widgets` cannot be attached beside
///   `acme/widgets`.
/// - **Subpath.** Normalised, and it must be a directory in the repository.
/// - **Monorepo.** A monorepo with no subpath is refused unless
///   `allow_monorepo_root` is set.
/// - **Pull request.** Fetched by number. When the named repository has no
///   such pull request and is a fork, its parent is tried, and the parent
///   becomes the repository (as `BranchPicker` does for a pasted fork URL).
///   A pull request that cannot be found is an error.
/// - **Branch.** A branch name gets a best-effort lookup of its open pull
///   request, which is then treated like one asked for by number. Only a
///   pull request whose head is that branch of the repository counts: one
///   opened from a fork that reuses the name is somebody else's branch.
/// - **Base.** The caller's, else the pull request's, else the repository's
///   default branch.
///
/// Blocking: every lookup runs `gh`. Lookups that don't depend on each other
/// run concurrently, so the whole call costs about three round-trips. Call
/// it on the blocking pool and ahead of the first store write.
pub(crate) fn resolve_repo_spec(
    host: &dyn RepoHost,
    request: RepoSpecRequest,
) -> Result<ResolvedRepoSpec, String> {
    let RepoSpecRequest {
        github_repo: named,
        subpath,
        branch_name,
        pr_number,
        base,
        allow_monorepo_root,
    } = request;
    let subpath = normalize_subpath(subpath.as_deref());
    let branch_name = non_blank(branch_name);
    let base = non_blank(base);

    let (owner, name) = split_repo_name(&named)?;
    let named_repo = host
        .canonical_repo(owner, name)
        .map_err(|e| format!("Could not look up repository '{}': {e}", named.trim()))?
        .ok_or_else(|| {
            format!(
                "Repository '{}' was not found on GitHub, or you don't have access to it.",
                named.trim()
            )
        })?;

    // The pull request goes first: finding it on a fork's parent changes
    // which repository everything after it is checked against.
    let (github_repo, numbered_pr, named_fork) = match pr_number {
        Some(number) => {
            let found = find_pull_request(host, &named_repo, number)?;
            if let Some(branch) = &branch_name {
                if *branch != found.pull_request.head_ref {
                    return Err(format!(
                        "Pull request #{number}'s branch is '{}', not '{branch}'. \
                         Give the pull request or the branch, not both.",
                        found.pull_request.head_ref
                    ));
                }
            }
            (
                found.github_repo,
                Some(found.pull_request),
                found.named_fork,
            )
        }
        None => (named_repo, None, None),
    };
    let branch_name = match &numbered_pr {
        Some(pr) => Some(pr.head_ref.clone()),
        None => branch_name,
    };

    let repo = github_repo.as_str();
    let (subpath_check, module_count, branch_pr, default_branch) = std::thread::scope(|scope| {
        let subpath_check = subpath
            .as_deref()
            .map(|subpath| scope.spawn(move || host.validate_subpath(repo, subpath)));
        // Only the bare root of a monorepo is refused, so the count matters
        // to nobody who gave a subpath or the override.
        let module_count = (subpath.is_none() && !allow_monorepo_root)
            .then(|| scope.spawn(move || host.monorepo_module_count(repo)));
        let branch_pr = match (&numbered_pr, branch_name.as_deref()) {
            (None, Some(branch)) => {
                Some(scope.spawn(move || host.open_pull_requests_for_branch(repo, branch)))
            }
            _ => None,
        };
        // Asked for before the branch's pull request is known, like the
        // form's prefetch: waiting to learn whether it is needed would cost
        // a round-trip whenever it is.
        let default_branch = (base.is_none() && numbered_pr.is_none())
            .then(|| scope.spawn(move || host.default_branch(repo)));
        (
            join(subpath_check),
            join(module_count),
            join(branch_pr),
            join(default_branch),
        )
    });

    if let (Some(subpath), Some(Err(e))) = (&subpath, subpath_check) {
        return Err(format!(
            "Subpath '{subpath}' can't be used in '{github_repo}': {e}"
        ));
    }

    match module_count {
        Some(Ok(count)) if is_monorepo(count) => {
            return Err(format!(
                "'{github_repo}' appears to be a monorepo ({count} modules in MODULES.yaml). \
                 Give the subpath of the service or package to work in, \
                 or pass --allow-monorepo-root to use the repository root."
            ));
        }
        // The form reads a failed check as "not a monorepo" too.
        Some(Err(e)) => log::warn!("[repo_spec] monorepo check failed for {github_repo}: {e}"),
        _ => {}
    }

    // The caller named a branch of this repository. GitHub's lookup goes by
    // the name alone, and on a public repository anyone can open a pull
    // request from a fork's branch of the same name. Adopting that one would
    // check out a stranger's code under the name the caller asked for, so
    // only a head in this repository counts. A head repository GitHub no
    // longer knows was a fork.
    let pull_request = numbered_pr.or_else(|| match branch_pr? {
        Ok(found) => found.into_iter().find(|pr| {
            pr.head_repo
                .as_deref()
                .is_some_and(|head| head.eq_ignore_ascii_case(&github_repo))
        }),
        Err(e) => {
            log::warn!("[repo_spec] pull request lookup for a branch of {github_repo} failed: {e}");
            None
        }
    });

    let pr_base = pull_request
        .as_ref()
        .map(|pr| pr.base_ref.trim())
        .filter(|base| !base.is_empty())
        .map(ToOwned::to_owned);
    let base = base.or(pr_base).or_else(|| match default_branch? {
        Ok(branch) => non_blank(Some(branch)),
        Err(e) => {
            log::warn!("[repo_spec] default branch detection failed for {github_repo}: {e}");
            None
        }
    });

    // Where the head lives is GitHub's to say. It can no longer say once a
    // fork is deleted, and then the fork the caller named is the best
    // answer, as it is for the form.
    let head_repo = pull_request
        .as_ref()
        .and_then(|pr| pr.head_repo.clone().or(named_fork))
        .filter(|head| !head.eq_ignore_ascii_case(&github_repo));

    Ok(ResolvedRepoSpec {
        subpath,
        branch_name,
        base,
        head_repo,
        pull_request: pull_request.map(|pr| ResolvedPullRequest {
            number: pr.number,
            title: pr.title,
            body: pr.body,
        }),
        github_repo,
    })
}

/// A pull request found by number, and where.
struct FoundPullRequest {
    /// The repository the pull request was opened against.
    github_repo: String,
    pull_request: PullRequest,
    /// The repository the caller named, when the pull request was found on
    /// its parent instead.
    named_fork: Option<String>,
}

/// Fetch pull request `number` from `github_repo`, or from the repository
/// `github_repo` was forked from when it isn't there.
fn find_pull_request(
    host: &dyn RepoHost,
    github_repo: &str,
    number: u64,
) -> Result<FoundPullRequest, String> {
    let not_here = match host.pull_request(github_repo, number) {
        Ok(pull_request) => {
            return Ok(FoundPullRequest {
                github_repo: github_repo.to_string(),
                pull_request,
                named_fork: None,
            });
        }
        Err(e) => e,
    };

    let parent = host.parent_repo(github_repo).unwrap_or_else(|e| {
        log::warn!("[repo_spec] parent lookup failed for {github_repo}: {e}");
        None
    });
    let Some(parent) = parent else {
        return Err(format!(
            "Could not find pull request #{number} in '{github_repo}': {not_here}"
        ));
    };
    match host.pull_request(&parent, number) {
        Ok(pull_request) => Ok(FoundPullRequest {
            github_repo: parent,
            pull_request,
            named_fork: Some(github_repo.to_string()),
        }),
        Err(e) => Err(format!(
            "Could not find pull request #{number} in '{github_repo}' \
             or in '{parent}', the repository it was forked from: {e}"
        )),
    }
}

/// `owner/repo` split into its two parts, refusing anything else: the parts
/// go into a GitHub API path.
fn split_repo_name(github_repo: &str) -> Result<(&str, &str), String> {
    let trimmed = github_repo.trim();
    trimmed
        .split_once('/')
        .filter(|(owner, name)| is_repo_name_part(owner) && is_repo_name_part(name))
        .ok_or_else(|| format!("Repository must be given as owner/repo (got '{trimmed}')."))
}

fn is_repo_name_part(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A subpath as the forms submit it: without surrounding whitespace or
/// slashes, and `None` when nothing is left.
fn normalize_subpath(subpath: Option<&str>) -> Option<String> {
    let normalized = subpath?.trim().trim_matches('/').trim();
    (!normalized.is_empty()).then(|| normalized.to_string())
}

fn non_blank(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(ToOwned::to_owned)
}

/// The result of a lookup spawned in a thread scope. A lookup that panicked
/// panics here, as it would have done if called inline.
fn join<T>(handle: Option<std::thread::ScopedJoinHandle<'_, T>>) -> Option<T> {
    handle.map(|handle| {
        handle
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

#[cfg(test)]
pub(super) mod tests;
