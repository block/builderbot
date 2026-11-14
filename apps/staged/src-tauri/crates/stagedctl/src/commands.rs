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

use crate::cli::{
    Cli, ImageVerb, Noun, ProjectVerb, SessionType, SessionVerb, TagGroupVerb, TagVerb,
    TimelineVerb,
};
use crate::client::{
    default_data_dir, discover_and_connect, discover_instance, session_credential_in_use, Approval,
    Client, CtlError, STAGEDCTL_LOCK_ENV,
};
use crate::grants::{self, GrantFile};
use crate::launch;
use crate::open;
use crate::tags;

/// One command's result: the value `--json` prints, and the human rendering.
#[derive(Debug)]
pub struct CmdOutput {
    pub json: Value,
    pub human: String,
}

pub fn run(cli: Cli) -> Result<CmdOutput, CtlError> {
    validate(&cli.command)?;
    let data_dir = default_data_dir();

    // The credential verbs come before the credential lookup: `connect`
    // is how a grant is obtained, `disconnect` must work with a grant the
    // app no longer accepts. Neither is available to a spawned session,
    // whose credential is its own and confined; the app refuses the same
    // way, but the answer is known here without a round trip.
    match &cli.command {
        Noun::Connect { .. } | Noun::Disconnect { .. } => {
            if let Some(path) = session_credential_in_use(
                cli.lock_path.as_deref(),
                std::env::var_os(STAGEDCTL_LOCK_ENV),
                data_dir.as_deref(),
            ) {
                return Err(session_enrolment_refusal(&cli.command, &path));
            }
        }
        _ => {}
    }
    match &cli.command {
        Noun::Connect {
            client,
            project,
            read_only,
            timeout,
        } => {
            let data_dir = data_dir.as_deref().ok_or_else(|| {
                CtlError::Environment("cannot determine the home directory".into())
            })?;
            let (app, _) = discover_instance(
                cli.lock_path.as_deref(),
                std::env::var_os(STAGEDCTL_LOCK_ENV),
                Some(data_dir),
            )?;
            return connect(
                &app,
                data_dir,
                client,
                project.as_deref(),
                *read_only,
                Duration::from_secs(*timeout),
            );
        }
        Noun::Disconnect { client } => {
            let data_dir = data_dir.as_deref().ok_or_else(|| {
                CtlError::Environment("cannot determine the home directory".into())
            })?;
            let app = discover_instance(
                cli.lock_path.as_deref(),
                std::env::var_os(STAGEDCTL_LOCK_ENV),
                Some(data_dir),
            )
            .ok()
            .map(|(app, _)| app);
            return disconnect(app.as_ref(), data_dir, client.as_deref());
        }
        _ => {}
    }

    // `project open` finds its own app: it is the one verb that starts
    // Staged when none is running (see `launch`), and it settles what it was
    // pointed at before looking for one.
    if let Noun::Project(ProjectVerb::Open { project, no_launch }) = &cli.command {
        return open::run(
            project.as_deref(),
            *no_launch,
            cli.lock_path.as_deref(),
            std::env::var_os(STAGEDCTL_LOCK_ENV),
            std::env::current_dir,
            data_dir.as_deref(),
            &launch::LaunchEnv::real(),
        );
    }

    let client = discover_and_connect(
        cli.lock_path.as_deref(),
        std::env::var_os(STAGEDCTL_LOCK_ENV),
        data_dir.as_deref(),
    )?;

    match cli.command {
        Noun::Project(ProjectVerb::List { tag }) => project_list(&client, &tag),
        Noun::Project(ProjectVerb::Get { project }) => project_get(&client, &project),
        Noun::Project(ProjectVerb::Tags { project }) => tags::project_tags(&client, &project),
        Noun::Project(ProjectVerb::Tag {
            project,
            tags,
            create,
        }) => tags::project_tag(&client, &project, &tags, create),
        Noun::Project(ProjectVerb::Untag {
            project,
            tags,
            group,
        }) => tags::project_untag(&client, &project, &tags, group.as_deref()),
        Noun::Tag(TagVerb::List { group }) => tags::tag_list(&client, group.as_deref()),
        Noun::Tag(TagVerb::Show { tag }) => tags::tag_show(&client, &tag),
        Noun::Tag(TagVerb::Create {
            name,
            group,
            color,
            icon,
        }) => tags::tag_create(&client, &name, group.as_deref(), color, icon.as_deref()),
        Noun::Tag(TagVerb::Edit {
            tag,
            name,
            color,
            icon,
            no_icon,
        }) => {
            let icon = match (icon.as_deref(), no_icon) {
                (_, true) => tags::IconChange::Remove,
                (Some(icon), false) => tags::IconChange::Set(icon),
                (None, false) => tags::IconChange::Keep,
            };
            tags::tag_edit(&client, &tag, name.as_deref(), color, icon)
        }
        Noun::Tag(TagVerb::Move {
            tag,
            group,
            index,
            yes,
            ..
        }) => tags::tag_move(&client, &tag, group.as_deref(), index, yes),
        Noun::Tag(TagVerb::Group(TagGroupVerb::List)) => tags::tag_group_list(&client),
        Noun::Tag(TagVerb::Group(TagGroupVerb::Create { name, template })) => {
            tags::tag_group_create(&client, &name, template)
        }
        Noun::Tag(TagVerb::Group(TagGroupVerb::Rename { group, name })) => {
            tags::tag_group_rename(&client, &group, &name)
        }
        Noun::Tag(TagVerb::Group(TagGroupVerb::SetIcon { group, .. })) => {
            tags::tag_group_set_icon(&client, group.as_deref())
        }
        Noun::Tag(TagVerb::Group(TagGroupVerb::SetSidebar { group, on, .. })) => {
            tags::tag_group_set_sidebar(&client, &group, on)
        }
        Noun::Tag(TagVerb::Group(TagGroupVerb::Reorder { groups })) => {
            tags::tag_group_reorder(&client, &groups)
        }
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
        Noun::Project(ProjectVerb::Open { .. })
        | Noun::Connect { .. }
        | Noun::Disconnect { .. } => {
            unreachable!("handled above")
        }
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
        Noun::Project(ProjectVerb::Untag { tags, group, .. }) => match (tags.is_empty(), group) {
            (false, Some(_)) => invalid(
                "--group clears the project's whole choice in that group and takes no <tag> \
                 arguments; name the tags to remove, or pass --group alone",
            ),
            (true, None) => invalid("name at least one <tag> to remove, or pass --group"),
            _ => Ok(()),
        },
        Noun::Tag(TagVerb::Edit {
            name,
            color,
            icon,
            no_icon,
            ..
        }) if name.is_none() && color.is_none() && icon.is_none() && !no_icon => {
            invalid("nothing to change: pass --name, --color, --icon or --no-icon")
        }
        Noun::Tag(TagVerb::Edit {
            name: Some(name), ..
        }) if name.trim().is_empty() => invalid("--name must not be empty"),
        Noun::Tag(TagVerb::Move { group, ungroup, .. }) if group.is_none() && !ungroup => invalid(
            "pass --group <group> to move the tag into a group (its own group, with --index, \
             to reorder it there), or --ungroup to take it out of its group",
        ),
        Noun::Tag(TagVerb::Group(TagGroupVerb::Rename { name, .. })) if name.trim().is_empty() => {
            invalid("the new name must not be empty")
        }
        Noun::Tag(TagVerb::Group(TagGroupVerb::SetIcon { group, none })) => match (group, none) {
            (None, false) => invalid("name a <group> to drive the project icon, or pass --none"),
            _ => Ok(()),
        },
        Noun::Tag(TagVerb::Group(TagGroupVerb::SetSidebar { on, off, .. })) if !on && !off => {
            invalid("pass --on to pin the group's board to the sidebar, or --off to unpin it")
        }
        Noun::Connect { client, .. } if client.trim().is_empty() => {
            invalid("--client must not be empty")
        }
        Noun::Connect { client, .. } if client.chars().count() > 64 => {
            invalid("--client must be at most 64 characters")
        }
        _ => Ok(()),
    }
}

