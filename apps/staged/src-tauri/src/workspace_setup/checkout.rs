//! Putting a workstation's clone on a branch without losing or moving work.
//!
//! [`check_out_workspace_branch`] is the checkout every branch setup ends in.
//! It runs in a clone that may already hold work: a branch's unpushed
//! commits, another branch of the same repository (they share the clone),
//! or commits an agent made on the base branch because the branch was never
//! checked out — the state a swallowed setup failure used to leave behind.
//!
//! So it switches only when the switch keeps everything where it is, with
//! one exception that brings work along on purpose: a clone on the base
//! branch, ahead of origin's base, whose branch exists nowhere gets the
//! branch created from where it stands, so the commits made there come with
//! it. Local base is left pointing at them, never reset or moved.
//!
//! A switch that would lose work or move it between branches is
//! [`CheckoutError::Refused`], with the clone untouched and a message that
//! names the branches and the commits. An exec that failed is
//! [`CheckoutError::Failed`]: it says nothing about the clone and is worth
//! trying again.

use crate::branches::GitRunner;

/// Why a checkout did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckoutError {
    /// The clone is in a state the checkout will not switch away from,
    /// because doing so would lose work or move it onto another branch. The
    /// clone is as it was; trying again changes nothing until someone has
    /// dealt with what is in it.
    Refused(String),
    /// Something outside the clone is in the way for now: a session working
    /// in it, a workstation that is not running. The clone is as it was,
    /// and trying again later is worth it.
    Blocked(String),
    /// A step failed: a fetch, a probe, the checkout itself.
    Failed(String),
}

impl CheckoutError {
    /// Whether the clone was left exactly as it was.
    pub(crate) fn is_refusal(&self) -> bool {
        !matches!(self, CheckoutError::Failed(_))
    }

    /// Whether trying again on its own could end differently. Not after a
    /// refusal over what is in the clone: that waits for someone to deal
    /// with it and ask again.
    pub(crate) fn worth_retrying(&self) -> bool {
        !matches!(self, CheckoutError::Refused(_))
    }

    pub(crate) fn message(&self) -> &str {
        match self {
            CheckoutError::Refused(message)
            | CheckoutError::Blocked(message)
            | CheckoutError::Failed(message) => message,
        }
    }

    /// The same error with its message rewritten.
    pub(crate) fn map_message(self, f: impl FnOnce(String) -> String) -> Self {
        match self {
            CheckoutError::Refused(message) => CheckoutError::Refused(f(message)),
            CheckoutError::Blocked(message) => CheckoutError::Blocked(f(message)),
            CheckoutError::Failed(message) => CheckoutError::Failed(f(message)),
        }
    }
}

impl std::fmt::Display for CheckoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// What to put the clone on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BranchCheckout<'a> {
    pub(crate) base_ref: &'a str,
    pub(crate) branch_name: &'a str,
    /// Whether the base branch is itself a branch Staged works on in this
    /// clone. Its local commits are then that branch's own, not ones left
    /// behind on the base by mistake, and they are neither brought along
    /// nor a reason to refuse.
    pub(crate) base_is_worked_on: bool,
}

impl<'a> BranchCheckout<'a> {
    /// A checkout in a clone whose base branch nothing works on.
    #[cfg(test)]
    pub(crate) fn new(base_ref: &'a str, branch_name: &'a str) -> Self {
        Self {
            base_ref,
            branch_name,
            base_is_worked_on: false,
        }
    }
}

/// How many commits a message lists before it says "and N more".
const LISTED_COMMITS: usize = 5;

/// What the clone has checked out, as far as a checkout needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Head {
    Branch(String),
    Detached,
}

impl std::fmt::Display for Head {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Head::Branch(name) => write!(f, "branch '{name}'"),
            Head::Detached => f.write_str("a detached HEAD"),
        }
    }
}

fn failed(what: impl std::fmt::Display, error: impl std::fmt::Display) -> CheckoutError {
    CheckoutError::Failed(format!("{what}: {error}"))
}

