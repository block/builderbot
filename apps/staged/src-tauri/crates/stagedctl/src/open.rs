//! `stagedctl project open`: ask a Staged window to show a project, and
//! word what the window did.
//!
//! The app's `open_project` command (`src-tauri/src/project_open/control.rs`)
//! picks the window a Dock click would raise, raises it, asks it to switch,
//! and answers `{ "outcome", "window" }` whatever the window did. A window
//! that refused is a result on the wire, not an error, because the control
//! server's plain errors carry no code. This module is where each outcome
//! becomes an exit code and a `code: message` line.
//!
//! The project is resolved here, before the call, so a name that matches
//! nothing never raises a window. It comes from the `<project>` argument or,
//! without one, from the current directory: Staged keeps a project's
//! worktrees under `~/.staged/workspaces/<local|remote>/projects/<id>/`, so
//! the id is a segment of the path ([`workspace_project_id`]). Either way
//! the project is looked up in the app's list first, and only an id from
//! that list crosses the socket, never a path.
//!
//! The older worktree layout, `workspaces/local/<repo>/<branch>/`, names no
//! project in its path and is not resolved; a directory there reads
//! `not_in_workspace`.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{Client, CtlError};
use crate::commands::{find_project, list_projects, parse, CmdOutput, Project};

/// The dispatch command behind the verb. A Staged from before it answers
/// 404, as for any unknown command, and the transport appends an "update
/// Staged" hint to exactly this name.
pub const OPEN_PROJECT_COMMAND: &str = "open_project";

/// What the window did with the request, as `open_project` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Outcome {
    /// The project is on screen: switched to, or already showing.
    Opened,
    /// A dialog is open in the window; switching would discard it.
    DialogOpen,
    /// The diff comment being written could not be saved first.
    DiffCommentUnsaved,
    /// The window does not have the project (a delete is in flight).
    ProjectNotFound,
    /// A newer request for the same window replaced this one.
    Superseded,
    /// The window did not answer within the app's wait.
    TimedOut,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenAnswer {
    outcome: Outcome,
    /// The label of the window that was asked and raised.
    window: String,
}

/// What `project open` was pointed at. Decided without the app, so a
/// directory that names no project fails before Staged is looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The `<project>` argument: a name or an id, matched by the usual
    /// rules.
    Named(String),
    /// No argument: the project id in the current directory's path.
    Directory {
        /// The id segment as it appears on disk. Not yet known to be a
        /// project; [`project_open`] confirms it with the app.
        project_id: String,
        /// The directory the id was read from, for error messages.
        dir: PathBuf,
    },
}

/// The project id in `path`, when `path` is a project's workspace directory
/// or anything beneath it: `<staged_home>/workspaces/<local|remote>/
/// projects/<id>/…`. `staged_home` is the app's data directory, `~/.staged`.
///
/// Purely lexical: nothing is read from disk, so a symlink on either side
/// is the caller's to resolve ([`directory_project_id`]). A path that
/// climbs with `..` after the Staged home is not matched, since where it
/// lands cannot be told from its text.
///
/// The segment is returned as written. Whether a project has that id is
/// for the app to say.
pub(crate) fn workspace_project_id<'a>(path: &'a Path, staged_home: &Path) -> Option<&'a str> {
    let rest = path.strip_prefix(staged_home).ok()?;
    if rest
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let mut segments = rest.components().map(Component::as_os_str);
    if segments.next()? != "workspaces" {
        return None;
    }
    let location = segments.next()?;
    if location != "local" && location != "remote" {
        return None;
    }
    if segments.next()? != "projects" {
        return None;
    }
    segments.next()?.to_str()
}

/// [`workspace_project_id`] for a directory on disk. The directory is
/// usually the physical path the OS reports for the current directory,
/// while the Staged home comes from `$HOME`, which may reach the same place
/// through a symlink (a linked home, or `~/.staged` kept on another
/// volume). So when the paths do not match as written, both are resolved
/// and compared again.
fn directory_project_id(dir: &Path, staged_home: &Path) -> Option<String> {
    if let Some(id) = workspace_project_id(dir, staged_home) {
        return Some(id.to_owned());
    }
    let dir = dir.canonicalize().ok()?;
    let staged_home = staged_home.canonicalize().ok()?;
    workspace_project_id(&dir, &staged_home).map(str::to_owned)
}