// =============================================================================
// connect / disconnect
// =============================================================================

/// How often to re-issue the long poll while the user has not answered.
const CONNECT_POLL_PAUSE: Duration = Duration::from_millis(250);

/// Exit 4 for `connect`/`disconnect` under a per-session credential. Opens
/// with the sentence the app's own refusal uses (pinned on both sides), so
/// an agent reads one explanation whichever side answers.
fn session_enrolment_refusal(command: &Noun, path: &std::path::Path) -> CtlError {
    let verb = match command {
        Noun::Disconnect { .. } => "disconnect",
        _ => "connect",
    };
    CtlError::Authorization {
        code: "scope_denied".to_string(),
        message: format!(
            "spawned sessions cannot request connected-app access: `stagedctl {verb}` is for \
             clients the user runs outside a Staged session, and this invocation runs inside \
             one ({} is the session's own credential, confined to its project). Use that \
             credential as is; if the task needs wider access, ask the user to run `stagedctl \
             connect` from their own terminal",
            path.display()
        ),
    }
}

fn connect(
    app: &Client,
    data_dir: &std::path::Path,
    client_name: &str,
    project: Option<&str>,
    read_only: bool,
    timeout: Duration,
) -> Result<CmdOutput, CtlError> {
    let (request_id, _) = app.request_authorization(client_name.trim(), project, read_only)?;
    eprintln!("Waiting for approval in Staged… (request {request_id})");
    let approval = wait_for_approval(
        || app.poll_authorization(&request_id),
        Instant::now() + timeout,
        timeout,
        CONNECT_POLL_PAUSE,
    )?;
    let path = grants::write_grant_file(data_dir, &approval.grant)?;
    Ok(render_connected(&approval.grant, &path))
}