/// Fetch `base_ref` into the clone `git` runs in, and put the clone on
/// `branch_name`.
///
/// A branch the clone already has is checked out as it is, never recreated.
/// Every remote branch of a repo shares one clone, and this runs again
/// whenever any of them is set up again (e.g. after `resume_workspace`), so
/// `checkout -B` would reset a branch onto origin and drop its unpushed
/// commits, whichever branch HEAD happened to be on. When origin has the
/// branch too, it is then fast-forwarded if it can be: a branch that is only
/// behind picks up what was pushed, and one that has diverged keeps its local
/// commits. Only a branch the clone doesn't have yet is created: from
/// origin's copy, from where the clone stands when that is the base branch
/// with commits origin's base doesn't have (see the module docs), or from
/// origin's base.
///
/// Before the clone leaves what it has checked out, [`guard_leaving`] makes
/// sure nothing is lost or carried across by the switch.
pub(crate) fn check_out_workspace_branch(
    git: &GitRunner<'_>,
    checkout: &BranchCheckout<'_>,
) -> Result<(), CheckoutError> {
    let BranchCheckout {
        base_ref,
        branch_name,
        ..
    } = *checkout;
    git(&["fetch", "origin", base_ref])
        .map_err(|e| failed(format!("could not fetch base branch '{base_ref}'"), e))?;

    // If the branch already exists on the remote (e.g. from an existing PR),
    // fetch it so we pick up its commits: a new branch starts from there and
    // an existing one fast-forwards to it. Otherwise a new branch falls back
    // to starting from the base branch.
    //
    // Whether it exists is read from `ls-remote`'s output, not from a fetch
    // failing: a fetch that never reached origin (a dropped exec, a network
    // blip) would otherwise read as "not on origin" and branch from the base,
    // leaving the PR's commits behind. Any failure here is a failure.
    let local_ref = format!("refs/heads/{branch_name}");
    let has_remote_branch = branch_name != base_ref
        && !git(&["ls-remote", "--heads", "origin", &local_ref])
            .map_err(|e| {
                failed(
                    format!("could not check whether '{branch_name}' is on origin"),
                    e,
                )
            })?
            .trim()
            .is_empty();
    if has_remote_branch {
        git(&["fetch", "origin", branch_name])
            .map_err(|e| failed(format!("could not fetch branch '{branch_name}'"), e))?;
    }
    let remote_branch = format!("origin/{branch_name}");

    // Whether the clone has the branch decides between checking it out as it
    // is and creating it. A failed exec is neither answer: read as
    // "missing", it would recreate a branch the clone does have from the
    // base, moving it off its unpushed commits.
    let has_local_branch = local_branch_exists(git, branch_name).map_err(CheckoutError::Failed)?;

    let head = current_head(git)?;
    let on_branch = head == Head::Branch(branch_name.to_string());
    let stranded = if on_branch {
        Vec::new()
    } else {
        guard_leaving(git, &head, checkout)?
    };

    if has_local_branch {
        if !stranded.is_empty() {
            return Err(refuse_stranded(&stranded, checkout, "the clone has"));
        }
        if !on_branch {
            switch(git, &head, branch_name, &["checkout", branch_name])?;
        }
        if has_remote_branch {
            // Fails when the branch has diverged from origin (or local
            // changes are in the way). The branch then stays as it is, local
            // commits and all, which is the point: never reset it.
            let _ = git(&["merge", "--ff-only", &remote_branch]);
        }
        return Ok(());
    }

    if has_remote_branch {
        if !stranded.is_empty() {
            return Err(refuse_stranded(&stranded, checkout, "origin has"));
        }
        // `checkout -B <branch> origin/<branch>` makes the checkout track
        // the branch it pushes to.
        return switch(
            git,
            &head,
            branch_name,
            &["checkout", "-B", branch_name, &remote_branch],
        );
    }

    if !stranded.is_empty() {
        // The branch exists nowhere and the base branch here holds commits
        // origin's doesn't: work made on the base because the branch was
        // never checked out. The branch is created where the clone stands,
        // so that work is on it. `checkout -b` keeps the worktree as it is
        // (it cannot conflict) and leaves local base where it was.
        log::warn!(
            "[workspace_setup] creating '{branch_name}' from local '{base_ref}', which holds {} \
             commit(s) that are on no other branch and not on origin: {}",
            stranded.len(),
            describe_commits(&stranded)
        );
        return git(&["checkout", "--no-track", "-b", branch_name])
            .map(|_| ())
            .map_err(|e| failed(format!("could not create branch '{branch_name}'"), e));
    }

    // A fresh branch must not track `origin/<base>` — git's upstream is read
    // back as the push target.
    switch(
        git,
        &head,
        branch_name,
        &[
            "checkout",
            "--no-track",
            "-B",
            branch_name,
            &format!("origin/{base_ref}"),
        ],
    )
}

