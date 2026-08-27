//! Verb implementations: resolve human-friendly addresses to ids, call the
//! allowlisted dispatch commands, and render results.
//!
//! This is deliberately a dumb flag→JSON mapper (berd's layering): the app
//! is the trust boundary and does all real validation; everything here is
//! UX — address resolution, a poll loop for `session wait`, and printing
//! ids in the hashtag reference forms agents paste into prompts.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli::{Cli, ImageVerb, Noun, ProjectVerb, SessionType, SessionVerb, TimelineVerb};
use crate::client::{default_data_dir, resolve_lock_path, Client, CtlError, STAGEDCTL_LOCK_ENV};

/// One command's result: the value `--json` prints, and the human rendering.
pub struct CmdOutput {
    pub json: Value,
    pub human: String,
}

pub fn run(cli: Cli) -> Result<CmdOutput, CtlError> {
    validate(&cli.command)?;
    let lock_path = resolve_lock_path(
        cli.lock_path.as_deref(),
        std::env::var_os(STAGEDCTL_LOCK_ENV),
        default_data_dir().as_deref(),
    )?;
    let client = Client::connect(&lock_path)?;

    match cli.command {
        Noun::Project(ProjectVerb::List) => project_list(&client),
        Noun::Project(ProjectVerb::Get { project }) => project_get(&client, &project),
        Noun::Timeline(TimelineVerb::Show {
            project,
            repo,
            branch,
        }) => timeline_show(&client, &project, repo.as_deref(), branch.as_deref()),
        Noun::Session(SessionVerb::Start {
            project,
            repo,
            branch,
            r#type,
            prompt,
        }) => session_start(
            &client,
            &project,
            repo.as_deref(),
            branch.as_deref(),
            r#type,
            &prompt,
        ),
        Noun::Session(SessionVerb::Status { session_id }) => session_status(&client, &session_id),
        Noun::Session(SessionVerb::Wait {
            session_id,
            timeout,
        }) => session_wait(&client, &session_id, timeout),
        Noun::Session(SessionVerb::Cancel { session_id }) => session_cancel(&client, &session_id),
        Noun::Session(SessionVerb::List { project, repo }) => {
            session_list(&client, project.as_deref(), repo.as_deref())
        }
        Noun::Image(ImageVerb::Add {
            project,
            repo,
            branch,
            file,
        }) => image_add(&client, &project, &repo, branch.as_deref(), &file),
    }
}

/// Flag-pairing rules that need no app: checked before discovery so they
/// fail the same way (exit 1) whether or not a Staged app is running.
fn validate(command: &Noun) -> Result<(), CtlError> {
    let invalid = |message: &str| Err(CtlError::command("invalid_arguments", message.to_string()));
    match command {
        Noun::Timeline(TimelineVerb::Show { repo, branch, .. })
            if branch.is_some() && repo.is_none() =>
        {
            invalid("--branch only applies together with --repo")
        }
        Noun::Session(SessionVerb::Start {
            repo,
            branch,
            r#type,
            prompt,
            ..
        }) => {
            if prompt.trim().is_empty() {
                return invalid("--prompt must not be empty");
            }
            if branch.is_some() && repo.is_none() {
                return invalid("--branch only applies together with --repo");
            }
            match (repo, r#type) {
                (Some(_), None) => {
                    invalid("--type is required with --repo (one of: note, commit, review)")
                }
                (None, Some(_)) => invalid(
                    "--type only applies with --repo; a project session always produces a \
                     project note",
                ),
                _ => Ok(()),
            }
        }
        Noun::Session(SessionVerb::List { project, repo })
            if repo.is_some() && project.is_none() =>
        {
            invalid("--repo requires --project (repo addresses are project-scoped)")
        }
        _ => Ok(()),
    }
}