/// The `connect` wait: re-issue `poll` (one long poll of the request) until
/// it yields the approval, fails, or `deadline` passes with the user still
/// silent, which is the `connect_timeout` command error (exit 1). `timeout`
/// is only quoted in that message.
fn wait_for_approval<P>(
    mut poll: P,
    deadline: Instant,
    timeout: Duration,
    pause: Duration,
) -> Result<Approval, CtlError>
where
    P: FnMut() -> Result<Option<Approval>, CtlError>,
{
    loop {
        match poll()? {
            Some(approval) => return Ok(approval),
            None if Instant::now() >= deadline => {
                return Err(CtlError::command(
                    "connect_timeout",
                    format!(
                        "no answer from the Staged user within {} seconds; the request stays \
                         in the app until it expires",
                        timeout.as_secs()
                    ),
                ));
            }
            None => std::thread::sleep(pause),
        }
    }
}

fn render_connected(grant: &GrantFile, path: &std::path::Path) -> CmdOutput {
    let json = json!({
        "grantId": grant.grant_id,
        "client": grant.client,
        "scope": { "projectId": grant.scope.project_id },
        "access": grant.access,
        "expiresAt": grant.expires_at,
        "grantFile": path.display().to_string(),
    });
    let human = format!(
        "connected as {} ({}, {} access); grant {} stored at {}\nexpires {} unless used \
         again; revoke with `stagedctl disconnect` or in Settings ▸ Connected apps",
        grant.client,
        grant.scope_label(),
        grant.access,
        grant.grant_id,
        path.display(),
        format_timestamp(grant.expires_at / 1000),
    );
    CmdOutput { json, human }
}