/// Whether the clone `git` runs in has a local branch `branch_name`.
///
/// Answered through stdout: `for-each-ref` exits 0 whether or not the ref
/// exists and prints it only when it does, so "missing" is an empty answer
/// from a command that ran. A probe that answers through its exit status
/// (`rev-parse --verify --quiet`) can't be told apart from an exec that never
/// reached the workstation, and here that failure is an error.
pub(crate) fn local_branch_exists(git: &GitRunner<'_>, branch_name: &str) -> Result<bool, String> {
    let local_ref = format!("refs/heads/{branch_name}");
    let refs = git(&["for-each-ref", "--format=%(refname)", &local_ref]).map_err(|e| {
        format!("could not check whether the clone already has branch '{branch_name}': {e}")
    })?;
    // The pattern also matches refs under `<branch>/`, so only an exact line
    // counts.
    Ok(refs.lines().any(|line| line.trim() == local_ref))
}

/// What the clone has checked out. `rev-parse --abbrev-ref HEAD` exits 0 for
/// a branch and for a detached HEAD alike (it prints `HEAD` for the latter),
/// so a failed exec is not mistaken for either.
fn current_head(git: &GitRunner<'_>) -> Result<Head, CheckoutError> {
    let out = git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(|e| failed("could not read what the clone has checked out", e))?;
    match out.trim() {
        "" => Err(CheckoutError::Failed(
            "could not read what the clone has checked out: empty answer".to_string(),
        )),
        "HEAD" => Ok(Head::Detached),
        name => Ok(Head::Branch(name.to_string())),
    }
}

/// Make sure the clone can leave `head` for the branch without losing work
/// or carrying it across, and return the commits it would leave stranded on
/// the base branch (see the module docs) for the caller to bring along or
/// refuse over.
///
/// - A detached HEAD holding commits no ref reaches: leaving drops them.
/// - Another branch with uncommitted changes: git would carry them onto
///   this branch, or abort. They are that branch's work.
/// - The base branch holding commits nothing else does: reported, not
///   refused here.
///   Uncommitted changes on the base are left to git, which carries them
///   along (a bootstrap that touched a lockfile, an agent's work in
///   progress) or aborts untouched ([`switch`]).
fn guard_leaving(
    git: &GitRunner<'_>,
    head: &Head,
    checkout: &BranchCheckout<'_>,
) -> Result<Vec<String>, CheckoutError> {
    let BranchCheckout {
        base_ref,
        branch_name,
        base_is_worked_on,
    } = *checkout;
    match head {
        Head::Detached => {
            let unreachable = ["HEAD", "--not", "--branches", "--tags", "--remotes"];
            let orphaned = list_commits(git, &unreachable)?;
            if orphaned.is_empty() {
                return Ok(Vec::new());
            }
            Err(CheckoutError::Refused(format!(
                "the clone has a detached HEAD with {} commit(s) that are on no branch ({}). \
                 Checking out '{branch_name}' would leave them unreachable, so the clone was \
                 left as it is. Put them on a branch in the workstation, then try again.",
                orphaned.len(),
                describe_commits(&orphaned)
            )))
        }
        // Commits that only local base holds: on no remote ref and on no
        // other local branch. Once a branch has been created from them they
        // are that branch's, and no longer stranded here.
        Head::Branch(name) if name == base_ref && !base_is_worked_on => list_commits(
            git,
            &[
                "HEAD",
                "--not",
                "--remotes",
                &format!("--exclude={base_ref}"),
                "--branches",
            ],
        ),
        Head::Branch(name) => {
            let changed = uncommitted_changes(git)?;
            if changed == 0 {
                return Ok(Vec::new());
            }
            Err(CheckoutError::Refused(format!(
                "the clone has branch '{name}' checked out with uncommitted changes to \
                 {changed} file(s). Checking out '{branch_name}' would carry them onto it, so \
                 the clone was left on '{name}'. Commit or stash them there, then try again."
            )))
        }
    }
}