/// Decide the [`Target`] from the optional `<project>` argument. Without
/// one, `current_dir` is asked for the directory and its project id is read
/// from the path under `staged_home`.
///
/// Needs no app. A directory outside every project workspace, or one that
/// cannot be determined, is `not_in_workspace` (exit 1).
pub fn target(
    project: Option<&str>,
    current_dir: impl FnOnce() -> std::io::Result<PathBuf>,
    staged_home: Option<&Path>,
) -> Result<Target, CtlError> {
    if let Some(needle) = project {
        return Ok(Target::Named(needle.to_owned()));
    }
    let dir = current_dir().map_err(|e| {
        CtlError::command(
            "not_in_workspace",
            format!(
                "cannot determine the current directory ({e}); pass a project name or id (see \
                 `stagedctl project list`)"
            ),
        )
    })?;
    let staged_home = staged_home
        .ok_or_else(|| CtlError::Environment("cannot determine the home directory".into()))?;
    match directory_project_id(&dir, staged_home) {
        Some(project_id) => Ok(Target::Directory { project_id, dir }),
        None => Err(CtlError::command(
            "not_in_workspace",
            format!(
                "{} is not inside a Staged project workspace; pass a project name or id (see \
                 `stagedctl project list`)",
                dir.display()
            ),
        )),
    }
}

/// The project whose workspace `dir` is in, from the app's list. Matched on
/// the id alone: the segment is an id by construction, so a project that
/// merely has it as a name is not the one.
fn project_for_directory<'a>(
    projects: &'a [Project],
    project_id: &str,
    dir: &Path,
) -> Result<&'a Project, CtlError> {
    projects.iter().find(|p| p.id == project_id).ok_or_else(|| {
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        let dir = dir.display();
        CtlError::command(
            "not_found",
            if names.is_empty() {
                format!(
                    "{dir} is in the workspace of project id {project_id}, which Staged \
                         does not have; the project was probably deleted, and the app lists no \
                         projects"
                )
            } else {
                format!(
                    "{dir} is in the workspace of project id {project_id}, which Staged \
                         does not have; the project was probably deleted. Projects: {}",
                    names.join(", ")
                )
            },
        )
    })
}

/// `project open [<project>]`: find the project in the app's list, then ask
/// the app to show it.
pub fn project_open(client: &Client, target: &Target) -> Result<CmdOutput, CtlError> {
    open_target(
        target,
        client.pid,
        || list_projects(client),
        |args| client.invoke(OPEN_PROJECT_COMMAND, args),
    )
}

/// Resolve `target` against the app's project list, then send the request.
/// `list` is the `list_projects` call and `invoke` the `open_project` one,
/// both passed in so the order is testable without an app: a target that
/// matches no project fails after `list` and never reaches `invoke`, which
/// is what raises a window.
fn open_target(
    target: &Target,
    pid: u32,
    list: impl FnOnce() -> Result<Vec<Project>, CtlError>,
    invoke: impl FnOnce(Value) -> Result<Value, CtlError>,
) -> Result<CmdOutput, CtlError> {
    let projects = list()?;
    let project = match target {
        Target::Named(needle) => find_project(&projects, needle)?,
        Target::Directory { project_id, dir } => project_for_directory(&projects, project_id, dir)?,
    };
    open_resolved(project, pid, invoke)
}