fn disconnect(
    app: Option<&Client>,
    data_dir: &std::path::Path,
    client_filter: Option<&str>,
) -> Result<CmdOutput, CtlError> {
    let grants: Vec<(std::path::PathBuf, GrantFile)> = grants::read_grant_files(data_dir)
        .into_iter()
        .filter(|(_, g)| client_filter.is_none() || client_filter == Some(g.client.as_str()))
        .collect();
    let mut removed = Vec::new();
    let mut unreachable = Vec::new();
    for (path, grant) in &grants {
        let revoked_in_app = match app {
            Some(app) => match app.revoke_grant(grant) {
                Ok(revoked) => Some(revoked),
                Err(e) => {
                    eprintln!(
                        "stagedctl: could not revoke grant {} in the app: {e}",
                        grant.grant_id
                    );
                    unreachable.push(grant.grant_id.clone());
                    None
                }
            },
            None => {
                unreachable.push(grant.grant_id.clone());
                None
            }
        };
        grants::remove_grant_file(path)?;
        removed.push(json!({
            "grantId": grant.grant_id,
            "client": grant.client,
            "revokedInApp": revoked_in_app,
        }));
    }
    let human = if grants.is_empty() {
        match client_filter {
            Some(c) => format!("no grants stored for client {c}"),
            None => "no grants stored on this machine".to_string(),
        }
    } else {
        let mut lines = vec![format!(
            "removed {} grant{}",
            grants.len(),
            if grants.len() == 1 { "" } else { "s" }
        )];
        for (_, grant) in &grants {
            lines.push(format!("  {} ({})", grant.grant_id, grant.client));
        }
        if !unreachable.is_empty() {
            lines.push(
                "some grants could not be revoked in the app (not running or unreachable); \
                 revoke them under Settings ▸ Connected apps if the app still lists them"
                    .to_string(),
            );
        }
        lines.join("\n")
    };
    Ok(CmdOutput {
        json: json!({ "removed": removed, "notRevokedInApp": unreachable }),
        human,
    })
}

// =============================================================================
// Wire shapes (camelCase, tolerant of extra fields)
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Project {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) github_repo: Option<String>,
    #[serde(default)]
    pub(crate) subpath: Option<String>,
    #[serde(default)]
    pub(crate) location: Option<String>,
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
    /// `session:<id>` / `app:<grant-id>` when a stagedctl caller started
    /// the session; absent for starts made in the app.
    #[serde(default)]
    started_by: Option<String>,
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

pub(crate) fn parse<T: serde::de::DeserializeOwned>(
    value: &Value,
    what: &str,
) -> Result<T, CtlError> {
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
pub(crate) fn find_project<'a>(
    projects: &'a [Project],
    needle: &str,
) -> Result<&'a Project, CtlError> {
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

/// Every project the caller's credential can see.
pub(crate) fn list_projects(client: &Client) -> Result<Vec<Project>, CtlError> {
    let raw = client.invoke("list_projects", json!({}))?;
    parse(&raw, "project list")
}

