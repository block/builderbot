//! The all-projects reads behind the project grid, the board and the
//! sidebar: every project's branch list, and every project's repo list, each
//! as one reply keyed by project id.
//!
//! The frontend used to ask per project, two commands each, and applied
//! every reply on its own, which redrew every card once per project. These
//! return the same lists the per-project commands do (`BranchWithWorkdir`
//! and `ProjectRepo`, unchanged) so that it can apply them in one pass.
//!
//! Each loops the per-project read over `list_projects`, taking and
//! releasing the store lock per read as the per-project commands do, so a
//! thousand projects never hold the lock against a write for the whole
//! pass. The reads are therefore not one snapshot: a project deleted partway
//! through is listed with whatever it had left, which the frontend drops
//! with the project when its `project-changed` reload lands.
//!
//! A project whose own read fails is left out of the reply, and the failure
//! logged: one unreadable project must not cost every other project its
//! lists. The frontend reads a project the reply lacks on its own, with the
//! per-project command, which then reports that project's error for that
//! project alone. Only a failure to list the projects fails the reply.

use std::collections::BTreeMap;

use crate::branches::project_branches_with_workdirs;
use crate::store::{ProjectRepo, Store};
use crate::BranchWithWorkdir;

/// Every project's branches, as `list_branches_for_project` lists them. A
/// project with no branches is listed with an empty list, so a project that
/// is absent is one the store did not hold when this ran, or one whose
/// branches could not be read.
pub(crate) fn list_branches_by_project_impl(
    store: &Store,
) -> Result<BTreeMap<String, Vec<BranchWithWorkdir>>, String> {
    let projects = store.list_projects().map_err(|e| e.to_string())?;
    Ok(lists_by_project(
        projects.into_iter().map(|project| project.id),
        "branches",
        |project_id| project_branches_with_workdirs(store, project_id),
    ))
}

/// Every project's repos, as `list_project_repos` lists them: primary first.
/// A project with no repos is listed with an empty list, and one whose repos
/// could not be read is absent.
pub(crate) fn list_project_repos_by_project_impl(
    store: &Store,
) -> Result<BTreeMap<String, Vec<ProjectRepo>>, String> {
    let projects = store.list_projects().map_err(|e| e.to_string())?;
    Ok(lists_by_project(
        projects.into_iter().map(|project| project.id),
        "repos",
        |project_id| {
            store
                .list_project_repos(project_id)
                .map_err(|e| e.to_string())
        },
    ))
}

