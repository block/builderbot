//! The `project create` and `project add-repo` writes: flag pairing, the
//! request each sends, the re-read after it, and the rendering.
//!
//! Like [`crate::commands`], this is a flag→JSON mapper. The app checks the
//! repository, the project name and the creator's limit
//! (`project_commands::control` in the app), and its refusals pass through
//! verbatim. Two things are decided here: which flags go together, and
//! whether a repo and subpath is already attached, which is answered from a
//! read the caller can make anyway and names the branch it has.
//!
//! Each verb is one write followed by reads. A read that fails after the
//! write landed is a [`CtlError::Partial`], so the caller still learns the
//! project or repo exists.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::cli::ProjectLocation;
use crate::client::{Client, CtlError};
use crate::commands::{
    branch_summary, find_project, parse, raw_branches_of_repo, repo_address, BranchRow, CmdOutput,
    Project, RepoAddress, RepoRow,
};

/// The dispatch commands behind the two verbs. A Staged from before they
/// reached the control server refuses them as not allowlisted (404), which
/// on the wire is the answer to any unknown command, so the transport
/// appends an "update Staged" hint to exactly these.
pub const PROJECT_WRITE_COMMANDS: &[&str] = &["create_project", "add_project_repo"];

pub fn is_project_write_command(command: &str) -> bool {
    PROJECT_WRITE_COMMANDS.contains(&command)
}

fn invalid(message: impl Into<String>) -> CtlError {
    CtlError::command("invalid_arguments", message)
}

// =============================================================================
// The repo flags
// =============================================================================

/// The repo a verb was asked to attach: `--repo` with `--branch`, `--pr`,
/// `--base` and `--allow-monorepo-root`, once the pairing rules passed.
/// Values are kept as typed. The app canonicalises the name and normalises
/// the subpath.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRequest {
    pub address: RepoAddress,
    pub branch: Option<String>,
    pub pr: Option<u64>,
    pub base: Option<String>,
    pub allow_monorepo_root: bool,
}

impl RepoRequest {
    /// The flags of `project add-repo`, and of `project create` when it has
    /// a `--repo`. A pull request brings its own branch and base, so `--pr`
    /// goes with neither `--branch` nor `--base`.
    pub fn from_flags(
        repo: &str,
        branch: Option<&str>,
        pr: Option<u64>,
        base: Option<&str>,
        allow_monorepo_root: bool,
    ) -> Result<Self, CtlError> {
        let address = RepoAddress::parse(repo).map_err(|_| {
            invalid(format!(
                "--repo must look like <owner>/<repo>[:<subpath>], got {repo:?}"
            ))
        })?;
        if pr.is_some() && branch.is_some() {
            return Err(invalid(
                "--branch and --pr cannot be combined: a pull request brings its own branch",
            ));
        }
        if pr.is_some() && base.is_some() {
            return Err(invalid(
                "--base and --pr cannot be combined: a pull request brings its own base branch",
            ));
        }
        for (flag, value) in [("--branch", branch), ("--base", base)] {
            if value.is_some_and(|v| v.trim().is_empty()) {
                return Err(invalid(format!("{flag} must not be empty")));
            }
        }
        Ok(Self {
            address,
            branch: branch.map(str::to_string),
            pr,
            base: base.map(str::to_string),
            allow_monorepo_root,
        })
    }

    /// The flags of `project create`, where the repo is optional. Every
    /// other repo flag describes that repo, so each needs `--repo`.
    pub fn for_create(
        repo: Option<&str>,
        branch: Option<&str>,
        pr: Option<u64>,
        base: Option<&str>,
        allow_monorepo_root: bool,
    ) -> Result<Option<Self>, CtlError> {
        let Some(repo) = repo else {
            let stray = [
                ("--branch", branch.is_some()),
                ("--pr", pr.is_some()),
                ("--base", base.is_some()),
                ("--allow-monorepo-root", allow_monorepo_root),
            ]
            .into_iter()
            .find(|(_, given)| *given);
            return match stray {
                Some((flag, _)) => Err(invalid(format!(
                    "{flag} describes the project's first repo and needs --repo \
                     <owner>/<repo>[:<subpath>]; without --repo the project is created empty"
                ))),
                None => Ok(None),
            };
        };
        Self::from_flags(repo, branch, pr, base, allow_monorepo_root).map(Some)
    }

