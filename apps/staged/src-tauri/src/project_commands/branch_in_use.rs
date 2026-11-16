//! Refusing a branch the same repository already has in a project.
//!
//! Two attachments of one repository share a clone, and a branch can be
//! checked out in it once. The shared add-repo body avoids the clash for a
//! name it infers, by adding a suffix. A name the caller gave, or a pull
//! request's branch, is used as it is, and the second checkout would fail in
//! the background setup, where only the log says so.
//!
//! The refusal is a control caller's: a desktop form shows the user the
//! project while they pick a branch. It runs twice. Once ahead of the GitHub
//! lookups, to save them ([`ensure_branch_is_free`]), and once beside the
//! write, under the lock [`claim_free_branch`] returns, so of two parallel
//! adds of one branch exactly one is written.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::git;
use crate::store::{self, Store};

/// The branch a caller asks to attach.
pub(super) struct BranchAsk<'a> {
    /// The repository, as GitHub spells it.
    pub github_repo: &'a str,
    /// The branch on origin: the name the caller gave, or the pull request's
    /// head branch.
    pub branch: &'a str,
    /// The pull request the branch belongs to, when there is one.
    pub pr_number: Option<u64>,
}

/// The one lock adds are written under, for the process.
static ADDING: Mutex<()> = Mutex::new(());

/// The lock a control caller's add holds from its last look at the project's
/// branches to the last row it writes, so a second add of the same branch
/// sees the first one's rows. An add that names no branch takes it here; one
/// that names a branch gets it from [`claim_free_branch`].
///
/// Nothing waits while holding it. An add writes a handful of rows under it,
/// and the check that precedes them reads the store and nothing else: what a
/// checkout tracks is asked of git before the lock is taken.
pub(super) fn adding() -> MutexGuard<'static, ()> {
    lock(&ADDING)
}

fn lock(adding: &Mutex<()>) -> MutexGuard<'_, ()> {
    adding
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Refuse a branch the same repository already has in the project.
///
/// The branch is taken when an attachment of the repository:
///
/// - is stored under the same branch name, or
/// - is for the same pull request, or
/// - was asked for under this name and renamed. When another project's
///   worktree holds the local name, the worktree code checks the branch out
///   as `<name>-2` and stores that. The branch on origin is still `<name>`,
///   and only git's upstream tracking says so, so a stored name of that
///   shape is settled by asking its checkout.
///
/// This takes no lock, so the answer can be out of date by the time the
/// caller acts on it. [`claim_free_branch`] is the one to write under.
pub(super) fn ensure_branch_is_free(
    store: &Store,
    project_id: &str,
    ask: &BranchAsk<'_>,
) -> Result<(), String> {
    ensure_branch_is_free_with(store, project_id, ask, &origin_branch_of_checkout)
}

/// [`ensure_branch_is_free`] with the lookup of a checkout's branch on origin
/// passed in, so the rule can be tested without a clone.
fn ensure_branch_is_free_with(
    store: &Store,
    project_id: &str,
    ask: &BranchAsk<'_>,
    origin_branch_of: &dyn Fn(&Store, &store::Branch) -> Option<String>,
) -> Result<(), String> {
    match judge(store, project_id, ask, &|row| {
        Some(origin_branch_of(store, row))
    })? {
        Verdict::Free => Ok(()),
        Verdict::Taken(refusal) => Err(refusal),
        // Every checkout asked about was answered just above.
        Verdict::Unsettled(_) => Ok(()),
    }
}

/// [`ensure_branch_is_free`], and the [`adding`] lock when the branch is
/// free. The caller writes its rows and then lets the lock go, so the rows
/// are written with the answer still true.
///
/// Settling a renamed branch means asking git what its checkout tracks,
/// which spawns a process. That is not done under the lock, which every add
/// in the process takes. The rule is read under the lock from the store and
/// from what is already known of the checkouts. When it turns on a checkout
/// not yet asked about, the lock is let go, the checkout is asked, and the
/// rule is read again from the start under the lock. So the read that admits
/// the add is always made under the lock the rows are written under.
///
/// A checkout is asked about once in a claim. It is asked only when its row
/// has the renamed name, and the worktree code sets the upstream before it
/// stores that name, so what the checkout tracks does not change after that.
/// A row whose worktree is not linked yet has no checkout to ask and reads
/// as tracking nothing, as it did when git was asked under the lock.
pub(super) fn claim_free_branch(
    store: &Store,
    project_id: &str,
    ask: &BranchAsk<'_>,
) -> Result<MutexGuard<'static, ()>, String> {
    claim_free_branch_with(&ADDING, store, project_id, ask, &origin_branch_of_checkout)
}