/// The refusal for a clone on the base branch whose local commits can't be
/// brought along by creating the branch, because the branch already exists.
fn refuse_stranded(
    stranded: &[String],
    checkout: &BranchCheckout<'_>,
    where_it_is: &str,
) -> CheckoutError {
    let BranchCheckout {
        base_ref,
        branch_name,
        ..
    } = *checkout;
    CheckoutError::Refused(format!(
        "the clone is on '{base_ref}' with {} commit(s) that are on no other branch and not on \
         origin ({}), and {where_it_is} a branch '{branch_name}' already, so they can't be \
         brought along by creating it. Checking out '{branch_name}' would leave them behind on '{base_ref}', so \
         the clone was left as it is. Move them onto '{branch_name}' in the workstation (or \
         reset '{base_ref}' if they are not wanted), then try again.",
        stranded.len(),
        describe_commits(stranded)
    ))
}

/// Run the checkout that leaves `head`. Git aborts a checkout that local
/// changes are in the way of without touching anything, so a failure over a
/// worktree that isn't clean is a refusal, not something to try again.
fn switch(
    git: &GitRunner<'_>,
    head: &Head,
    branch_name: &str,
    args: &[&str],
) -> Result<(), CheckoutError> {
    let error = match git(args) {
        Ok(_) => return Ok(()),
        Err(e) => e,
    };
    match uncommitted_changes(git) {
        Ok(changed) if changed > 0 => Err(CheckoutError::Refused(format!(
            "the clone has {head} checked out with uncommitted changes to {changed} file(s) \
             that are in the way of checking out '{branch_name}', so the clone was left as it \
             is ({}). Commit or stash them in the workstation, then try again.",
            error.trim()
        ))),
        _ => Err(failed(
            format!("could not check out branch '{branch_name}'"),
            error,
        )),
    }
}

/// How many paths `git status` reports: staged, unstaged and untracked.
/// `--no-optional-locks` keeps the read from taking `.git/index.lock`, which
/// a killed exec would leave behind.
fn uncommitted_changes(git: &GitRunner<'_>) -> Result<usize, CheckoutError> {
    let out = git(&["--no-optional-locks", "status", "--porcelain"])
        .map_err(|e| failed("could not read the clone's uncommitted changes", e))?;
    Ok(out.lines().filter(|line| !line.trim().is_empty()).count())
}

/// The commits in `range`, newest first, as `<short sha> <subject>`.
fn list_commits(git: &GitRunner<'_>, range: &[&str]) -> Result<Vec<String>, CheckoutError> {
    let mut args = vec!["log", "--format=%h %s"];
    args.extend_from_slice(range);
    let out = git(&args).map_err(|e| failed("could not list the clone's local commits", e))?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn describe_commits(commits: &[String]) -> String {
    let listed = commits
        .iter()
        .take(LISTED_COMMITS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    match commits.len().saturating_sub(LISTED_COMMITS) {
        0 => listed,
        more => format!("{listed}; and {more} more"),
    }
}