    /// Add this repo's arguments to a request, under the names the control
    /// surface takes. A flag that was not given is left out, not sent as
    /// `false` or `null`: the app reads a present repo argument as given.
    /// `headRepo` and `setAsPrimary` are never sent. The app refuses both
    /// from a control caller.
    fn insert_into(&self, args: &mut Map<String, Value>) {
        args.insert("githubRepo".into(), json!(self.address.github_repo));
        if let Some(subpath) = &self.address.subpath {
            args.insert("subpath".into(), json!(subpath));
        }
        if let Some(branch) = &self.branch {
            args.insert("branchName".into(), json!(branch));
        }
        if let Some(pr) = self.pr {
            args.insert("prNumber".into(), json!(pr));
        }
        if let Some(base) = &self.base {
            args.insert("defaultBranch".into(), json!(base));
        }
        if self.allow_monorepo_root {
            args.insert("allowMonorepoRoot".into(), json!(true));
        }
    }
}

/// The `create_project` request. `location` is always sent, so the app
/// never falls back to the location the New Project form remembers.
pub(crate) fn create_args(
    name: &str,
    location: ProjectLocation,
    repo: Option<&RepoRequest>,
) -> Value {
    let mut args = Map::new();
    args.insert("name".into(), json!(name.trim()));
    args.insert("location".into(), json!(location.as_str()));
    if let Some(repo) = repo {
        repo.insert_into(&mut args);
    }
    Value::Object(args)
}

/// The `add_project_repo` request.
pub(crate) fn add_repo_args(project_id: &str, repo: &RepoRequest, reason: Option<&str>) -> Value {
    let mut args = Map::new();
    args.insert("projectId".into(), json!(project_id));
    repo.insert_into(&mut args);
    if let Some(reason) = reason {
        args.insert("reason".into(), json!(reason));
    }
    Value::Object(args)
}

// =============================================================================
// The already-attached check
// =============================================================================

/// A subpath as the app stores it: without surrounding whitespace or
/// slashes, and `None` when nothing is left.
fn normalize_subpath(subpath: Option<&str>) -> Option<&str> {
    let normalized = subpath?.trim().trim_matches('/').trim();
    (!normalized.is_empty()).then_some(normalized)
}

/// Refuse a repo and subpath the project already has, naming the branch it
/// has there. Repo names compare without case, as GitHub's do. Subpaths
/// compare exactly, once normalised.
///
/// The app refuses the same thing when the row is written. This answers
/// first, before the app asks GitHub about the repo, and says more. It
/// cannot see a fork that a pull request swaps for its parent. The app's
/// own refusal covers that.
pub(crate) fn ensure_not_attached(
    project: &Project,
    attached: &[RepoRow],
    address: &RepoAddress,
) -> Result<(), CtlError> {
    let wanted_repo = address.github_repo.trim();
    let wanted_subpath = normalize_subpath(address.subpath.as_deref());
    let Some(existing) = attached.iter().find(|row| {
        row.github_repo.eq_ignore_ascii_case(wanted_repo)
            && normalize_subpath(row.subpath.as_deref()) == wanted_subpath
    }) else {
        return Ok(());
    };
    let shown = repo_address(&existing.github_repo, existing.subpath.as_deref());
    let branch = if existing.branch_name.is_empty() {
        String::new()
    } else {
        format!(", on branch {}", existing.branch_name)
    };
    Err(CtlError::command(
        "already_attached",
        format!(
            "{shown} is already attached to project {:?}{branch}. A project holds a repo \
             once per subpath: address the existing one with --repo {shown}, or give a \
             different subpath",
            project.name
        ),
    ))
}

// =============================================================================
// The re-read
// =============================================================================

/// The project note a pull request's description was saved as.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteRow {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    created_at: i64,
}

/// What background work the app started for a new repo's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Setup {
    /// A local branch: a git worktree, then the repo's setup actions.
    Worktree,
    /// A remote branch with no workspace to join: it provisions this one.
    ProvisionWorkspace(String),
    /// A remote branch joining the project's workspace: cloned into it.
    CloneIntoWorkspace(String),
}