// =============================================================================
// Wire shapes (camelCase, tolerant of extra fields)
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: String,
    name: String,
    #[serde(default)]
    github_repo: Option<String>,
    #[serde(default)]
    subpath: Option<String>,
    #[serde(default)]
    location: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoRow {
    id: String,
    github_repo: String,
    #[serde(default)]
    subpath: Option<String>,
    #[serde(default)]
    is_primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchRow {
    id: String,
    #[serde(default)]
    project_repo_id: Option<String>,
    branch_name: String,
    #[serde(default)]
    base_branch: String,
    #[serde(default)]
    pr_number: Option<u64>,
    #[serde(default)]
    pr_state: Option<String>,
    #[serde(default)]
    pr_checks_status: Option<String>,
    #[serde(default)]
    pr_url: Option<String>,
    #[serde(default)]
    worktree_path: Option<String>,
    #[serde(default)]
    workspace_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRow {
    id: String,
    #[serde(default)]
    prompt: String,
    status: String,
    #[serde(default)]
    working_dir: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default)]
    completion_reason: Option<String>,
    #[serde(default)]
    acp_title: Option<String>,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveSessionRow {
    session_id: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    branch_id: Option<String>,
    #[serde(default)]
    session_type: Option<String>,
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchStartResponse {
    session_id: String,
    artifact_id: String,
    session_status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectStartResponse {
    session_id: String,
    note_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageResponse {
    id: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    size_bytes: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct TimelineWire {
    commits: Vec<CommitItem>,
    notes: Vec<NoteItem>,
    reviews: Vec<ReviewItem>,
    images: Vec<ImageItem>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct CommitItem {
    id: Option<String>,
    sha: String,
    subject: String,
    author: String,
    timestamp: i64,
    sort_timestamp: i64,
    order: i64,
    session_id: Option<String>,
    session_status: Option<String>,
    completion_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct NoteItem {
    id: String,
    title: String,
    created_at: i64,
    session_id: Option<String>,
    session_status: Option<String>,
    completion_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ReviewItem {
    id: String,
    commit_sha: String,
    scope: String,
    title: Option<String>,
    comment_count: u64,
    created_at: i64,
    completed_at: Option<i64>,
    session_id: Option<String>,
    session_status: Option<String>,
    completion_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ImageItem {
    id: String,
    filename: String,
    created_at: i64,
    session_id: Option<String>,
    session_status: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ProjectNoteRow {
    id: String,
    title: String,
    session_id: Option<String>,
    session_status: Option<String>,
    completion_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectTimelineWire {
    #[serde(default)]
    project_notes: Vec<ProjectNoteRow>,
    #[serde(default)]
    branches: Vec<ProjectBranchWire>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectBranchWire {
    branch: BranchRow,
    #[serde(default)]
    github_repo: Option<String>,
    #[serde(default)]
    subpath: Option<String>,
    #[serde(default)]
    timeline: Option<TimelineWire>,
    #[serde(default)]
    error: Option<String>,
}

fn parse<T: serde::de::DeserializeOwned>(value: &Value, what: &str) -> Result<T, CtlError> {
    serde_json::from_value(value.clone()).map_err(|e| {
        CtlError::Transport(format!(
            "unexpected {what} shape from the app (CLI/app drift?): {e}"
        ))
    })
}

// =============================================================================
// Address resolution
// =============================================================================

/// A `--repo` value: `<owner>/<repo>[:<subpath>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoAddress {
    pub github_repo: String,
    pub subpath: Option<String>,
}

impl RepoAddress {
    pub fn parse(input: &str) -> Result<Self, CtlError> {
        let (repo, subpath) = match input.split_once(':') {
            Some((repo, subpath)) if !subpath.is_empty() => (repo, Some(subpath.to_string())),
            Some((repo, _)) => (repo, None),
            None => (input, None),
        };
        if repo.trim().is_empty() || !repo.contains('/') {
            return Err(CtlError::command(
                "invalid_arguments",
                format!(
                    "repo address must look like <owner>/<repo>[:<subpath>], got {input:?} \
                     (copy one from `stagedctl project get`)"
                ),
            ));
        }
        Ok(Self {
            github_repo: repo.to_string(),
            subpath,
        })
    }
}

fn repo_address(github_repo: &str, subpath: Option<&str>) -> String {
    match subpath {
        Some(subpath) => format!("{github_repo}:{subpath}"),
        None => github_repo.to_string(),
    }
}

/// Match a `<project>` argument against the app's project list: exact id,
/// then exact name, then unique case-insensitive name.
fn find_project<'a>(projects: &'a [Project], needle: &str) -> Result<&'a Project, CtlError> {
    if let Some(project) = projects.iter().find(|p| p.id == needle) {
        return Ok(project);
    }
    if let Some(project) = projects.iter().find(|p| p.name == needle) {
        return Ok(project);
    }
    let loose: Vec<&Project> = projects
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(needle))
        .collect();
    match loose.len() {
        1 => Ok(loose[0]),
        0 => {
            let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
            Err(CtlError::command(
                "not_found",
                if names.is_empty() {
                    format!("no project named {needle:?} — the app has no projects yet")
                } else {
                    format!(
                        "no project named {needle:?}. Projects: {}",
                        names.join(", ")
                    )
                },
            ))
        }
        _ => Err(CtlError::command(
            "ambiguous",
            format!(
                "{needle:?} matches more than one project by case-insensitive name; \
                 use the exact name or the project id from `stagedctl project list`"
            ),
        )),
    }
}

fn resolve_project(client: &Client, needle: &str) -> Result<Project, CtlError> {
    let raw = client.invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&raw, "project list")?;
    find_project(&projects, needle).cloned()
}

struct ResolvedBranch {
    branch: BranchRow,
    address: String,
}

/// Resolve `--repo` (and optional `--branch`) to the single branch row the
/// backing commands need. `branchId` is the only stable handle in the app's
/// model — project + branch name is ambiguous by construction — so every
/// repo-scoped verb funnels through here.
fn resolve_branch(
    client: &Client,
    project: &Project,
    repo_flag: &str,
    branch_flag: Option<&str>,
) -> Result<ResolvedBranch, CtlError> {
    let address = RepoAddress::parse(repo_flag)?;

    let repos_raw = client.invoke("list_project_repos", json!({"projectId": project.id}))?;
    let repos: Vec<RepoRow> = parse(&repos_raw, "project repo list")?;
    let repo = repos
        .iter()
        .find(|r| {
            r.github_repo.eq_ignore_ascii_case(&address.github_repo)
                && r.subpath.as_deref() == address.subpath.as_deref()
        })
        .ok_or_else(|| {
            let attached: Vec<String> = repos
                .iter()
                .map(|r| repo_address(&r.github_repo, r.subpath.as_deref()))
                .collect();
            CtlError::command(
                "not_found",
                format!(
                    "project {:?} has no repo {}. Attached repos: {}",
                    project.name,
                    repo_address(&address.github_repo, address.subpath.as_deref()),
                    if attached.is_empty() {
                        "none".to_string()
                    } else {
                        attached.join(", ")
                    }
                ),
            )
        })?;

    let branches_raw = client.invoke(
        "list_branches_for_project",
        json!({"projectId": project.id}),
    )?;
    let branches: Vec<BranchRow> = parse(&branches_raw, "branch list")?;
    let mut hits: Vec<BranchRow> = branches
        .into_iter()
        .filter(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str()))
        .filter(|b| branch_flag.map_or(true, |name| b.branch_name == name))
        .collect();

    let shown_address = repo_address(&repo.github_repo, repo.subpath.as_deref());
    match hits.len() {
        1 => Ok(ResolvedBranch {
            branch: hits.remove(0),
            address: shown_address,
        }),
        0 => Err(CtlError::command(
            "not_found",
            match branch_flag {
                Some(name) => format!(
                    "repo {shown_address} has no branch named {name:?} in project {:?}",
                    project.name
                ),
                None => format!(
                    "repo {shown_address} has no branch in project {:?} yet",
                    project.name
                ),
            },
        )),
        _ => {
            let names: Vec<&str> = hits.iter().map(|b| b.branch_name.as_str()).collect();
            Err(CtlError::command(
                "ambiguous",
                format!(
                    "repo {shown_address} has {} branches in project {:?}: {}. \
                     Disambiguate with --branch <name>",
                    hits.len(),
                    project.name,
                    names.join(", ")
                ),
            ))
        }
    }
}

// =============================================================================
// project
// =============================================================================

fn project_list(client: &Client) -> Result<CmdOutput, CtlError> {
    let raw = client.invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&raw, "project list")?;

    let human = if projects.is_empty() {
        "no projects".to_string()
    } else {
        projects
            .iter()
            .map(|p| {
                let repo = p
                    .github_repo
                    .as_deref()
                    .map(|r| repo_address(r, p.subpath.as_deref()))
                    .unwrap_or_else(|| "(no repo)".to_string());
                format!(
                    "{}  {}  {}  id:{}",
                    p.name,
                    repo,
                    p.location.as_deref().unwrap_or("local"),
                    p.id
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(CmdOutput { json: raw, human })
}

fn project_get(client: &Client, needle: &str) -> Result<CmdOutput, CtlError> {
    let projects_raw = client.invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&projects_raw, "project list")?;
    let project = find_project(&projects, needle)?.clone();
    let project_raw = projects_raw
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(project.id.as_str()))
        })
        .cloned()
        .unwrap_or(Value::Null);

    let repos_raw = client.invoke("list_project_repos", json!({"projectId": project.id}))?;
    let repos: Vec<RepoRow> = parse(&repos_raw, "project repo list")?;
    let branches_raw = client.invoke(
        "list_branches_for_project",
        json!({"projectId": project.id}),
    )?;
    let branches: Vec<BranchRow> = parse(&branches_raw, "branch list")?;

    let mut joined = Vec::new();
    let mut human = vec![
        format!("project {}", project.name),
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
    for (i, repo) in repos.iter().enumerate() {
        let address = repo_address(&repo.github_repo, repo.subpath.as_deref());
        human.push(format!(
            "    {address}{}",
            if repo.is_primary { "  (primary)" } else { "" }
        ));
        let repo_branches: Vec<&BranchRow> = branches
            .iter()
            .filter(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str()))
            .collect();
        if repo_branches.is_empty() {
            human.push("      (no branch yet)".to_string());
        }
        for branch in &repo_branches {
            human.push(format!("      {}", branch_summary(branch)));
        }
        joined.push(json!({
            "repo": repos_raw.get(i).cloned().unwrap_or(Value::Null),
            "branches": branches_raw
                .as_array()
                .map(|a| a
                    .iter()
                    .filter(|v| v.get("projectRepoId").and_then(Value::as_str)
                        == Some(repo.id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>())
                .unwrap_or_default(),
        }));
    }

    Ok(CmdOutput {
        json: json!({ "project": project_raw, "repos": joined }),
        human: human.join("\n"),
    })
}

fn branch_summary(branch: &BranchRow) -> String {
    let mut parts = vec![format!("branch {}", branch.branch_name)];
    if !branch.base_branch.is_empty() {
        parts.push(format!("base:{}", branch.base_branch));
    }
    if let Some(n) = branch.pr_number {
        let mut pr = format!("PR #{n}");
        if let Some(state) = &branch.pr_state {
            pr.push_str(&format!(" {state}"));
        }
        if let Some(checks) = &branch.pr_checks_status {
            pr.push_str(&format!(" checks:{checks}"));
        }
        if let Some(url) = &branch.pr_url {
            pr.push_str(&format!(" {url}"));
        }
        parts.push(pr);
    }
    if let Some(workspace) = &branch.workspace_name {
        parts.push(format!("remote workspace:{workspace}"));
    } else if let Some(path) = &branch.worktree_path {
        parts.push(format!("worktree:{path}"));
    }
    parts.join("  ")
}

// =============================================================================
// timeline
// =============================================================================

fn timeline_show(
    client: &Client,
    project_needle: &str,
    repo_flag: Option<&str>,
    branch_flag: Option<&str>,
) -> Result<CmdOutput, CtlError> {
    let project = resolve_project(client, project_needle)?;

    match repo_flag {
        Some(repo_flag) => {
            let resolved = resolve_branch(client, &project, repo_flag, branch_flag)?;
            let raw = client.invoke(
                "get_branch_timeline",
                json!({"branchId": resolved.branch.id}),
            )?;
            let timeline: TimelineWire = parse(&raw, "branch timeline")?;

            let mut human = vec![format!(
                "repo {}  branch {}",
                resolved.address, resolved.branch.branch_name
            )];
            human.extend(timeline_lines(&timeline, "  "));
            Ok(CmdOutput {
                json: json!({
                    "project": { "id": project.id, "name": project.name },
                    "repo": resolved.address,
                    "branchId": resolved.branch.id,
                    "timeline": raw,
                }),
                human: human.join("\n"),
            })
        }
        None => {
            let raw = client.invoke("get_project_timeline", json!({"projectId": project.id}))?;
            let timeline: ProjectTimelineWire = parse(&raw, "project timeline")?;

            let mut human = vec![
                format!("project {}", project.name),
                "project notes:".to_string(),
            ];
            if timeline.project_notes.is_empty() {
                human.push("  (none)".to_string());
            }
            for note in &timeline.project_notes {
                human.push(format!(
                    "  #project-note:{}  {}{}",
                    note.id,
                    display_title(&note.title),
                    session_suffix(
                        note.session_id.as_deref(),
                        note.session_status.as_deref(),
                        note.completion_reason.as_deref()
                    )
                ));
            }
            for branch in &timeline.branches {
                let address = branch
                    .github_repo
                    .as_deref()
                    .map(|r| repo_address(r, branch.subpath.as_deref()))
                    .unwrap_or_else(|| "(detached repo)".to_string());
                human.push(format!(
                    "repo {address}  branch {}:",
                    branch.branch.branch_name
                ));
                match (&branch.timeline, &branch.error) {
                    (Some(timeline), _) => human.extend(timeline_lines(timeline, "  ")),
                    (None, Some(error)) => human.push(format!("  timeline error: {error}")),
                    (None, None) => human.push("  (no timeline)".to_string()),
                }
            }
            Ok(CmdOutput {
                json: json!({
                    "project": { "id": project.id, "name": project.name },
                    "timeline": raw,
                }),
                human: human.join("\n"),
            })
        }
    }
}

/// Merge a branch timeline's four item arrays into one chronological list,
/// the way the app's timeline view does (sort timestamp, then insertion
/// order as tiebreak).
fn timeline_lines(timeline: &TimelineWire, indent: &str) -> Vec<String> {
    let mut items: Vec<(i64, usize, String)> = Vec::new();

    for commit in &timeline.commits {
        let suffix = session_suffix(
            commit.session_id.as_deref(),
            commit.session_status.as_deref(),
            commit.completion_reason.as_deref(),
        );
        let line = if commit.sha.is_empty() {
            // A pending row: the commit a running/queued session will produce.
            let id = commit.id.as_deref().unwrap_or("?");
            format!(
                "pending commit {id}  {}{suffix}",
                display_title(&commit.subject)
            )
        } else {
            format!(
                "#commit:{}  {}  ({}, {}){suffix}",
                commit.sha,
                display_title(&commit.subject),
                commit.author,
                format_timestamp(commit.timestamp)
            )
        };
        items.push((commit.sort_timestamp, commit.order as usize, line));
    }
    for note in &timeline.notes {
        items.push((
            note.created_at,
            usize::MAX,
            format!(
                "#note:{}  {}{}",
                note.id,
                display_title(&note.title),
                session_suffix(
                    note.session_id.as_deref(),
                    note.session_status.as_deref(),
                    note.completion_reason.as_deref()
                )
            ),
        ));
    }
    for review in &timeline.reviews {
        let title = review
            .title
            .clone()
            .unwrap_or_else(|| format!("review of #commit:{}", short_sha(&review.commit_sha)));
        items.push((
            review.completed_at.unwrap_or(review.created_at),
            usize::MAX,
            format!(
                "#review:{}  {}  ({} scope, {} comments){}",
                review.id,
                display_title(&title),
                review.scope,
                review.comment_count,
                session_suffix(
                    review.session_id.as_deref(),
                    review.session_status.as_deref(),
                    review.completion_reason.as_deref()
                )
            ),
        ));
    }
    for image in &timeline.images {
        items.push((
            image.created_at,
            usize::MAX,
            format!(
                "image {}  {}{}",
                image.id,
                image.filename,
                session_suffix(
                    image.session_id.as_deref(),
                    image.session_status.as_deref(),
                    None
                )
            ),
        ));
    }

    if items.is_empty() {
        return vec![format!("{indent}(no timeline items)")];
    }
    items.sort_by_key(|(ts, order, _)| (*ts, *order));
    items
        .into_iter()
        .map(|(_, _, line)| format!("{indent}{line}"))
        .collect()
}

/// `[session <id> <status>]` marker for items whose producing session is
/// not (successfully) finished — running work, queued work, and failures.
fn session_suffix(
    session_id: Option<&str>,
    status: Option<&str>,
    completion_reason: Option<&str>,
) -> String {
    match status {
        Some(status) if status != "completed" => {
            let id = session_id.unwrap_or("?");
            let reason = completion_reason
                .map(|r| format!(": {r}"))
                .unwrap_or_default();
            format!("  [session {id} {status}{reason}]")
        }
        _ => String::new(),
    }
}

fn display_title(title: &str) -> &str {
    if title.trim().is_empty() {
        "(untitled)"
    } else {
        title.trim()
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

fn format_timestamp(unix_seconds: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix_seconds)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| unix_seconds.to_string())
}

// =============================================================================
// session
// =============================================================================

fn session_start(
    client: &Client,
    project_needle: &str,
    repo_flag: Option<&str>,
    branch_flag: Option<&str>,
    session_type: Option<SessionType>,
    prompt: &str,
) -> Result<CmdOutput, CtlError> {
    let project = resolve_project(client, project_needle)?;

    match repo_flag {
        Some(repo_flag) => {
            // The (repo, type) pairing is enforced in validate().
            let Some(session_type) = session_type else {
                unreachable!("validate() requires --type with --repo");
            };
            let resolved = resolve_branch(client, &project, repo_flag, branch_flag)?;
            let raw = client.invoke(
                "start_branch_session",
                json!({
                    "branchId": resolved.branch.id,
                    "prompt": prompt,
                    "sessionType": session_type.as_str(),
                }),
            )?;
            let response: BranchStartResponse = parse(&raw, "session start response")?;

            let artifact = match session_type {
                SessionType::Note => format!("#note:{}", response.artifact_id),
                SessionType::Review => format!("#review:{}", response.artifact_id),
                SessionType::Commit => format!(
                    "pending commit {} (becomes #commit:<sha> when it lands)",
                    response.artifact_id
                ),
            };
            let human = [
                format!(
                    "session {} {}",
                    response.session_id, response.session_status
                ),
                format!(
                    "type: {} on {} (branch {})",
                    session_type.as_str(),
                    resolved.address,
                    resolved.branch.branch_name
                ),
                format!("artifact: {artifact}"),
                format!("next: stagedctl session wait {}", response.session_id),
            ]
            .join("\n");

            Ok(CmdOutput {
                json: json!({
                    "sessionId": response.session_id,
                    "artifactId": response.artifact_id,
                    "sessionStatus": response.session_status,
                    "sessionType": session_type.as_str(),
                    "projectId": project.id,
                    "branchId": resolved.branch.id,
                }),
                human,
            })
        }
        None => {
            let raw = client.invoke(
                "start_project_session",
                json!({"projectId": project.id, "prompt": prompt}),
            )?;
            let response: ProjectStartResponse = parse(&raw, "session start response")?;
            let human = [
                format!("session {} running", response.session_id),
                format!("type: project session on {}", project.name),
                format!("artifact: #project-note:{}", response.note_id),
                format!("next: stagedctl session wait {}", response.session_id),
            ]
            .join("\n");
            Ok(CmdOutput {
                json: json!({
                    "sessionId": response.session_id,
                    "noteId": response.note_id,
                    "projectId": project.id,
                }),
                human,
            })
        }
    }
}

fn fetch_session(client: &Client, session_id: &str) -> Result<(SessionRow, Value), CtlError> {
    let raw = client.invoke("get_session", json!({"sessionId": session_id}))?;
    if raw.is_null() {
        return Err(CtlError::command(
            "not_found",
            format!("no session with id {session_id}"),
        ));
    }
    let session: SessionRow = parse(&raw, "session")?;
    Ok((session, raw))
}

fn render_session(session: &SessionRow) -> String {
    let mut lines = vec![format!(
        "session {}: {}{}",
        session.id,
        session.status,
        session
            .completion_reason
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default()
    )];
    if let Some(provider) = &session.provider {
        lines.push(format!("provider: {provider}"));
    }
    if let Some(title) = session.acp_title.as_deref().filter(|t| !t.is_empty()) {
        lines.push(format!("title: {title}"));
    }
    lines.push(format!("prompt: {}", truncate(&session.prompt, 160)));
    if !session.working_dir.is_empty() {
        lines.push(format!("working dir: {}", session.working_dir));
    }
    if let Some(error) = session.error_message.as_deref().filter(|e| !e.is_empty()) {
        lines.push(format!("error: {error}"));
    }
    lines.push(format!(
        "created: {}  updated: {}",
        format_timestamp(session.created_at),
        format_timestamp(session.updated_at)
    ));
    lines.join("\n")
}

fn truncate(text: &str, max_chars: usize) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= max_chars {
        single_line
    } else {
        let cut: String = single_line.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

fn session_status(client: &Client, session_id: &str) -> Result<CmdOutput, CtlError> {
    let (session, raw) = fetch_session(client, session_id)?;
    Ok(CmdOutput {
        json: raw,
        human: render_session(&session),
    })
}

const WAIT_POLL_INTERVAL: Duration = Duration::from_secs(2);

fn session_wait(
    client: &Client,
    session_id: &str,
    timeout_secs: u64,
) -> Result<CmdOutput, CtlError> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let (session, raw) = fetch_session(client, session_id)?;
        match session.status.as_str() {
            "queued" | "running" => {
                if Instant::now() >= deadline {
                    return Err(CtlError::command(
                        "wait_timeout",
                        format!(
                            "session {session_id} is still {} after {timeout_secs}s; it keeps \
                             running (waiting never cancels) — wait again or check \
                             `stagedctl session status {session_id}`",
                            session.status
                        ),
                    ));
                }
                std::thread::sleep(
                    WAIT_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            "completed" => {
                return Ok(CmdOutput {
                    human: format!(
                        "session {session_id} completed{}",
                        session
                            .completion_reason
                            .as_deref()
                            .map(|r| format!(" ({r})"))
                            .unwrap_or_default()
                    ),
                    json: raw,
                });
            }
            "error" => {
                return Err(CtlError::command(
                    "session_error",
                    session
                        .error_message
                        .filter(|e| !e.is_empty())
                        .unwrap_or_else(|| format!("session {session_id} ended with an error")),
                ));
            }
            "cancelled" => {
                return Err(CtlError::command(
                    "session_cancelled",
                    format!("session {session_id} was cancelled"),
                ));
            }
            other => {
                // Unknown terminal-looking status: surface it rather than
                // spinning forever against a newer app.
                return Ok(CmdOutput {
                    human: format!("session {session_id} ended with status {other}"),
                    json: raw,
                });
            }
        }
    }
}

fn session_cancel(client: &Client, session_id: &str) -> Result<CmdOutput, CtlError> {
    client.invoke("cancel_session", json!({"sessionId": session_id}))?;
    Ok(CmdOutput {
        json: json!({"sessionId": session_id, "cancelled": true}),
        human: format!("session {session_id} cancelled"),
    })
}

fn session_list(
    client: &Client,
    project_flag: Option<&str>,
    repo_flag: Option<&str>,
) -> Result<CmdOutput, CtlError> {
    let raw = client.invoke("get_active_sessions", json!({}))?;
    let sessions: Vec<ActiveSessionRow> = parse(&raw, "active session list")?;

    let projects_raw = client.invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&projects_raw, "project list")?;
    let project_names: HashMap<&str, &str> = projects
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();

    let mut project_filter: Option<String> = None;
    let mut branch_filter: Option<HashSet<String>> = None;
    let mut branch_names: HashMap<String, String> = HashMap::new();
    if let Some(needle) = project_flag {
        let project = find_project(&projects, needle)?.clone();
        let branches_raw = client.invoke(
            "list_branches_for_project",
            json!({"projectId": project.id}),
        )?;
        let branches: Vec<BranchRow> = parse(&branches_raw, "branch list")?;
        for branch in &branches {
            branch_names.insert(branch.id.clone(), branch.branch_name.clone());
        }
        if let Some(repo_flag) = repo_flag {
            let address = RepoAddress::parse(repo_flag)?;
            let repos_raw =
                client.invoke("list_project_repos", json!({"projectId": project.id}))?;
            let repos: Vec<RepoRow> = parse(&repos_raw, "project repo list")?;
            let repo = repos
                .iter()
                .find(|r| {
                    r.github_repo.eq_ignore_ascii_case(&address.github_repo)
                        && r.subpath.as_deref() == address.subpath.as_deref()
                })
                .ok_or_else(|| {
                    CtlError::command(
                        "not_found",
                        format!(
                            "project {:?} has no repo {}",
                            project.name,
                            repo_address(&address.github_repo, address.subpath.as_deref())
                        ),
                    )
                })?;
            branch_filter = Some(
                branches
                    .iter()
                    .filter(|b| b.project_repo_id.as_deref() == Some(repo.id.as_str()))
                    .map(|b| b.id.clone())
                    .collect(),
            );
        }
        project_filter = Some(project.id);
    }

    let empty = Vec::new();
    let raw_rows = raw.as_array().unwrap_or(&empty);
    let mut kept_raw = Vec::new();
    let mut lines = Vec::new();
    for (session, raw_row) in sessions.iter().zip(raw_rows) {
        if let Some(project_id) = &project_filter {
            if session.project_id.as_deref() != Some(project_id.as_str()) {
                continue;
            }
        }
        if let Some(branch_ids) = &branch_filter {
            if !session
                .branch_id
                .as_deref()
                .is_some_and(|id| branch_ids.contains(id))
            {
                continue;
            }
        }
        let project = session
            .project_id
            .as_deref()
            .map(|id| project_names.get(id).copied().unwrap_or(id))
            .unwrap_or("-");
        let branch = session
            .branch_id
            .as_deref()
            .map(|id| branch_names.get(id).map(String::as_str).unwrap_or(id))
            .unwrap_or("-");
        lines.push(format!(
            "{}  {}  {}  project:{}  branch:{}",
            session.session_id,
            session.status,
            session.session_type.as_deref().unwrap_or("-"),
            project,
            branch
        ));
        kept_raw.push(raw_row.clone());
    }

    Ok(CmdOutput {
        json: Value::Array(kept_raw),
        human: if lines.is_empty() {
            "no queued or running sessions".to_string()
        } else {
            lines.join("\n")
        },
    })
}

// =============================================================================
// image
// =============================================================================

fn image_add(
    client: &Client,
    project_needle: &str,
    repo_flag: &str,
    branch_flag: Option<&str>,
    file: &std::path::Path,
) -> Result<CmdOutput, CtlError> {
    let file = std::fs::canonicalize(file).map_err(|e| {
        CtlError::command(
            "not_found",
            format!("cannot read image file {}: {e}", file.display()),
        )
    })?;
    let project = resolve_project(client, project_needle)?;
    let resolved = resolve_branch(client, &project, repo_flag, branch_flag)?;

    // pending:false is what lands the image in the timeline instead of a
    // session-attachment limbo.
    let raw = client.invoke(
        "create_image",
        json!({
            "branchId": resolved.branch.id,
            "projectId": project.id,
            "filePath": file.to_string_lossy(),
            "pending": false,
        }),
    )?;
    let image: ImageResponse = parse(&raw, "image")?;

    Ok(CmdOutput {
        human: format!(
            "image {} added to {} (branch {}): {} ({} bytes)",
            image.id,
            resolved.address,
            resolved.branch.branch_name,
            image.filename,
            image.size_bytes
        ),
        json: raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // Repo address parsing
    // -------------------------------------------------------------------

    #[test]
    fn repo_address_parses_with_and_without_subpath() {
        let plain = RepoAddress::parse("block/builderbot").unwrap();
        assert_eq!(plain.github_repo, "block/builderbot");
        assert_eq!(plain.subpath, None);

        let with_subpath = RepoAddress::parse("block/builderbot:apps/staged").unwrap();
        assert_eq!(with_subpath.github_repo, "block/builderbot");
        assert_eq!(with_subpath.subpath.as_deref(), Some("apps/staged"));

        // A trailing colon means "no subpath".
        let trailing = RepoAddress::parse("block/builderbot:").unwrap();
        assert_eq!(trailing.subpath, None);
    }

    #[test]
    fn repo_address_rejects_malformed_input() {
        for bad in ["builderbot", "", ":apps/staged", "   "] {
            let err = RepoAddress::parse(bad).unwrap_err();
            assert_eq!(err.exit_code(), 1, "{bad:?} should be a command error");
            assert!(err.to_string().starts_with("invalid_arguments:"));
        }
    }

    // -------------------------------------------------------------------
    // Project matching
    // -------------------------------------------------------------------

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            github_repo: None,
            subpath: None,
            location: None,
        }
    }

    #[test]
    fn find_project_prefers_id_then_name_then_loose_name() {
        let projects = vec![
            project("p1", "Alpha"),
            project("p2", "beta"),
            project("p3", "p1"),
        ];
        // Exact id wins even when another project is *named* that string.
        assert_eq!(find_project(&projects, "p1").unwrap().id, "p1");
        assert_eq!(find_project(&projects, "beta").unwrap().id, "p2");
        assert_eq!(find_project(&projects, "ALPHA").unwrap().id, "p1");
    }

    #[test]
    fn find_project_errors_list_alternatives() {
        let projects = vec![project("p1", "Alpha")];
        let err = find_project(&projects, "nope").unwrap_err();
        assert_eq!(err.exit_code(), 1);
        assert!(
            err.to_string().contains("Alpha"),
            "should list projects: {err}"
        );
    }

    // -------------------------------------------------------------------
    // Timeline rendering
    // -------------------------------------------------------------------

    #[test]
    fn timeline_lines_merge_chronologically_with_hashtag_refs() {
        let timeline = TimelineWire {
            commits: vec![CommitItem {
                sha: "abc123def456".into(),
                subject: "add feature".into(),
                author: "Matt".into(),
                timestamp: 200,
                sort_timestamp: 200,
                order: 0,
                ..Default::default()
            }],
            notes: vec![NoteItem {
                id: "n1".into(),
                title: "findings".into(),
                created_at: 100,
                ..Default::default()
            }],
            reviews: vec![ReviewItem {
                id: "r1".into(),
                commit_sha: "abc123def456".into(),
                scope: "commit".into(),
                comment_count: 3,
                created_at: 300,
                ..Default::default()
            }],
            images: vec![],
        };
        let lines = timeline_lines(&timeline, "");
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("#note:n1  findings"), "{lines:?}");
        assert!(
            lines[1].starts_with("#commit:abc123def456  add feature"),
            "{lines:?}"
        );
        assert!(
            lines[2]
                .starts_with("#review:r1  review of #commit:abc123de  (commit scope, 3 comments)"),
            "{lines:?}"
        );
    }

    #[test]
    fn pending_commits_and_running_sessions_are_labelled() {
        let timeline = TimelineWire {
            commits: vec![CommitItem {
                id: Some("c9".into()),
                sha: String::new(),
                subject: "fix the bug".into(),
                session_id: Some("s1".into()),
                session_status: Some("running".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let lines = timeline_lines(&timeline, "");
        assert_eq!(
            lines,
            vec!["pending commit c9  fix the bug  [session s1 running]"]
        );
    }

    #[test]
    fn completed_sessions_get_no_suffix() {
        assert_eq!(session_suffix(Some("s1"), Some("completed"), None), "");
        assert_eq!(session_suffix(None, None, None), "");
        assert_eq!(
            session_suffix(Some("s1"), Some("error"), Some("crashed")),
            "  [session s1 error: crashed]"
        );
    }

    #[test]
    fn truncate_flattens_whitespace_and_bounds_length() {
        assert_eq!(truncate("a\nb\tc", 100), "a b c");
        let long = "x".repeat(200);
        let out = truncate(&long, 160);
        assert_eq!(out.chars().count(), 161, "160 chars plus ellipsis");
        assert!(out.ends_with('…'));
    }

    #[test]
    fn timestamps_render_as_rfc3339() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
    }

    // -------------------------------------------------------------------
    // Pre-connect flag-pairing validation
    // -------------------------------------------------------------------

    fn parse_cli(args: &[&str]) -> Noun {
        use clap::Parser;
        Cli::try_parse_from(args).unwrap().command
    }

    #[test]
    fn validate_enforces_flag_pairings_before_discovery() {
        let cases: &[(&[&str], &str)] = &[
            (
                &["stagedctl", "timeline", "show", "p", "--branch", "b"],
                "--branch only applies",
            ),
            (
                &[
                    "stagedctl",
                    "session",
                    "start",
                    "p",
                    "--repo",
                    "o/r",
                    "--prompt",
                    "x",
                ],
                "--type is required",
            ),
            (
                &[
                    "stagedctl",
                    "session",
                    "start",
                    "p",
                    "--type",
                    "note",
                    "--prompt",
                    "x",
                ],
                "--type only applies",
            ),
            (
                &[
                    "stagedctl",
                    "session",
                    "start",
                    "p",
                    "--branch",
                    "b",
                    "--prompt",
                    "x",
                ],
                "--branch only applies",
            ),
            (
                &["stagedctl", "session", "start", "p", "--prompt", "  "],
                "--prompt must not be empty",
            ),
            (
                &["stagedctl", "session", "list", "--repo", "o/r"],
                "--repo requires --project",
            ),
        ];
        for (args, expected) in cases {
            let err = validate(&parse_cli(args)).unwrap_err();
            assert_eq!(err.exit_code(), 1, "{args:?}");
            let message = err.to_string();
            assert!(
                message.starts_with("invalid_arguments:") && message.contains(expected),
                "{args:?} should mention {expected:?}, got: {message}"
            );
        }
    }

    #[test]
    fn validate_accepts_well_paired_flags() {
        let good: &[&[&str]] = &[
            &["stagedctl", "project", "list"],
            &["stagedctl", "timeline", "show", "p"],
            &[
                "stagedctl",
                "timeline",
                "show",
                "p",
                "--repo",
                "o/r",
                "--branch",
                "b",
            ],
            &["stagedctl", "session", "start", "p", "--prompt", "x"],
            &[
                "stagedctl",
                "session",
                "start",
                "p",
                "--repo",
                "o/r",
                "--type",
                "review",
                "--prompt",
                "x",
            ],
            &[
                "stagedctl",
                "session",
                "list",
                "--project",
                "p",
                "--repo",
                "o/r",
            ],
        ];
        for args in good {
            assert!(validate(&parse_cli(args)).is_ok(), "{args:?}");
        }
    }
}