/// One list per project, keyed by project id. A project whose `read` fails
/// is logged and left out, so the rest of the reply still stands (see the
/// module docs).
fn lists_by_project<T>(
    project_ids: impl IntoIterator<Item = String>,
    what: &str,
    read: impl Fn(&str) -> Result<Vec<T>, String>,
) -> BTreeMap<String, Vec<T>> {
    let mut by_project = BTreeMap::new();
    for project_id in project_ids {
        match read(&project_id) {
            Ok(list) => {
                by_project.insert(project_id, list);
            }
            Err(error) => {
                log::warn!(
                    "Leaving project '{project_id}' out of the all-projects {what} list: {error}"
                );
            }
        }
    }
    by_project
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branches::list_branches_for_project_impl;
    use crate::store::{Branch, Commit, Project, Workdir};

    fn project(store: &Store, name: &str) -> Project {
        let project = Project::named(name);
        store.create_project(&project).unwrap();
        project
    }

    fn json<T: serde::Serialize>(value: &T) -> serde_json::Value {
        serde_json::to_value(value).unwrap()
    }

    #[test]
    fn branches_are_keyed_by_project_and_match_the_per_project_list() {
        let store = Store::in_memory().unwrap();
        let alpha = project(&store, "alpha");
        let beta = project(&store, "beta");
        let empty = project(&store, "empty");

        let feature = Branch::new(&alpha.id, "feature", "main");
        store.create_branch(&feature).unwrap();
        store
            .create_workdir(&Workdir::new(&alpha.id, "/tmp/wt/feature").with_branch(&feature.id))
            .unwrap();
        store
            .create_commit(&Commit::new_with_sha(&feature.id, "abc123"))
            .unwrap();
        let second = Branch::new(&alpha.id, "second", "main");
        store.create_branch(&second).unwrap();
        let other = Branch::new(&beta.id, "other", "main");
        store.create_branch(&other).unwrap();

        let listed = list_branches_by_project_impl(&store).unwrap();

        assert_eq!(
            listed.keys().cloned().collect::<Vec<_>>(),
            {
                let mut ids = vec![alpha.id.clone(), beta.id.clone(), empty.id.clone()];
                ids.sort();
                ids
            },
            "every project is listed, the one with no branches included"
        );
        assert!(listed[&empty.id].is_empty());
        // The worktree path and the commit count come through, per branch.
        let by_id = |id: &str| {
            listed[&alpha.id]
                .iter()
                .find(|branch| branch.id == id)
                .expect("the branch is listed under its project")
        };
        assert_eq!(listed[&alpha.id].len(), 2);
        assert_eq!(
            by_id(&feature.id).worktree_path.as_deref(),
            Some("/tmp/wt/feature")
        );
        assert_eq!(by_id(&feature.id).commit_count, Some(1));
        assert_eq!(by_id(&second.id).worktree_path, None);
        assert_eq!(by_id(&second.id).commit_count, Some(0));
        assert_eq!(listed[&beta.id].len(), 1);
        assert_eq!(listed[&beta.id][0].id, other.id);
        // Each list is exactly what the per-project command returns.
        for project in [&alpha, &beta, &empty] {
            assert_eq!(
                json(&listed[&project.id]),
                json(&list_branches_for_project_impl(&store, &project.id).unwrap()),
            );
        }
    }

    #[test]
    fn repos_are_keyed_by_project_and_match_the_per_project_list() {
        let store = Store::in_memory().unwrap();
        let alpha = project(&store, "alpha");
        let beta = project(&store, "beta");
        let empty = project(&store, "empty");

        let secondary =
            ProjectRepo::new(&alpha.id, "acme/tools", "feature", Some("cli".to_string()));
        store.create_project_repo(&secondary).unwrap();
        let primary = ProjectRepo::new(&alpha.id, "acme/storefront", "feature", None).primary();
        store.create_project_repo(&primary).unwrap();
        let other = ProjectRepo::new(&beta.id, "acme/ledger", "other", None).primary();
        store.create_project_repo(&other).unwrap();

        let listed = list_project_repos_by_project_impl(&store).unwrap();

        assert_eq!(listed.len(), 3);
        assert!(listed[&empty.id].is_empty());
        assert_eq!(
            listed[&alpha.id]
                .iter()
                .map(|repo| repo.id.as_str())
                .collect::<Vec<_>>(),
            vec![primary.id.as_str(), secondary.id.as_str()],
            "primary first, as the per-project list orders them"
        );
        for project in [&alpha, &beta, &empty] {
            assert_eq!(
                json(&listed[&project.id]),
                json(&store.list_project_repos(&project.id).unwrap()),
            );
        }
    }

    #[test]
    fn an_empty_store_lists_no_projects() {
        let store = Store::in_memory().unwrap();

        assert!(list_branches_by_project_impl(&store).unwrap().is_empty());
        assert!(list_project_repos_by_project_impl(&store)
            .unwrap()
            .is_empty());
    }

    /// A project deleted between two all-projects reads leaves the second
    /// one: the frontend relies on absence meaning "not in the store".
    #[test]
    fn a_deleted_project_is_no_longer_listed() {
        let store = Store::in_memory().unwrap();
        let kept = project(&store, "kept");
        let doomed = project(&store, "doomed");
        store
            .create_branch(&Branch::new(&doomed.id, "feature", "main"))
            .unwrap();
        assert_eq!(list_branches_by_project_impl(&store).unwrap().len(), 2);

        store.delete_project(&doomed.id).unwrap();

        let branches = list_branches_by_project_impl(&store).unwrap();
        let repos = list_project_repos_by_project_impl(&store).unwrap();
        assert_eq!(branches.keys().collect::<Vec<_>>(), vec![&kept.id]);
        assert_eq!(repos.keys().collect::<Vec<_>>(), vec![&kept.id]);
    }

    /// One project's failed read leaves that project out, and no other: the
    /// frontend reads an absent project on its own, so the failure costs one
    /// project its lists, not all of them.
    #[test]
    fn a_project_whose_read_fails_is_left_out_and_the_rest_are_listed() {
        let ids = ["alpha", "broken", "gamma"].map(String::from);

        let listed = lists_by_project(ids, "branches", |project_id| {
            if project_id == "broken" {
                Err("database disk image is malformed".to_string())
            } else {
                Ok(vec![format!("{project_id}-branch")])
            }
        });

        assert_eq!(
            listed.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["alpha", "gamma"]
        );
        assert_eq!(listed["alpha"], vec!["alpha-branch".to_string()]);
        assert_eq!(listed["gamma"], vec!["gamma-branch".to_string()]);
    }

    /// Every read failing is still a reply, an empty one: each project is
    /// then read on its own and reports its own error.
    #[test]
    fn every_read_failing_lists_no_project() {
        let ids = ["alpha", "beta"].map(String::from);

        let listed = lists_by_project(ids, "repos", |_| {
            Err::<Vec<String>, _>("locked".to_string())
        });

        assert!(listed.is_empty());
    }

    /// The same through the store: a branch row that cannot be decoded fails
    /// its own project's read, as the per-project command reports, and the
    /// all-projects list carries every other project.
    #[test]
    fn a_project_with_an_unreadable_branch_is_left_out_of_the_branch_list() {
        let store = Store::in_memory().unwrap();
        let healthy = project(&store, "healthy");
        let broken = project(&store, "broken");
        let kept = Branch::new(&healthy.id, "feature", "main");
        store.create_branch(&kept).unwrap();
        let unreadable = Branch::new(&broken.id, "feature", "main");
        store.create_branch(&unreadable).unwrap();
        store
            .hold_connection()
            .execute(
                "UPDATE branches SET pr_number = 'not a number' WHERE id = ?1",
                [&unreadable.id],
            )
            .unwrap();
        assert!(list_branches_for_project_impl(&store, &broken.id).is_err());

        let branches = list_branches_by_project_impl(&store).unwrap();
        let repos = list_project_repos_by_project_impl(&store).unwrap();

        assert_eq!(branches.keys().collect::<Vec<_>>(), vec![&healthy.id]);
        assert_eq!(branches[&healthy.id][0].id, kept.id);
        // Its repos read fine, so the repo list still carries it.
        assert_eq!(repos.len(), 2);
        assert!(repos.contains_key(&broken.id));
    }

    /// A store that cannot list its projects fails the reply: there is no
    /// project to key a partial one by.
    #[test]
    fn a_store_that_cannot_list_projects_fails_the_reply() {
        let store = Store::in_memory().unwrap();
        project(&store, "alpha");
        store
            .hold_connection()
            .execute_batch("ALTER TABLE projects RENAME TO projects_moved")
            .unwrap();

        assert!(list_branches_by_project_impl(&store).is_err());
        assert!(list_project_repos_by_project_impl(&store).is_err());
    }
}