impl Setup {
    /// Which of the three `branch` got, read off the project's branches.
    /// The app clones into a workspace another branch of the project
    /// already names, and provisions one otherwise.
    fn of(branch: &BranchRow, project_branches: &[BranchRow]) -> Self {
        let named = |b: &BranchRow| {
            b.workspace_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        };
        match named(branch) {
            None => Self::Worktree,
            Some(workspace) => {
                let joins = project_branches
                    .iter()
                    .any(|other| other.id != branch.id && named(other).is_some());
                if joins {
                    Self::CloneIntoWorkspace(workspace)
                } else {
                    Self::ProvisionWorkspace(workspace)
                }
            }
        }
    }

    fn line(&self) -> String {
        match self {
            Self::Worktree => {
                "worktree and setup actions are running in the background".to_string()
            }
            Self::ProvisionWorkspace(workspace) => format!(
                "workspace {workspace} is being provisioned; it starts while a Staged window \
                 is open"
            ),
            Self::CloneIntoWorkspace(workspace) => {
                format!("cloning into workspace {workspace} in the background")
            }
        }
    }
}

/// One repo of a project as it stands after a write: its row, its branches,
/// the pull request note if one was saved, and what setup was started.
#[derive(Debug, Clone)]
pub(crate) struct RepoState {
    repo: RepoRow,
    repo_raw: Value,
    branches: Vec<BranchRow>,
    branches_raw: Vec<Value>,
    note: Option<NoteRow>,
    setup: Option<Setup>,
}

/// The newest project note titled for pull request `number`, which is how
/// the app titles the note it saves a pull request's description as.
fn find_pr_note(
    invoke: &mut impl FnMut(&str, Value) -> Result<Value, CtlError>,
    project_id: &str,
    number: u64,
) -> Result<Option<NoteRow>, CtlError> {
    let raw = invoke("list_project_notes", json!({ "projectId": project_id }))?;
    let notes: Vec<NoteRow> = parse(&raw, "project note list")?;
    let prefix = format!("PR #{number}:");
    Ok(notes
        .into_iter()
        .filter(|note| note.title.starts_with(&prefix))
        .max_by_key(|note| note.created_at))
}

/// Read a project's repos and branches, and for each repo `wanted` accepts,
/// its state. The note is looked up only for a branch that carries a pull
/// request.
fn read_repo_states(
    invoke: &mut impl FnMut(&str, Value) -> Result<Value, CtlError>,
    project_id: &str,
    wanted: impl Fn(&RepoRow) -> bool,
) -> Result<Vec<RepoState>, CtlError> {
    let repos_raw = invoke("list_project_repos", json!({ "projectId": project_id }))?;
    let repos: Vec<RepoRow> = parse(&repos_raw, "project repo list")?;
    let branches_raw = invoke(
        "list_branches_for_project",
        json!({ "projectId": project_id }),
    )?;
    let all_branches: Vec<BranchRow> = parse(&branches_raw, "branch list")?;

    let mut states = Vec::new();
    for (i, repo) in repos.iter().enumerate() {
        if !wanted(repo) {
            continue;
        }
        let branches: Vec<BranchRow> = all_branches
            .iter()
            .filter(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str()))
            .cloned()
            .collect();
        let note = match branches.iter().find_map(|b| b.pr_number) {
            Some(number) => find_pr_note(invoke, project_id, number)?,
            None => None,
        };
        let setup = branches.first().map(|b| Setup::of(b, &all_branches));
        states.push(RepoState {
            repo: repo.clone(),
            repo_raw: repos_raw.get(i).cloned().unwrap_or(Value::Null),
            branches_raw: raw_branches_of_repo(&branches_raw, &repo.id),
            branches,
            note,
            setup,
        });
    }
    Ok(states)
}

fn note_json(note: &NoteRow) -> Value {
    json!({ "id": note.id, "title": note.title })
}

/// A repo's branches, one line each, as `project get` prints them.
fn branch_lines(state: &RepoState, indent: &str) -> Vec<String> {
    if state.branches.is_empty() {
        return vec![format!("{indent}(no branch yet)")];
    }
    state
        .branches
        .iter()
        .map(|branch| format!("{indent}{}", branch_summary(branch)))
        .collect()
}