/// [`claim_free_branch`] with the lock and the lookup of a checkout's branch
/// on origin passed in, so a test can hold the one and watch the other.
fn claim_free_branch_with<'a>(
    adding: &'a Mutex<()>,
    store: &Store,
    project_id: &str,
    ask: &BranchAsk<'_>,
    origin_branch_of: &dyn Fn(&Store, &store::Branch) -> Option<String>,
) -> Result<MutexGuard<'a, ()>, String> {
    // What each checkout asked about so far tracks on origin, by branch id.
    let mut asked: HashMap<String, Option<String>> = HashMap::new();
    // Each round asks about at least one checkout not asked about before,
    // and a project has only so many, so this ends.
    loop {
        let unsettled = {
            let guard = lock(adding);
            match judge(store, project_id, ask, &|row| asked.get(&row.id).cloned())? {
                Verdict::Free => return Ok(guard),
                Verdict::Taken(refusal) => return Err(refusal),
                Verdict::Unsettled(rows) => rows,
            }
        };
        // The lock is free again here.
        for row in unsettled {
            let tracks = origin_branch_of(store, &row);
            asked.insert(row.id, tracks);
        }
    }
}

/// What [`judge`] makes of a branch a caller asks to attach.
enum Verdict {
    Free,
    /// An attachment has the branch. The refusal, worded for the caller.
    Taken(String),
    /// Nothing known has the branch, and these rows might: each is stored
    /// under a name the worktree code could have made from the one asked
    /// for, and what its checkout tracks is not known yet.
    Unsettled(Vec<store::Branch>),
}

/// The rule of [`ensure_branch_is_free`], read from the store. `tracks`
/// answers what a branch row's checkout tracks on origin: `Some(answer)`
/// when that is known, where the answer is `None` for a branch with no
/// checkout, and `None` when it has not been looked up.
fn judge(
    store: &Store,
    project_id: &str,
    ask: &BranchAsk<'_>,
    tracks: &dyn Fn(&store::Branch) -> Option<Option<String>>,
) -> Result<Verdict, String> {
    let attached: Vec<store::ProjectRepo> = store
        .list_project_repos(project_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|repo| repo.github_repo.eq_ignore_ascii_case(ask.github_repo))
        .collect();
    if attached.is_empty() {
        return Ok(Verdict::Free);
    }
    let branches = store
        .list_branches_for_project(project_id)
        .map_err(|e| e.to_string())?;

    let mut unsettled = Vec::new();
    let taken = attached.iter().find(|repo| {
        if repo.branch_name == ask.branch {
            return true;
        }
        let Some(row) = branches
            .iter()
            .find(|branch| branch.project_repo_id.as_deref() == Some(repo.id.as_str()))
        else {
            return false;
        };
        if ask.pr_number.is_some() && row.pr_number == ask.pr_number {
            return true;
        }
        if !is_deduplicated_name_of(&repo.branch_name, ask.branch) {
            return false;
        }
        match tracks(row) {
            Some(tracked) => tracked.as_deref() == Some(ask.branch),
            None => {
                unsettled.push(row.clone());
                false
            }
        }
    });
    let Some(taken) = taken else {
        return Ok(if unsettled.is_empty() {
            Verdict::Free
        } else {
            Verdict::Unsettled(unsettled)
        });
    };

    let place = match taken.subpath.as_deref() {
        Some(subpath) => format!("at subpath '{subpath}'"),
        None => "at the repository root".to_string(),
    };
    let checked_out_as = if taken.branch_name == ask.branch {
        String::new()
    } else {
        format!(", checked out as '{}'", taken.branch_name)
    };
    Ok(Verdict::Taken(format!(
        "Branch '{}' of '{}' is already attached to this project {place}{checked_out_as}. \
         A branch can be checked out once per project, so give a different branch name.",
        ask.branch, ask.github_repo
    )))
}

