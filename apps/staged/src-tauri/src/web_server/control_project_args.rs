//! The `create_project` and `add_project_repo` arguments the control surface
//! accepts.
//!
//! Both commands take the argument names the desktop forms send, so one
//! command has one wire shape. What differs is who filled them in. A form
//! sends values it already checked. A control caller sends what was typed,
//! and `project_commands::control` checks it. So a control caller may not
//! send the values those checks produce, and gets one argument the forms do
//! not have:
//!
//! - `headRepo` is refused. The pull request decides where its head lives.
//! - `setAsPrimary` is refused for `add_project_repo`. The app has no way to
//!   change the primary repo back, and the control surface offers only what
//!   the user can undo.
//! - `location` must be `local` or `remote`. The forms' body reads anything
//!   else as local, which would hide a typo.
//! - `allowMonorepoRoot` accepts the root of a monorepo, which is refused
//!   otherwise.
//! - `defaultBranch` is the branch to start from, as it is for the forms.
//!   Left out, the pull request's base or the repository's default is used.
//!
//! A repo argument given to `create_project` without `githubRepo` is
//! refused too, where the forms' body would drop it unread.

use serde_json::Value;

use super::{arg, opt_arg};
use crate::project_commands::{ControlAddRepo, ControlCreateProject, RepoSpecRequest};

/// The arguments that describe a repository, besides `githubRepo` itself.
const REPO_ARGS: &[&str] = &[
    "subpath",
    "branchName",
    "prNumber",
    "defaultBranch",
    "allowMonorepoRoot",
];

/// A control caller's `create_project` arguments.
pub(super) fn create_project(args: &Value) -> Result<ControlCreateProject, String> {
    refuse_head_repo("create_project", args)?;
    let name: String = arg(args, "name")?;
    let remote = match opt_arg::<String>(args, "location")?.as_deref() {
        None | Some("local") => false,
        Some("remote") => true,
        Some(other) => {
            return Err(format!(
                "create_project takes a location of \"local\" or \"remote\" through the control \
                 surface (got {other:?})"
            ));
        }
    };
    let repo = match opt_arg::<String>(args, "githubRepo")? {
        Some(github_repo) => Some(repo_spec(github_repo, args)?),
        None => {
            if let Some(stray) = REPO_ARGS.iter().find(|key| is_given(args, key)) {
                return Err(format!(
                    "create_project was given {stray} without a githubRepo: it describes the \
                     repository to create the project with"
                ));
            }
            None
        }
    };
    Ok(ControlCreateProject { name, remote, repo })
}

/// A control caller's `add_project_repo` arguments: the project id, and
/// what to add to it.
pub(super) fn add_project_repo(args: &Value) -> Result<(String, ControlAddRepo), String> {
    refuse_head_repo("add_project_repo", args)?;
    if opt_arg::<bool>(args, "setAsPrimary")?.unwrap_or(false) {
        return Err(
            "add_project_repo with setAsPrimary=true is not available through the control \
             surface: the app has no way to change a project's primary repo back"
                .to_string(),
        );
    }
    let project_id: String = arg(args, "projectId")?;
    let repo = repo_spec(arg(args, "githubRepo")?, args)?;
    let reason: Option<String> = opt_arg(args, "reason")?;
    Ok((project_id, ControlAddRepo { repo, reason }))
}

fn repo_spec(github_repo: String, args: &Value) -> Result<RepoSpecRequest, String> {
    Ok(RepoSpecRequest {
        github_repo,
        subpath: opt_arg(args, "subpath")?,
        branch_name: opt_arg(args, "branchName")?,
        pr_number: opt_arg(args, "prNumber")?,
        base: opt_arg(args, "defaultBranch")?,
        allow_monorepo_root: opt_arg::<bool>(args, "allowMonorepoRoot")?.unwrap_or(false),
    })
}

fn refuse_head_repo(cmd: &str, args: &Value) -> Result<(), String> {
    if is_given(args, "headRepo") {
        return Err(format!(
            "{cmd} does not take a headRepo through the control surface: the pull request \
             (prNumber) decides which repository its head lives in"
        ));
    }
    Ok(())
}