/// What else there is to say about a new repo: why it was added, the pull
/// request note, and the setup the app started.
fn outcome_lines(state: &RepoState) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(reason) = state.repo.reason.as_deref().map(str::trim) {
        if !reason.is_empty() {
            lines.push(format!("  reason: {reason}"));
        }
    }
    if let Some(note) = &state.note {
        lines.push(format!(
            "  note: #project-note:{}  {}",
            note.id,
            note.title.trim()
        ));
    }
    if let Some(setup) = &state.setup {
        lines.push(format!("  setup: {}", setup.line()));
    }
    lines
}

/// The error for a write that landed and a re-read that did not.
fn landed_but_unread(
    command: &'static str,
    args: Value,
    result: Value,
    human: String,
    error: CtlError,
) -> CtlError {
    CtlError::Partial {
        error: Box::new(error),
        applied: vec![json!({ "command": command, "args": args, "result": result })],
        human,
    }
}

// =============================================================================
// project create
// =============================================================================

pub fn project_create(
    client: &Client,
    name: &str,
    location: ProjectLocation,
    repo: Option<&RepoRequest>,
) -> Result<CmdOutput, CtlError> {
    create_with(
        |command, args| client.invoke(command, args),
        name,
        location,
        repo,
    )
}

/// [`project_create`] over any `invoke`, so it can be tested without a
/// Client.
///
/// The name is not checked against `list_projects` first: a session sees
/// only the projects it covers, so the app answers for a name in use. A
/// refused create is the app's error as it stands. Once the create landed,
/// a failed re-read is a [`CtlError::Partial`].
pub(crate) fn create_with(
    mut invoke: impl FnMut(&str, Value) -> Result<Value, CtlError>,
    name: &str,
    location: ProjectLocation,
    repo: Option<&RepoRequest>,
) -> Result<CmdOutput, CtlError> {
    let args = create_args(name, location, repo);
    let created = invoke("create_project", args.clone())?;

    let project: Project = match parse(&created, "created project") {
        Ok(project) => project,
        Err(error) => {
            let human = format!(
                "project {:?} created\n  (created; the app's answer could not be read)",
                name.trim()
            );
            return Err(landed_but_unread(
                "create_project",
                args,
                created,
                human,
                error,
            ));
        }
    };
    // The project is new, so every repo it has is the one it was made with.
    match read_repo_states(&mut invoke, &project.id, |_| true) {
        Ok(repos) => Ok(render_created(&project, &created, &repos)),
        Err(error) => {
            let human = format!(
                "project {:?} created\n  id: {}\n  (created; re-reading its repos failed)\n\
                 next: stagedctl project get {}",
                project.name, project.id, project.id
            );
            Err(landed_but_unread(
                "create_project",
                args,
                created,
                human,
                error,
            ))
        }
    }
}

/// `project create`'s output: the `project get` shape for the new project.
/// `--json` adds `ok: true`, as the other writes do, and the pull request
/// note's id and title when one was saved.
pub(crate) fn render_created(
    project: &Project,
    project_raw: &Value,
    repos: &[RepoState],
) -> CmdOutput {
    let mut human = vec![
        format!("project {:?} created", project.name),
        format!("  id: {}", project.id),
        format!(
            "  location: {}",
            project.location.as_deref().unwrap_or("local")
        ),
        "  repos:".to_string(),
    ];
    if repos.is_empty() {
        human.push("    (none)".to_string());
    }
    for state in repos {
        human.push(format!(
            "    {}{}",
            repo_address(&state.repo.github_repo, state.repo.subpath.as_deref()),
            if state.repo.is_primary {
                "  (primary)"
            } else {
                ""
            }
        ));
        human.extend(branch_lines(state, "      "));
    }
    human.extend(repos.iter().flat_map(outcome_lines));
    human.push(if repos.is_empty() {
        format!(
            "next: stagedctl project add-repo {} --repo <owner>/<repo>[:<subpath>]",
            project.id
        )
    } else {
        format!("next: stagedctl project get {}", project.id)
    });

    let mut json = Map::new();
    json.insert("ok".into(), json!(true));
    json.insert("project".into(), project_raw.clone());
    json.insert(
        "repos".into(),
        repos
            .iter()
            .map(|state| json!({ "repo": state.repo_raw, "branches": state.branches_raw }))
            .collect(),
    );
    if let Some(note) = repos.iter().find_map(|state| state.note.as_ref()) {
        json.insert("note".into(), note_json(note));
    }
    CmdOutput {
        json: Value::Object(json),
        human: human.join("\n"),
    }
}