/// Whether `stored` is a name the worktree code makes for `branch` when the
/// local name is held elsewhere: `<branch>-2` to `<branch>-50`
/// (`branches::unique_branch_name`). A branch really named that way matches
/// too, which is why the checkout is asked next.
fn is_deduplicated_name_of(stored: &str, branch: &str) -> bool {
    stored
        .strip_prefix(branch)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|suffix| suffix.parse::<u32>().ok())
        .is_some_and(|suffix| (2..=50).contains(&suffix))
}

/// The branch on origin a branch row's local checkout pushes to, read from
/// git's upstream tracking. `None` when the branch has no checkout yet, or
/// none on this machine.
fn origin_branch_of_checkout(store: &Store, branch: &store::Branch) -> Option<String> {
    let workdir = store.get_workdir_for_branch(&branch.id).ok()??;
    let path = Path::new(&workdir.path);
    path.is_dir()
        .then(|| git::upstream_branch_name(path, &branch.branch_name, &branch.base_branch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_checkout(_: &Store, _: &store::Branch) -> Option<String> {
        None
    }

    /// A project holding `acme/widgets` on `branch`, with its branch row.
    fn project_with(
        store: &Store,
        branch: &str,
        subpath: Option<&str>,
        pr_number: Option<u64>,
    ) -> String {
        let project = store::Project::named("alpha");
        store.create_project(&project).unwrap();
        let repo = store::ProjectRepo::new(
            &project.id,
            "acme/widgets",
            branch,
            subpath.map(str::to_string),
        );
        store.create_project_repo(&repo).unwrap();
        let mut row =
            store::Branch::new(&project.id, branch, "origin/main").with_project_repo(&repo.id);
        if let Some(pr) = pr_number {
            row = row.with_pr(pr);
        }
        store.create_branch(&row).unwrap();
        project.id
    }

    fn ask<'a>(github_repo: &'a str, branch: &'a str, pr_number: Option<u64>) -> BranchAsk<'a> {
        BranchAsk {
            github_repo,
            branch,
            pr_number,
        }
    }

    #[test]
    fn a_branch_stored_under_the_same_name_is_taken() {
        let store = Store::in_memory().unwrap();
        let project_id = project_with(&store, "fix-login", Some("services/auth"), None);

        let err = ensure_branch_is_free_with(
            &store,
            &project_id,
            &ask("Acme/Widgets", "fix-login", None),
            &no_checkout,
        )
        .unwrap_err();

        assert_eq!(
            err,
            "Branch 'fix-login' of 'Acme/Widgets' is already attached to this project at \
             subpath 'services/auth'. A branch can be checked out once per project, so give \
             a different branch name."
        );
        // Another branch, another repository and another project are free.
        for free in [
            ask("acme/widgets", "fix-logout", None),
            ask("acme/gadgets", "fix-login", None),
        ] {
            ensure_branch_is_free_with(&store, &project_id, &free, &no_checkout).unwrap();
        }
        ensure_branch_is_free_with(
            &store,
            "another-project",
            &ask("acme/widgets", "fix-login", None),
            &no_checkout,
        )
        .unwrap();
    }

    /// A pull request's branch that the worktree code renamed is still that
    /// pull request's: the number on the branch row says so, whatever the
    /// local name became and wherever the head lives.
    #[test]
    fn a_renamed_pull_request_branch_is_taken_by_its_number() {
        let store = Store::in_memory().unwrap();
        let project_id = project_with(&store, "fix-login-2", None, Some(42));

        let err = ensure_branch_is_free_with(
            &store,
            &project_id,
            &ask("acme/widgets", "fix-login", Some(42)),
            &no_checkout,
        )
        .unwrap_err();

        assert!(
            err.contains("Branch 'fix-login' of 'acme/widgets' is already attached")
                && err.contains("at the repository root, checked out as 'fix-login-2'"),
            "{err}"
        );
        // Another pull request is free, and so is the name without one:
        // nothing says this checkout is that branch.
        ensure_branch_is_free_with(
            &store,
            &project_id,
            &ask("acme/widgets", "other", Some(43)),
            &no_checkout,
        )
        .unwrap();
        ensure_branch_is_free_with(
            &store,
            &project_id,
            &ask("acme/widgets", "fix-login", None),
            &no_checkout,
        )
        .unwrap();
    }

    /// A branch with no pull request that was renamed is known by what its
    /// checkout tracks on origin.
    #[test]
    fn a_renamed_branch_is_taken_when_its_checkout_tracks_the_name() {
        let store = Store::in_memory().unwrap();
        let project_id = project_with(&store, "fix-login-2", None, None);
        let tracks = |name: &'static str| {
            move |_: &Store, row: &store::Branch| {
                assert_eq!(row.branch_name, "fix-login-2");
                Some(name.to_string())
            }
        };

        let err = ensure_branch_is_free_with(
            &store,
            &project_id,
            &ask("acme/widgets", "fix-login", None),
            &tracks("fix-login"),
        )
        .unwrap_err();
        assert!(err.contains("checked out as 'fix-login-2'"), "{err}");

        // A branch that really is called `fix-login-2` tracks itself, and
        // leaves `fix-login` free.
        ensure_branch_is_free_with(
            &store,
            &project_id,
            &ask("acme/widgets", "fix-login", None),
            &tracks("fix-login-2"),
        )
        .unwrap();
    }

    /// The checkout is only asked about a stored name the worktree code
    /// could have made from the one asked for.
    #[test]
    fn only_a_deduplicated_name_sends_the_check_to_the_checkout() {
        let store = Store::in_memory().unwrap();
        let project_id = project_with(&store, "fix-login-auth", None, None);
        let never = |_: &Store, _: &store::Branch| -> Option<String> {
            panic!("an unrelated stored name must not be looked up")
        };

        ensure_branch_is_free_with(
            &store,
            &project_id,
            &ask("acme/widgets", "fix-login", None),
            &never,
        )
        .unwrap();

        assert!(is_deduplicated_name_of("fix-login-2", "fix-login"));
        assert!(is_deduplicated_name_of("fix-login-50", "fix-login"));
        for (stored, branch) in [
            ("fix-login", "fix-login"),
            ("fix-login-1", "fix-login"),
            ("fix-login-51", "fix-login"),
            ("fix-login-auth", "fix-login"),
            ("fix-login2", "fix-login"),
            ("fix-login--2", "fix-login"),
            ("other-2", "fix-login"),
        ] {
            assert!(!is_deduplicated_name_of(stored, branch), "{stored}");
        }
    }

    /// A claim settles a renamed branch by asking git about its checkout,
    /// and does that with the lock free: every add in the process takes the
    /// lock, and none of them should wait on a spawned process. The checkout
    /// is asked once, and the lock is held when the claim returns.
    #[test]
    fn a_claim_asks_the_checkout_with_the_lock_free() {
        let store = Store::in_memory().unwrap();
        let project_id = project_with(&store, "fix-login-2", None, None);
        let adding = Mutex::new(());
        let asked = std::cell::Cell::new(0);
        let tracks = |name: &'static str| {
            let (adding, asked) = (&adding, &asked);
            move |_: &Store, row: &store::Branch| {
                asked.set(asked.get() + 1);
                assert_eq!(row.branch_name, "fix-login-2");
                assert!(
                    adding.try_lock().is_ok(),
                    "the checkout is asked while the lock is held"
                );
                Some(name.to_string())
            }
        };

        // The checkout tracks the branch asked for: refused, and the lock is
        // not left held.
        let err = claim_free_branch_with(
            &adding,
            &store,
            &project_id,
            &ask("acme/widgets", "fix-login", None),
            &tracks("fix-login"),
        )
        .unwrap_err();
        assert!(err.contains("checked out as 'fix-login-2'"), "{err}");
        assert_eq!(asked.get(), 1);
        assert!(adding.try_lock().is_ok());

        // A branch really called `fix-login-2` leaves `fix-login` free, and
        // the claim comes back holding the lock.
        asked.set(0);
        let claim = claim_free_branch_with(
            &adding,
            &store,
            &project_id,
            &ask("acme/widgets", "fix-login", None),
            &tracks("fix-login-2"),
        )
        .unwrap();
        assert_eq!(asked.get(), 1, "one checkout is asked about once");
        assert!(adding.try_lock().is_err(), "the claim holds the lock");
        drop(claim);
        assert!(adding.try_lock().is_ok());
    }

    /// The read that admits an add is made under the lock, after the
    /// checkouts were asked. An add of the same branch that was written
    /// while git was being asked is seen by it.
    #[test]
    fn a_branch_attached_while_the_checkout_was_asked_is_seen_by_the_claim() {
        let store = Store::in_memory().unwrap();
        let project_id = project_with(&store, "fix-login-2", None, None);
        let adding = Mutex::new(());
        let parallel_add = |store: &Store, _: &store::Branch| {
            // Another add of `fix-login`, at a subpath, takes the lock for
            // its rows while this one is asking git.
            let _adding = adding
                .try_lock()
                .expect("the lock is free during the lookup");
            let repo = store::ProjectRepo::new(
                &project_id,
                "acme/widgets",
                "fix-login",
                Some("docs".to_string()),
            );
            store.create_project_repo(&repo).unwrap();
            store
                .create_branch(
                    &store::Branch::new(&project_id, "fix-login", "origin/main")
                        .with_project_repo(&repo.id),
                )
                .unwrap();
            // The renamed branch is another branch altogether.
            Some("fix-login-2".to_string())
        };

        let err = claim_free_branch_with(
            &adding,
            &store,
            &project_id,
            &ask("acme/widgets", "fix-login", None),
            &parallel_add,
        )
        .map(drop)
        .unwrap_err();

        assert_eq!(
            err,
            "Branch 'fix-login' of 'acme/widgets' is already attached to this project at \
             subpath 'docs'. A branch can be checked out once per project, so give a \
             different branch name."
        );
        assert!(adding.try_lock().is_ok());
    }

    /// A claim that turns on no checkout asks git nothing: a free branch is
    /// claimed, and one taken by name or by pull request is refused, from
    /// the store alone.
    #[test]
    fn a_claim_settled_by_the_store_asks_no_checkout() {
        let store = Store::in_memory().unwrap();
        let project_id = project_with(&store, "fix-login", None, Some(42));
        let adding = Mutex::new(());
        let never = |_: &Store, _: &store::Branch| -> Option<String> {
            panic!("the store settles this, so no checkout is asked")
        };
        let claim = |ask: BranchAsk<'_>| {
            claim_free_branch_with(&adding, &store, &project_id, &ask, &never).map(drop)
        };

        claim(ask("acme/widgets", "fix-logout", None)).unwrap();
        claim(ask("acme/gadgets", "fix-login", None)).unwrap();
        let by_name = claim(ask("acme/widgets", "fix-login", None)).unwrap_err();
        assert!(by_name.contains("is already attached"), "{by_name}");
        let by_pr = claim(ask("acme/widgets", "renamed-head", Some(42))).unwrap_err();
        assert!(by_pr.contains("checked out as 'fix-login'"), "{by_pr}");
        assert!(adding.try_lock().is_ok());
    }

    /// The real lookup reads git's upstream tracking in the branch's
    /// worktree, and has nothing to say about a branch with no checkout.
    #[test]
    fn a_branch_without_a_checkout_tracks_nothing() {
        let store = Store::in_memory().unwrap();
        let project_id = project_with(&store, "fix-login-2", None, None);
        let row = store
            .list_branches_for_project(&project_id)
            .unwrap()
            .remove(0);

        assert_eq!(origin_branch_of_checkout(&store, &row), None);

        // A workdir row whose directory is gone is no checkout either.
        let workdir = store::Workdir::new(&project_id, "/nonexistent/staged-test/fix-login-2")
            .with_branch(&row.id);
        store.create_workdir(&workdir).unwrap();
        assert_eq!(origin_branch_of_checkout(&store, &row), None);
    }
}