/// Whether `key` is present with a value. An explicit `null` is not.
fn is_given(args: &Value, key: &str) -> bool {
    args.get(key).is_some_and(|value| !value.is_null())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_name_alone_is_a_local_project_without_a_repo() {
        let ask = create_project(&json!({ "name": "Fix login" })).unwrap();
        assert_eq!(ask.name, "Fix login");
        assert!(!ask.remote, "local is the default");
        assert!(ask.repo.is_none());
        // Nulls are what a client sends for flags that were not given.
        let ask = create_project(&json!({
            "name": "Fix login", "location": null, "githubRepo": null, "subpath": null,
            "branchName": null, "prNumber": null, "defaultBranch": null, "headRepo": null,
            "allowMonorepoRoot": null,
        }))
        .unwrap();
        assert!(!ask.remote);
        assert!(ask.repo.is_none());
    }

    #[test]
    fn create_requires_a_name() {
        let err = create_project(&json!({})).unwrap_err();
        assert!(err.contains("missing argument: name"), "{err}");
    }

    #[test]
    fn location_is_local_or_remote_and_nothing_else() {
        let local = create_project(&json!({ "name": "n", "location": "local" })).unwrap();
        assert!(!local.remote);
        let remote = create_project(&json!({ "name": "n", "location": "remote" })).unwrap();
        assert!(remote.remote);
        // The forms' body reads anything else as local. A typo must not
        // quietly become a local project.
        for bad in ["Remote", "remtoe", ""] {
            let err = create_project(&json!({ "name": "n", "location": bad })).unwrap_err();
            assert!(err.contains("\"local\" or \"remote\""), "{bad:?}: {err}");
        }
    }

    #[test]
    fn create_maps_the_repo_arguments_onto_the_resolver_request() {
        let ask = create_project(&json!({
            "name": "Fix login",
            "githubRepo": "Acme/Widgets",
            "subpath": "/services/auth/",
            "branchName": "fix-login",
            "prNumber": 42,
            "defaultBranch": "develop",
            "allowMonorepoRoot": true,
        }))
        .unwrap();
        // Passed on as typed: the resolver canonicalises and normalises.
        let repo = ask.repo.expect("a repo was given");
        assert_eq!(repo.github_repo, "Acme/Widgets");
        assert_eq!(repo.subpath.as_deref(), Some("/services/auth/"));
        assert_eq!(repo.branch_name.as_deref(), Some("fix-login"));
        assert_eq!(repo.pr_number, Some(42));
        assert_eq!(repo.base.as_deref(), Some("develop"));
        assert!(repo.allow_monorepo_root);
    }

    #[test]
    fn the_monorepo_override_is_off_unless_asked_for() {
        let ask = create_project(&json!({ "name": "n", "githubRepo": "acme/widgets" })).unwrap();
        assert!(!ask.repo.unwrap().allow_monorepo_root);
        let (_, ask) =
            add_project_repo(&json!({ "projectId": "p", "githubRepo": "acme/widgets" })).unwrap();
        assert!(!ask.repo.allow_monorepo_root);
        let (_, ask) = add_project_repo(&json!({
            "projectId": "p", "githubRepo": "acme/widgets", "allowMonorepoRoot": true
        }))
        .unwrap();
        assert!(ask.repo.allow_monorepo_root);
    }

    #[test]
    fn create_refuses_a_repo_argument_without_a_repo() {
        for (key, value) in [
            ("subpath", json!("services/auth")),
            ("branchName", json!("fix-login")),
            ("prNumber", json!(42)),
            ("defaultBranch", json!("develop")),
            ("allowMonorepoRoot", json!(true)),
        ] {
            let mut args = json!({ "name": "n" });
            args[key] = value;
            let err = create_project(&args).unwrap_err();
            assert!(
                err.contains(key) && err.contains("without a githubRepo"),
                "{key}: {err}"
            );
        }
        assert_eq!(
            REPO_ARGS.len(),
            5,
            "a new repo argument needs a row in this test"
        );
    }

    #[test]
    fn neither_command_takes_a_head_repo() {
        let err = create_project(&json!({
            "name": "n", "githubRepo": "acme/widgets", "headRepo": "fork/widgets"
        }))
        .unwrap_err();
        assert!(
            err.contains("create_project") && err.contains("headRepo"),
            "{err}"
        );
        let err = add_project_repo(&json!({
            "projectId": "p", "githubRepo": "acme/widgets", "headRepo": "fork/widgets"
        }))
        .unwrap_err();
        assert!(
            err.contains("add_project_repo") && err.contains("headRepo"),
            "{err}"
        );
    }

    #[test]
    fn add_requires_a_project_and_a_repo() {
        let err = add_project_repo(&json!({ "githubRepo": "acme/widgets" })).unwrap_err();
        assert!(err.contains("missing argument: projectId"), "{err}");
        let err = add_project_repo(&json!({ "projectId": "p" })).unwrap_err();
        assert!(err.contains("missing argument: githubRepo"), "{err}");
    }

    #[test]
    fn add_maps_its_arguments_and_passes_the_reason_through() {
        let (project_id, ask) = add_project_repo(&json!({
            "projectId": "p",
            "githubRepo": "acme/widgets",
            "subpath": "services/auth",
            "branchName": "fix-login",
            "prNumber": 42,
            "defaultBranch": "develop",
            "reason": "The login service lives here.",
        }))
        .unwrap();
        assert_eq!(project_id, "p");
        assert_eq!(ask.repo.github_repo, "acme/widgets");
        assert_eq!(ask.repo.subpath.as_deref(), Some("services/auth"));
        assert_eq!(ask.repo.branch_name.as_deref(), Some("fix-login"));
        assert_eq!(ask.repo.pr_number, Some(42));
        assert_eq!(ask.repo.base.as_deref(), Some("develop"));
        assert_eq!(ask.reason.as_deref(), Some("The login service lives here."));
    }

    #[test]
    fn add_refuses_to_choose_the_primary_repo() {
        let err = add_project_repo(&json!({
            "projectId": "p", "githubRepo": "acme/widgets", "setAsPrimary": true
        }))
        .unwrap_err();
        assert!(err.contains("setAsPrimary"), "{err}");
        // `false` asks for nothing.
        add_project_repo(&json!({
            "projectId": "p", "githubRepo": "acme/widgets", "setAsPrimary": false
        }))
        .unwrap();
    }

    #[test]
    fn a_mistyped_argument_is_an_error_not_a_default() {
        let err = create_project(&json!({
            "name": "n", "githubRepo": "acme/widgets", "prNumber": "forty-two"
        }))
        .unwrap_err();
        assert!(err.contains("invalid argument 'prNumber'"), "{err}");
        let err = add_project_repo(&json!({
            "projectId": "p", "githubRepo": "acme/widgets", "allowMonorepoRoot": "yes"
        }))
        .unwrap_err();
        assert!(
            err.contains("invalid argument 'allowMonorepoRoot'"),
            "{err}"
        );
    }
}