pub(crate) fn resolve_project(client: &Client, needle: &str) -> Result<Project, CtlError> {
    find_project(&list_projects(client)?, needle).cloned()
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

fn project_list(client: &Client, tag_filters: &[String]) -> Result<CmdOutput, CtlError> {
    let mut raw = client.invoke("list_projects", json!({}))?;
    let mut projects: Vec<Project> = parse(&raw, "project list")?;

    // --tag narrows client-side: one catalog read, then keep the projects
    // carrying every named tag. Without the flag the catalog isn't read, so
    // a plain `project list` still works against a Staged without tags.
    if !tag_filters.is_empty() {
        let state_raw = client.invoke("get_tag_state", json!({}))?;
        let state: tags::TagStateWire = parse(&state_raw, "tag state")?;
        let keep = tags::projects_with_all_tags(&state, tag_filters)?;
        projects.retain(|p| keep(&p.id));
        if let Some(rows) = raw.as_array_mut() {
            rows.retain(|row| row.get("id").and_then(Value::as_str).is_some_and(&keep));
        }
    }

    let human = if projects.is_empty() {
        if tag_filters.is_empty() {
            "no projects".to_string()
        } else {
            "no projects carry every tag named".to_string()
        }
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
    if let Some(actor) = session.started_by.as_deref().filter(|a| !a.is_empty()) {
        lines.push(format!("started by: {actor}"));
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
    wait_for_session(
        session_id,
        Duration::from_secs(timeout_secs),
        WAIT_POLL_INTERVAL,
        || fetch_session(client, session_id),
    )
}

/// The poll loop behind `session wait`, over an injected fetch so tests can
/// script a session's status sequence.
fn wait_for_session(
    session_id: &str,
    timeout: Duration,
    poll_interval: Duration,
    mut fetch: impl FnMut() -> Result<(SessionRow, Value), CtlError>,
) -> Result<CmdOutput, CtlError> {
    let timeout_secs = timeout.as_secs();
    let deadline = Instant::now() + timeout;
    loop {
        let (session, raw) = fetch()?;
        match session.status.as_str() {
            // Not finished. `disconnected` is a Blox session whose connection
            // dropped mid-turn: the remote turn may still be running, and
            // Staged's reconcile can reattach it to `running`.
            "queued" | "running" | "disconnected" => {
                if Instant::now() >= deadline {
                    return Err(if session.status == "disconnected" {
                        CtlError::command(
                            "session_disconnected",
                            format!(
                                "session {session_id} is still disconnected after \
                                 {timeout_secs}s: Staged lost its connection to the Blox \
                                 workstation, but the remote turn may still be running and \
                                 Staged reattaches it when the connection returns (waiting \
                                 never cancels) — wait again or check \
                                 `stagedctl session status {session_id}`"
                            ),
                        )
                    } else {
                        CtlError::command(
                            "wait_timeout",
                            format!(
                                "session {session_id} is still {} after {timeout_secs}s; it \
                                 keeps running (waiting never cancels) — wait again or check \
                                 `stagedctl session status {session_id}`",
                                session.status
                            ),
                        )
                    });
                }
                std::thread::sleep(
                    poll_interval.min(deadline.saturating_duration_since(Instant::now())),
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
                // A status this build doesn't know. Neither success (it may
                // not be finished at all) nor a reason to spin forever.
                return Err(CtlError::command(
                    "unknown_status",
                    format!(
                        "session {session_id} has status {other:?}, which this stagedctl \
                         doesn't recognize; it may not be finished — check \
                         `stagedctl session status {session_id}`"
                    ),
                ));
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
            "no queued, running, or disconnected sessions".to_string()
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
    // connect / disconnect
    // -------------------------------------------------------------------

    fn stored_grant(id: &str, client: &str) -> GrantFile {
        GrantFile {
            grant_id: id.to_string(),
            reconnect_secret: "s".to_string(),
            client: client.to_string(),
            scope: grants::GrantScope { project_id: None },
            access: "write".to_string(),
            expires_at: 1_700_000_000_000,
        }
    }

    fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    #[test]
    fn connect_and_disconnect_refuse_to_run_under_a_session_credential() {
        // The file need not exist, let alone answer: the refusal is by the
        // control file's name, before discovery.
        let session = std::path::PathBuf::from("/nonexistent/control/1/sessions/s-1-7.json");
        for args in [
            vec!["stagedctl", "connect", "--client", "Claude Code"],
            vec![
                "stagedctl",
                "connect",
                "--client",
                "x",
                "--project",
                "p",
                "--read-only",
            ],
            vec!["stagedctl", "disconnect"],
            vec!["stagedctl", "disconnect", "--client", "x"],
        ] {
            let mut cli = <Cli as clap::Parser>::try_parse_from(&args).unwrap();
            cli.lock_path = Some(session.clone());
            let err = run(cli).unwrap_err();
            assert_eq!(err.exit_code(), 4, "{args:?}: {err}");
            let text = err.to_string();
            assert!(
                text.starts_with(
                    "scope_denied: spawned sessions cannot request connected-app access"
                ),
                "{text}"
            );
            assert!(text.contains(&format!("`stagedctl {}`", args[1])), "{text}");
            assert!(text.contains("s-1-7.json"), "names the credential: {text}");
        }
    }

    #[test]
    fn connect_validates_the_client_name_before_discovery() {
        let connect = |client: &str| Noun::Connect {
            client: client.to_string(),
            project: None,
            read_only: false,
            timeout: 1,
        };
        assert!(validate(&connect("my-agent")).is_ok());
        let err = validate(&connect("   ")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid_arguments: --client must not be empty"
        );
        let err = validate(&connect(&"x".repeat(65))).unwrap_err();
        assert!(err.to_string().contains("at most 64"), "{err}");
        assert!(validate(&Noun::Disconnect { client: None }).is_ok());
    }

    /// The wait loop against an app that never approves: every poll comes
    /// back pending, the loop keeps asking until the deadline, then exits 1
    /// with `connect_timeout` (the request itself stays with the app). A
    /// poll that fails ends the wait with that error; one that approves
    /// ends it with the approval.
    #[test]
    fn connect_waits_until_the_deadline_then_reports_a_timeout() {
        let polls = std::cell::Cell::new(0usize);
        let never = || {
            polls.set(polls.get() + 1);
            Ok(None)
        };
        let err = wait_for_approval(
            never,
            Instant::now() + Duration::from_millis(30),
            Duration::from_secs(240),
            Duration::from_millis(5),
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 1, "{err}");
        assert_eq!(
            err.to_string(),
            "connect_timeout: no answer from the Staged user within 240 seconds; the request \
             stays in the app until it expires"
        );
        assert!(
            polls.get() >= 2,
            "kept polling until the deadline: {}",
            polls.get()
        );

        // A deadline already passed still polls once (the answer may be
        // waiting) and times out on `None`.
        polls.set(0);
        let err = wait_for_approval(
            never,
            Instant::now() - Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_millis(5),
        )
        .unwrap_err();
        assert_eq!(polls.get(), 1);
        assert!(err.to_string().starts_with("connect_timeout:"), "{err}");

        let approved = wait_for_approval(
            || {
                Ok(Some(Approval {
                    grant: stored_grant("g-1", "alpha"),
                    token: "live".to_string(),
                }))
            },
            Instant::now() - Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_millis(5),
        )
        .unwrap();
        assert_eq!(approved.token, "live");

        let err = wait_for_approval(
            || {
                Err(CtlError::Authorization {
                    code: "authorization_denied".to_string(),
                    message: "denied".to_string(),
                })
            },
            Instant::now() + Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_millis(5),
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 4, "a poll failure ends the wait: {err}");
    }

    #[test]
    fn disconnect_without_an_app_still_forgets_the_grants_and_honours_the_filter() {
        let dir = private_tempdir();
        grants::write_grant_file(dir.path(), &stored_grant("a-a", "alpha")).unwrap();
        grants::write_grant_file(dir.path(), &stored_grant("b-b", "beta")).unwrap();

        let out = disconnect(None, dir.path(), Some("alpha")).unwrap();
        assert_eq!(out.json["removed"].as_array().unwrap().len(), 1);
        assert_eq!(out.json["removed"][0]["grantId"], "a-a");
        assert_eq!(out.json["removed"][0]["revokedInApp"], Value::Null);
        assert_eq!(out.json["notRevokedInApp"], json!(["a-a"]));
        assert!(
            out.human.starts_with("removed 1 grant\n  a-a (alpha)"),
            "{}",
            out.human
        );
        assert!(
            out.human.contains("Settings ▸ Connected apps"),
            "{}",
            out.human
        );
        let left: Vec<String> = grants::read_grant_files(dir.path())
            .into_iter()
            .map(|(_, g)| g.grant_id)
            .collect();
        assert_eq!(left, ["b-b"]);

        let out = disconnect(None, dir.path(), None).unwrap();
        assert_eq!(out.json["removed"].as_array().unwrap().len(), 1);
        assert!(grants::read_grant_files(dir.path()).is_empty());

        let out = disconnect(None, dir.path(), None).unwrap();
        assert_eq!(out.human, "no grants stored on this machine");
        let out = disconnect(None, dir.path(), Some("alpha")).unwrap();
        assert_eq!(out.human, "no grants stored for client alpha");
    }

    #[test]
    fn a_fresh_connection_renders_the_grant_and_where_it_lives() {
        let mut grant = stored_grant("g-1", "my-agent");
        grant.scope.project_id = Some("p-1".to_string());
        grant.access = "read".to_string();
        let out = render_connected(&grant, std::path::Path::new("/tmp/x/g-1.json"));
        assert_eq!(out.json["grantId"], "g-1");
        assert_eq!(out.json["scope"]["projectId"], "p-1");
        assert_eq!(out.json["grantFile"], "/tmp/x/g-1.json");
        assert!(
            out.human
                .starts_with("connected as my-agent (project p-1, read access); grant g-1 stored at /tmp/x/g-1.json"),
            "{}",
            out.human
        );
        assert!(
            out.human.contains("expires 2023-11-14T22:13:20Z"),
            "{}",
            out.human
        );
    }

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
    // session wait
    // -------------------------------------------------------------------

    fn session_row(status: &str) -> (SessionRow, Value) {
        let raw = json!({"id": "s1", "status": status, "errorMessage": "agent crashed"});
        (parse(&raw, "session").unwrap(), raw)
    }

    /// Run the wait loop against a scripted status sequence, polling without
    /// sleeps. Returns the outcome and how many times the session was read.
    fn wait_over(statuses: &[&str], timeout: Duration) -> (Result<CmdOutput, CtlError>, usize) {
        let mut polls = 0;
        let result = wait_for_session("s1", timeout, Duration::ZERO, || {
            let status = statuses
                .get(polls)
                .unwrap_or_else(|| panic!("polled past the script {statuses:?}"));
            polls += 1;
            Ok(session_row(status))
        });
        (result, polls)
    }

    fn command_code(err: &CtlError) -> &'static str {
        match err {
            CtlError::Command { code, .. } => code,
            other => panic!("expected a command error, got {other:?}"),
        }
    }

    const LONG: Duration = Duration::from_secs(600);

    #[test]
    fn wait_polls_through_disconnected_until_the_session_finishes() {
        let script = [
            "queued",
            "running",
            "disconnected",
            "disconnected",
            "running",
            "completed",
        ];
        let (result, polls) = wait_over(&script, LONG);
        let output = result.expect("a completed session exits 0");
        assert_eq!(polls, script.len(), "disconnected must not end the wait");
        assert_eq!(output.human, "session s1 completed");

        // Reconcile can also settle a disconnected turn without reattaching.
        let (result, _) = wait_over(&["disconnected", "error"], LONG);
        assert_eq!(command_code(&result.unwrap_err()), "session_error");
    }

    #[test]
    fn wait_timeout_while_disconnected_has_its_own_code() {
        let (result, polls) = wait_over(&["disconnected"], Duration::ZERO);
        let err = result.unwrap_err();
        assert_eq!(polls, 1);
        assert_eq!(command_code(&err), "session_disconnected");
        assert_eq!(err.exit_code(), 1);
        assert!(
            err.to_string().contains("may still be running"),
            "the message should say the remote turn may continue: {err}"
        );
    }

    #[test]
    fn wait_timeout_while_queued_or_running_is_wait_timeout() {
        for status in ["queued", "running"] {
            let (result, _) = wait_over(&[status], Duration::ZERO);
            let err = result.unwrap_err();
            assert_eq!(command_code(&err), "wait_timeout", "{status}");
            assert!(err.to_string().contains(&format!("still {status}")));
        }
        // The code follows the status seen at the deadline, not earlier ones.
        let mut polls = 0;
        let start = Instant::now();
        let result = wait_for_session(
            "s1",
            Duration::from_millis(50),
            Duration::from_millis(5),
            || {
                polls += 1;
                let status = if polls == 1 {
                    "disconnected"
                } else {
                    "running"
                };
                Ok(session_row(status))
            },
        );
        assert!(start.elapsed() >= Duration::from_millis(50));
        assert_eq!(command_code(&result.unwrap_err()), "wait_timeout");
    }

    #[test]
    fn wait_reports_unknown_statuses_as_errors() {
        let (result, polls) = wait_over(&["running", "paused"], LONG);
        let err = result.unwrap_err();
        assert_eq!(
            polls, 2,
            "an unknown status ends the wait rather than spinning"
        );
        assert_eq!(command_code(&err), "unknown_status");
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("\"paused\""), "{err}");
    }

    #[test]
    fn wait_maps_terminal_statuses_to_their_exits() {
        let (result, _) = wait_over(&["completed"], Duration::ZERO);
        assert!(result.is_ok(), "completed exits 0 even at the deadline");

        let (result, _) = wait_over(&["error"], LONG);
        let err = result.unwrap_err();
        assert_eq!(command_code(&err), "session_error");
        assert_eq!(err.to_string(), "session_error: agent crashed");

        let (result, _) = wait_over(&["cancelled"], LONG);
        assert_eq!(command_code(&result.unwrap_err()), "session_cancelled");
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
            (
                &[
                    "stagedctl",
                    "project",
                    "untag",
                    "p",
                    "q4",
                    "--group",
                    "Status",
                ],
                "--group clears",
            ),
            (
                &["stagedctl", "project", "untag", "p"],
                "name at least one <tag>",
            ),
            (
                &["stagedctl", "tag", "edit", "Status/Done"],
                "nothing to change",
            ),
            (
                &["stagedctl", "tag", "edit", "Status/Done", "--name", " "],
                "--name must not be empty",
            ),
            (
                &["stagedctl", "tag", "move", "q4", "--index", "2"],
                "pass --group <group>",
            ),
            (
                &["stagedctl", "tag", "group", "rename", "Status", ""],
                "must not be empty",
            ),
            (&["stagedctl", "tag", "group", "set-icon"], "name a <group>"),
            (
                &["stagedctl", "tag", "group", "set-sidebar", "Status"],
                "pass --on",
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
            &["stagedctl", "tag", "edit", "Status/Done", "--no-icon"],
            &["stagedctl", "tag", "edit", "Status/Done", "--color", "red"],
            &["stagedctl", "tag", "move", "q4", "--ungroup"],
            &[
                "stagedctl",
                "tag",
                "move",
                "q4",
                "--group",
                "Status",
                "--index",
                "0",
                "--yes",
            ],
            &["stagedctl", "tag", "group", "set-icon", "--none"],
            &["stagedctl", "tag", "group", "set-icon", "Status"],
            &[
                "stagedctl",
                "tag",
                "group",
                "set-sidebar",
                "Status",
                "--off",
            ],
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
            &["stagedctl", "project", "tag", "p", "q4", "Status/Done"],
            &["stagedctl", "project", "tag", "p", "q4", "--create"],
            &["stagedctl", "project", "untag", "p", "q4", "Status/Done"],
            &["stagedctl", "project", "untag", "p", "--group", "Status"],
        ];
        for args in good {
            assert!(validate(&parse_cli(args)).is_ok(), "{args:?}");
        }
    }
}