/// Send the request for an already-resolved project and map the window's
/// answer. `invoke` is the `open_project` call, passed in so the mapping is
/// testable without an app; `pid` is the instance being asked.
fn open_resolved(
    project: &Project,
    pid: u32,
    invoke: impl FnOnce(Value) -> Result<Value, CtlError>,
) -> Result<CmdOutput, CtlError> {
    // Only the id crosses the socket; the app looks the project up itself.
    let raw = invoke(json!({ "projectId": project.id }))?;
    let answer: OpenAnswer = parse(&raw, "open_project answer")?;
    let name = &project.name;
    match answer.outcome {
        Outcome::Opened => Ok(CmdOutput {
            json: json!({
                "outcome": "opened",
                "project": { "id": project.id, "name": name },
                "window": answer.window,
                "pid": pid,
            }),
            human: format!("opened {name}  id:{}  pid:{pid}", project.id),
        }),
        Outcome::DialogOpen => Err(CtlError::command(
            "ui_busy",
            "Staged has a dialog open; close it and run the command again",
        )),
        Outcome::DiffCommentUnsaved => Err(CtlError::command(
            "ui_busy",
            "Staged could not save the diff comment being written; save or discard it in the \
             window and run the command again",
        )),
        // Not a dialog in the way, but the same class of failure: the
        // window was taken up with something else, so nothing was shown
        // for this caller, and running the command again is the remedy.
        Outcome::Superseded => Err(CtlError::command(
            "ui_busy",
            format!(
                "another `stagedctl project open` reached the same Staged window and replaced \
                 this one before the window acted on it, so {name:?} was not shown; run the \
                 command again if it still should be"
            ),
        )),
        Outcome::ProjectNotFound => Err(CtlError::command(
            "not_found",
            format!(
                "the Staged window no longer has project {name:?} (id {}); it was probably \
                 just deleted (see `stagedctl project list`)",
                project.id
            ),
        )),
        Outcome::TimedOut => Err(CtlError::command(
            "ui_timeout",
            "the Staged window was brought to the front but did not answer in time, so the \
             project it shows may not have changed; run the command again",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            github_repo: None,
            subpath: None,
            location: None,
        }
    }

    fn widgets() -> Project {
        project("p1", "Widgets")
    }

    /// A project id as the app mints them.
    const ID: &str = "7b23e070-efd3-4568-8671-bd532a5bbf9d";

    /// The Staged home the lexical tests use. Nothing is read from disk, so
    /// it need not exist.
    fn home() -> PathBuf {
        PathBuf::from("/Users/me/.staged")
    }

    fn id_at(path: &str) -> Option<String> {
        workspace_project_id(Path::new(path), &home()).map(str::to_owned)
    }

    /// An `open_project` call that must not happen.
    fn no_open(_: Value) -> Result<Value, CtlError> {
        panic!("open_project was called, which raises a window")
    }

    fn opened(_: Value) -> Result<Value, CtlError> {
        Ok(json!({ "outcome": "opened", "window": "main" }))
    }

    // -------------------------------------------------------------------
    // Path to project id
    // -------------------------------------------------------------------

    #[test]
    fn a_project_workspace_and_everything_beneath_it_names_the_project() {
        for path in [
            // The project's own directory, with and without a trailing slash.
            "/Users/me/.staged/workspaces/local/projects/7b23e070-efd3-4568-8671-bd532a5bbf9d",
            "/Users/me/.staged/workspaces/local/projects/7b23e070-efd3-4568-8671-bd532a5bbf9d/",
            // A worktree root.
            "/Users/me/.staged/workspaces/local/projects/7b23e070-efd3-4568-8671-bd532a5bbf9d/block-builderbot--my-branch",
            // Nested directories inside a worktree, however deep.
            "/Users/me/.staged/workspaces/local/projects/7b23e070-efd3-4568-8671-bd532a5bbf9d/block-builderbot--my-branch/apps/staged",
            "/Users/me/.staged/workspaces/local/projects/7b23e070-efd3-4568-8671-bd532a5bbf9d/block-builderbot--my-branch/apps/staged/src-tauri/crates/stagedctl/src",
            // A directory that happens to repeat the layout further down
            // still belongs to the outer project.
            "/Users/me/.staged/workspaces/local/projects/7b23e070-efd3-4568-8671-bd532a5bbf9d/wt/workspaces/local/projects/other",
        ] {
            assert_eq!(id_at(path).as_deref(), Some(ID), "{path}");
        }
    }

    #[test]
    fn local_and_remote_workspaces_both_resolve() {
        assert_eq!(
            id_at("/Users/me/.staged/workspaces/local/projects/p1/wt").as_deref(),
            Some("p1")
        );
        assert_eq!(
            id_at("/Users/me/.staged/workspaces/remote/projects/p2").as_deref(),
            Some("p2")
        );
        assert_eq!(
            id_at("/Users/me/.staged/workspaces/cloud/projects/p3"),
            None
        );
    }

    #[test]
    fn a_trailing_slash_on_the_staged_home_changes_nothing() {
        let path = Path::new("/Users/me/.staged/workspaces/local/projects/p1/wt");
        assert_eq!(
            workspace_project_id(path, Path::new("/Users/me/.staged/")),
            Some("p1")
        );
    }

    #[test]
    fn paths_outside_a_project_workspace_name_no_project() {
        for path in [
            // Nowhere near Staged.
            "/Users/me/code/builderbot",
            "/",
            // The Staged home and the levels above a project id.
            "/Users/me/.staged",
            "/Users/me/.staged/workspaces",
            "/Users/me/.staged/workspaces/local",
            "/Users/me/.staged/workspaces/local/projects",
            "/Users/me/.staged/workspaces/local/projects/",
            // Other things Staged keeps in its home.
            "/Users/me/.staged/repos/block/builderbot",
            "/Users/me/.staged/control/4242/sessions",
            "/Users/me/.staged/projects/p1",
            // The right segments in the wrong place.
            "/Users/me/.staged/local/projects/p1",
            "/Users/me/.staged/workspaces/projects/p1",
            "/Users/me/.staged/workspaces/local/local/projects/p1",
            // Segment names are matched exactly.
            "/Users/me/.staged/Workspaces/local/projects/p1",
            "/Users/me/.staged/workspaces/local/project/p1",
            // A sibling whose name merely starts with the home's.
            "/Users/me/.staged-backup/workspaces/local/projects/p1",
            // Someone else's Staged home.
            "/Users/you/.staged/workspaces/local/projects/p1",
            // Relative paths are not placed anywhere.
            "workspaces/local/projects/p1",
            ".staged/workspaces/local/projects/p1",
        ] {
            assert_eq!(id_at(path), None, "{path}");
        }
    }

    #[test]
    fn the_older_worktree_layout_names_no_project() {
        // `workspaces/local/<repo-name>/<branch>/` carries no id; resolving
        // it is deferred, so it must not be mistaken for the new layout,
        // even for a repo that is itself called `projects`.
        assert_eq!(
            id_at("/Users/me/.staged/workspaces/local/builderbot/my-branch"),
            None
        );
        assert_eq!(
            id_at("/Users/me/.staged/workspaces/local/builderbot/my-branch/apps/staged"),
            None
        );
        // Not told apart from the new layout by its text: the segment is
        // returned, and the app's project list is what rejects it.
        assert_eq!(
            id_at("/Users/me/.staged/workspaces/local/projects/my-branch/src").as_deref(),
            Some("my-branch")
        );
    }

    #[test]
    fn a_path_that_climbs_with_dot_dot_is_not_matched_lexically() {
        // Really `/Users/me/.staged/repos`; its text must not name `p1`.
        assert_eq!(
            id_at("/Users/me/.staged/workspaces/local/projects/p1/../../../../repos"),
            None
        );
        assert_eq!(
            id_at("/Users/me/.staged/workspaces/local/projects/.."),
            None
        );
        // `.` segments change nothing.
        assert_eq!(
            id_at("/Users/me/.staged/./workspaces/local/./projects/p1/.").as_deref(),
            Some("p1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_id_segment_that_is_not_utf8_names_no_project() {
        use std::os::unix::ffi::OsStrExt;
        let path = home()
            .join("workspaces/local/projects")
            .join(std::ffi::OsStr::from_bytes(b"p\xff1"))
            .join("wt");
        assert_eq!(workspace_project_id(&path, &home()), None);
    }

    // -------------------------------------------------------------------
    // Symlinks between the directory and the Staged home
    // -------------------------------------------------------------------

    /// A temp root as the OS reports it (macOS reaches its temp directory
    /// through a symlink), so only the links a test makes are in play.
    #[cfg(unix)]
    fn real_tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().canonicalize().unwrap();
        (dir, real)
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_home_still_resolves() {
        // `$HOME` is a link (`/home/me` -> `/data/home/me`), while the OS
        // reports the current directory by its physical path.
        let (_guard, root) = real_tempdir();
        let worktree = root.join("data/me/.staged/workspaces/local/projects/p1/repo--branch/src");
        std::fs::create_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink(root.join("data/me"), root.join("home")).unwrap();
        let staged_home = root.join("home/.staged");

        assert_eq!(workspace_project_id(&worktree, &staged_home), None);
        assert_eq!(
            directory_project_id(&worktree, &staged_home).as_deref(),
            Some("p1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_staged_home_still_resolves() {
        // `~/.staged` itself kept on another volume.
        let (_guard, root) = real_tempdir();
        let worktree = root.join("volume/staged/workspaces/remote/projects/p2");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::os::unix::fs::symlink(root.join("volume/staged"), root.join("home/.staged")).unwrap();

        assert_eq!(
            directory_project_id(&worktree, &root.join("home/.staged")).as_deref(),
            Some("p2")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_reached_through_a_symlink_resolves_to_where_it_is() {
        let (_guard, root) = real_tempdir();
        let staged_home = root.join("home/.staged");
        let worktree = staged_home.join("workspaces/local/projects/p1/repo--branch");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(root.join("code")).unwrap();
        std::os::unix::fs::symlink(&worktree, root.join("code/link")).unwrap();

        assert_eq!(
            directory_project_id(&root.join("code/link"), &staged_home).as_deref(),
            Some("p1")
        );
        // A `..` is followed on disk rather than read as text.
        std::fs::create_dir_all(staged_home.join("repos")).unwrap();
        assert_eq!(
            directory_project_id(&worktree.join("../../../../../repos"), &staged_home),
            None
        );
        assert_eq!(
            directory_project_id(&worktree.join("../../p1/repo--branch"), &staged_home).as_deref(),
            Some("p1")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolving_symlinks_does_not_invent_a_workspace() {
        let (_guard, root) = real_tempdir();
        let staged_home = root.join("home/.staged");
        std::fs::create_dir_all(staged_home.join("workspaces/local/projects")).unwrap();
        std::fs::create_dir_all(root.join("code/app")).unwrap();

        assert_eq!(
            directory_project_id(&root.join("code/app"), &staged_home),
            None
        );
        assert_eq!(
            directory_project_id(&staged_home.join("workspaces/local/projects"), &staged_home),
            None
        );
        // A Staged home that does not exist contains nothing.
        assert_eq!(
            directory_project_id(&root.join("code/app"), &root.join("nobody/.staged")),
            None
        );
    }

    // -------------------------------------------------------------------
    // Choosing the target
    // -------------------------------------------------------------------

    #[test]
    fn a_project_argument_is_the_target_and_the_directory_is_never_read() {
        let target = target(
            Some("Widgets"),
            || panic!("the current directory is not needed"),
            None,
        )
        .unwrap();
        assert_eq!(target, Target::Named("Widgets".to_string()));
    }

    #[test]
    fn no_argument_takes_the_project_from_the_current_directory() {
        let dir = home()
            .join("workspaces/local/projects")
            .join(ID)
            .join("wt/src");
        let target = target(None, || Ok(dir.clone()), Some(&home())).unwrap();
        assert_eq!(
            target,
            Target::Directory {
                project_id: ID.to_string(),
                dir,
            }
        );
    }

    #[test]
    fn a_directory_outside_every_workspace_is_not_in_workspace() {
        let err = target(None, || Ok(PathBuf::from("/Users/me/code")), Some(&home())).unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(
            err.to_string(),
            "not_in_workspace: /Users/me/code is not inside a Staged project workspace; pass a \
             project name or id (see `stagedctl project list`)"
        );
        // So is a worktree in the older layout.
        let old = home().join("workspaces/local/builderbot/my-branch");
        let err = target(None, || Ok(old), Some(&home())).unwrap_err();
        assert_eq!(err.code(), "not_in_workspace");
    }

    #[test]
    fn a_current_directory_that_cannot_be_read_is_not_in_workspace() {
        let err = target(
            None,
            || Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
            Some(&home()),
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(err.code(), "not_in_workspace");
        assert!(
            err.to_string()
                .starts_with("not_in_workspace: cannot determine the current directory ("),
            "{err}"
        );
        assert!(
            err.to_string()
                .ends_with("pass a project name or id (see `stagedctl project list`)"),
            "{err}"
        );
    }

    #[test]
    fn no_argument_and_no_home_directory_is_an_environment_error() {
        let err = target(None, || Ok(PathBuf::from("/Users/me/code")), None).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert_eq!(err.to_string(), "cannot determine the home directory");
    }

    // -------------------------------------------------------------------
    // Confirming the target with the app
    // -------------------------------------------------------------------

    fn directory(project_id: &str) -> Target {
        Target::Directory {
            project_id: project_id.to_string(),
            dir: home()
                .join("workspaces/local/projects")
                .join(project_id)
                .join("wt"),
        }
    }

    #[test]
    fn a_directory_target_opens_the_project_with_that_id() {
        let projects = vec![project("p0", "Gadgets"), widgets()];
        let mut sent = None;
        let output = open_target(
            &directory("p1"),
            4242,
            || Ok(projects),
            |args| {
                sent = Some(args);
                opened(Value::Null)
            },
        )
        .unwrap();
        assert_eq!(sent, Some(json!({ "projectId": "p1" })));
        // The same output as when the project was named.
        assert_eq!(output.human, "opened Widgets  id:p1  pid:4242");
    }

    #[test]
    fn a_directory_whose_project_the_app_does_not_have_is_not_found_and_raises_nothing() {
        let projects = vec![project("p0", "Gadgets"), widgets()];
        let err = open_target(&directory("gone"), 4242, || Ok(projects), no_open).unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(
            err.to_string(),
            "not_found: /Users/me/.staged/workspaces/local/projects/gone/wt is in the workspace \
             of project id gone, which Staged does not have; the project was probably deleted. \
             Projects: Gadgets, Widgets"
        );

        let err = open_target(&directory("gone"), 4242, || Ok(vec![]), no_open).unwrap_err();
        assert_eq!(err.code(), "not_found");
        assert!(
            err.to_string().ends_with("and the app lists no projects"),
            "{err}"
        );
    }

    #[test]
    fn a_directory_id_is_never_matched_against_project_names() {
        // A directory called `Widgets` under `projects/` is not project p1,
        // whatever case it is written in.
        for segment in ["Widgets", "widgets"] {
            let err = open_target(&directory(segment), 4242, || Ok(vec![widgets()]), no_open)
                .unwrap_err();
            assert_eq!(err.code(), "not_found", "{segment}");
        }
    }

    #[test]
    fn a_named_target_keeps_the_name_rules_and_fails_before_any_window() {
        let projects = || {
            Ok(vec![
                widgets(),
                project("p2", "widgets"),
                project("p3", "Gears"),
            ])
        };
        for needle in ["p3", "Gears", "gears"] {
            let mut sent = None;
            open_target(&Target::Named(needle.to_string()), 4242, projects, |args| {
                sent = Some(args);
                opened(Value::Null)
            })
            .unwrap();
            assert_eq!(sent, Some(json!({ "projectId": "p3" })), "{needle}");
        }
        let err =
            open_target(&Target::Named("nope".to_string()), 4242, projects, no_open).unwrap_err();
        assert_eq!((err.exit_code(), err.code()), (1, "not_found"));
        let err = open_target(
            &Target::Named("WIDGETS".to_string()),
            4242,
            projects,
            no_open,
        )
        .unwrap_err();
        assert_eq!((err.exit_code(), err.code()), (1, "ambiguous"));
    }

    #[test]
    fn a_project_list_that_fails_is_the_error_and_raises_nothing() {
        for target in [Target::Named("Widgets".to_string()), directory("p1")] {
            let err = open_target(
                &target,
                4242,
                || {
                    Err(CtlError::Authorization {
                        code: "authorization_required".to_string(),
                        message: "run `stagedctl connect --client <name>` once, then retry".into(),
                    })
                },
                no_open,
            )
            .unwrap_err();
            assert_eq!((err.exit_code(), err.code()), (4, "authorization_required"));
        }
    }

    // -------------------------------------------------------------------
    // Wording the window's answer
    // -------------------------------------------------------------------

    /// `open_resolved` against an app that answers `outcome` from `main`.
    fn open_with(outcome: &str) -> Result<CmdOutput, CtlError> {
        let answer = json!({ "outcome": outcome, "window": "main" });
        open_resolved(&widgets(), 4242, move |_| Ok(answer))
    }

    /// The exit code and stderr line of a refused open.
    fn refusal(outcome: &str) -> (i32, String) {
        let err = open_with(outcome).expect_err("the outcome is a failure");
        (err.exit_code(), err.to_string())
    }

    #[test]
    fn the_request_carries_the_project_id_and_nothing_else() {
        let mut sent = None;
        open_resolved(&widgets(), 4242, |args| {
            sent = Some(args);
            Ok(json!({ "outcome": "opened", "window": "main" }))
        })
        .unwrap();
        assert_eq!(sent, Some(json!({ "projectId": "p1" })));
    }

    #[test]
    fn opened_prints_the_name_the_id_and_the_instance_pid() {
        let output = open_with("opened").unwrap();
        assert_eq!(output.human, "opened Widgets  id:p1  pid:4242");
        assert_eq!(
            output.json,
            json!({
                "outcome": "opened",
                "project": { "id": "p1", "name": "Widgets" },
                "window": "main",
                "pid": 4242,
            })
        );
    }

    #[test]
    fn a_dialog_or_an_unsaved_diff_comment_is_ui_busy() {
        assert_eq!(
            refusal("dialogOpen"),
            (
                1,
                "ui_busy: Staged has a dialog open; close it and run the command again".to_string()
            )
        );
        assert_eq!(
            refusal("diffCommentUnsaved"),
            (
                1,
                "ui_busy: Staged could not save the diff comment being written; save or \
                 discard it in the window and run the command again"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_superseded_request_is_ui_busy_with_its_own_message() {
        let (exit, line) = refusal("superseded");
        assert_eq!(exit, 1);
        assert_eq!(
            line,
            "ui_busy: another `stagedctl project open` reached the same Staged window and \
             replaced this one before the window acted on it, so \"Widgets\" was not shown; \
             run the command again if it still should be"
        );
        assert_ne!(line, refusal("dialogOpen").1);
    }

    #[test]
    fn a_project_the_window_lost_is_not_found() {
        assert_eq!(
            refusal("projectNotFound"),
            (
                1,
                "not_found: the Staged window no longer has project \"Widgets\" (id p1); it \
                 was probably just deleted (see `stagedctl project list`)"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_window_that_never_answered_is_ui_timeout() {
        assert_eq!(
            refusal("timedOut"),
            (
                1,
                "ui_timeout: the Staged window was brought to the front but did not answer \
                 in time, so the project it shows may not have changed; run the command again"
                    .to_string()
            )
        );
    }

    #[test]
    fn every_outcome_the_app_names_is_understood() {
        // The names `project_open::control::outcome_name` returns app-side.
        for (name, outcome) in [
            ("opened", Outcome::Opened),
            ("dialogOpen", Outcome::DialogOpen),
            ("diffCommentUnsaved", Outcome::DiffCommentUnsaved),
            ("projectNotFound", Outcome::ProjectNotFound),
            ("superseded", Outcome::Superseded),
            ("timedOut", Outcome::TimedOut),
        ] {
            let answer: OpenAnswer =
                serde_json::from_value(json!({ "outcome": name, "window": "w2" })).unwrap();
            assert_eq!(answer.outcome, outcome, "{name}");
            assert_eq!(answer.window, "w2");
        }
    }

    #[test]
    fn an_outcome_this_cli_does_not_know_is_drift_not_success() {
        let err = open_with("minimized").unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.to_string()
                .contains("unexpected open_project answer shape"),
            "{err}"
        );
        // Nor is an answer without an outcome.
        let err = open_resolved(&widgets(), 4242, |_| Ok(json!({ "window": "main" }))).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn the_apps_own_refusals_pass_through_untouched() {
        // Scope and credential refusals are exit 4 with the app's code.
        let err = open_resolved(&widgets(), 4242, |_| {
            Err(CtlError::Authorization {
                code: "scope_denied".to_string(),
                message: "open_project changes the app and this credential is read-only".into(),
            })
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 4);
        assert_eq!(
            err.to_string(),
            "scope_denied: open_project changes the app and this credential is read-only"
        );
        // A request the app could not make at all stays the app's error.
        let err = open_resolved(&widgets(), 4242, |_| {
            Err(CtlError::command(
                "app_error",
                "Staged has no window to show the project in; restart Staged and try again",
            ))
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(err.code(), "app_error");
    }
}