// =============================================================================
// project add-repo
// =============================================================================

pub fn project_add_repo(
    client: &Client,
    needle: &str,
    repo: &RepoRequest,
    reason: Option<&str>,
) -> Result<CmdOutput, CtlError> {
    add_repo_with(
        |command, args| client.invoke(command, args),
        needle,
        repo,
        reason,
    )
}

/// [`project_add_repo`] over any `invoke`, so it can be tested without a
/// Client.
///
/// `<PROJECT>` resolves among the projects `list_projects` returns, which
/// for a session is its own project and the ones it created. Anything else
/// is `not_found`, by name or by id, and the app is asked nothing more.
pub(crate) fn add_repo_with(
    mut invoke: impl FnMut(&str, Value) -> Result<Value, CtlError>,
    needle: &str,
    repo: &RepoRequest,
    reason: Option<&str>,
) -> Result<CmdOutput, CtlError> {
    let projects_raw = invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&projects_raw, "project list")?;
    let project = find_project(&projects, needle)?.clone();

    let attached_raw = invoke("list_project_repos", json!({ "projectId": project.id }))?;
    let attached: Vec<RepoRow> = parse(&attached_raw, "project repo list")?;
    ensure_not_attached(&project, &attached, &repo.address)?;

    let args = add_repo_args(&project.id, repo, reason);
    let added = invoke("add_project_repo", args.clone())?;

    let reread = parse::<RepoRow>(&added, "added repo").and_then(|row| {
        let state = read_repo_states(&mut invoke, &project.id, |r| r.id == row.id)?
            .into_iter()
            .next();
        // A row the list no longer has was removed in the app meanwhile:
        // report it from the app's own answer.
        Ok(state.unwrap_or(RepoState {
            repo: row,
            repo_raw: added.clone(),
            branches: Vec::new(),
            branches_raw: Vec::new(),
            note: None,
            setup: None,
        }))
    });
    match reread {
        Ok(state) => Ok(render_added(&project, &state)),
        Err(error) => {
            let shown = added
                .get("githubRepo")
                .and_then(Value::as_str)
                .map(|name| repo_address(name, added.get("subpath").and_then(Value::as_str)))
                .unwrap_or_else(|| {
                    repo_address(&repo.address.github_repo, repo.address.subpath.as_deref())
                });
            let human = format!(
                "repo {shown} added to project {:?}\n  (added; re-reading it failed)\n\
                 next: stagedctl project get {}",
                project.name, project.id
            );
            Err(landed_but_unread(
                "add_project_repo",
                args,
                added,
                human,
                error,
            ))
        }
    }
}

/// `project add-repo`'s output: the new repo with its branch, as
/// `project get` prints one. `--json` is `{ ok, projectId, repo, branches }`
/// with the app's rows, plus the pull request note's id and title when one
/// was saved.
pub(crate) fn render_added(project: &Project, state: &RepoState) -> CmdOutput {
    let mut human = vec![format!(
        "repo {} added to project {:?}{}",
        repo_address(&state.repo.github_repo, state.repo.subpath.as_deref()),
        project.name,
        if state.repo.is_primary {
            "  (primary)"
        } else {
            ""
        }
    )];
    human.extend(branch_lines(state, "  "));
    human.extend(outcome_lines(state));
    human.push(format!("next: stagedctl project get {}", project.id));

    let mut json = Map::new();
    json.insert("ok".into(), json!(true));
    json.insert("projectId".into(), json!(project.id));
    json.insert("repo".into(), state.repo_raw.clone());
    json.insert("branches".into(), Value::Array(state.branches_raw.clone()));
    if let Some(note) = &state.note {
        json.insert("note".into(), note_json(note));
    }
    CmdOutput {
        json: Value::Object(json),
        human: human.join("\n"),
    }
}

#[cfg(test)]
mod tests;
