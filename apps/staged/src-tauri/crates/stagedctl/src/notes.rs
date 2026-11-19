//! The `note move` and `note copy` writes: how a `<NOTE>` is read, the
//! request each sends, and the rendering of where the note landed.
//!
//! Like [`crate::commands`], this is a flag→JSON mapper. The transfer is the
//! app's (`note_transfer` in the app), the one its "Move to project" and
//! "Copy to project" menus run: what a move carries, what a copy leaves
//! behind, which repo the destination gains, and every refusal are decided
//! there and pass through verbatim. Two things are decided here: which of
//! the app's two kinds of note a reference names, and which project `--to`
//! names, answered from a read the caller can make anyway.
//!
//! Each verb is one write with no read after it. The app's answer carries
//! the branch the note landed on, the child notes a moved project note
//! took along, and the repos the transfer added, which only the transfer
//! knows. An answer that cannot be read is a [`CtlError::Partial`], so the
//! caller still learns the write landed.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::{Client, CtlError};
use crate::commands::{find_project, parse, repo_address, BranchRow, CmdOutput, Project};

/// The dispatch commands behind the two verbs. A Staged from before they
/// reached the control server refuses them as not allowlisted (404), which
/// on the wire is the answer to any unknown command, so the transport
/// appends an "update Staged" hint to exactly these.
pub const NOTE_TRANSFER_COMMANDS: &[&str] = &[
    "move_note_to_project",
    "copy_note_to_project",
    "move_project_note_to_project",
    "copy_project_note_to_project",
];

pub fn is_note_transfer_command(command: &str) -> bool {
    NOTE_TRANSFER_COMMANDS.contains(&command)
}

/// Which of the two verbs is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Move,
    Copy,
}

impl Transfer {
    /// The value of `mode` under `--json`, which is the verb.
    fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Copy => "copy",
        }
    }

    fn past(self) -> &'static str {
        match self {
            Self::Move => "moved",
            Self::Copy => "copied",
        }
    }
}

// =============================================================================
// The <NOTE> argument
// =============================================================================

/// A `<NOTE>` argument: a hashtag reference as `timeline show` prints one.
/// The two kinds are separate tables in the app with separate commands, so
/// the kind is part of the address and a bare id is not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteRef {
    /// `#note:<id>`: a note on a repo's branch.
    Branch(String),
    /// `#project-note:<id>`: a note on the project itself.
    Project(String),
}

impl NoteRef {
    /// Read `#note:<id>` or `#project-note:<id>`. The leading `#` may be
    /// left off: unquoted, a shell reads it as the start of a comment, and
    /// the argument never arrives.
    pub fn parse(input: &str) -> Result<Self, CtlError> {
        let invalid = |why: &str| {
            CtlError::command(
                "invalid_arguments",
                format!(
                    "{why}: <NOTE> must be '#note:<id>' or '#project-note:<id>', got {input:?} \
                     (copy one from `stagedctl timeline show`, and quote it: an unquoted # \
                     starts a shell comment)"
                ),
            )
        };
        let reference = input.trim();
        let reference = reference.strip_prefix('#').unwrap_or(reference);
        let Some((kind, id)) = reference.split_once(':') else {
            return Err(invalid("not a hashtag reference"));
        };
        if id.is_empty() || id.chars().any(char::is_whitespace) {
            return Err(invalid("the reference has no id"));
        }
        match kind {
            "note" => Ok(Self::Branch(id.to_string())),
            "project-note" => Ok(Self::Project(id.to_string())),
            "review" | "commit" => Err(invalid(
                "only notes move between projects; commits and reviews stay on their branch",
            )),
            _ => Err(invalid("not a note reference")),
        }
    }

    fn id(&self) -> &str {
        match self {
            Self::Branch(id) | Self::Project(id) => id,
        }
    }

    /// What the output calls this kind of note.
    fn kind(&self) -> &'static str {
        match self {
            Self::Branch(_) => "note",
            Self::Project(_) => "project note",
        }
    }

    /// The hashtag reference of the note `id` of this kind. A copy is a
    /// note of the same kind as its original.
    fn reference_to(&self, id: &str) -> String {
        match self {
            Self::Branch(_) => format!("#note:{id}"),
            Self::Project(_) => format!("#project-note:{id}"),
        }
    }

    fn command(&self, transfer: Transfer) -> &'static str {
        match (self, transfer) {
            (Self::Branch(_), Transfer::Move) => "move_note_to_project",
            (Self::Branch(_), Transfer::Copy) => "copy_note_to_project",
            (Self::Project(_), Transfer::Move) => "move_project_note_to_project",
            (Self::Project(_), Transfer::Copy) => "copy_project_note_to_project",
        }
    }

    /// The request, under the names the app's transfer commands take.
    fn args(&self, target_project_id: &str) -> Value {
        match self {
            Self::Branch(id) => json!({ "noteId": id, "targetProjectId": target_project_id }),
            Self::Project(id) => {
                json!({ "projectNoteId": id, "targetProjectId": target_project_id })
            }
        }
    }
}

// =============================================================================
// The app's answer
// =============================================================================

/// What the app answers a control caller: the note as it stands after the
/// transfer and the repos the transfer added to the destination. For a
/// branch note, also the branch it landed on and the repo that branch
/// tracks. Either is `null` when the app could not read the row back after
/// the transfer committed. For a project note, also the child notes that
/// came with it, which is none for a copy.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Landed {
    note: LandedNote,
    #[serde(default)]
    branch: Option<BranchRow>,
    #[serde(default)]
    repo: Option<RepoName>,
    #[serde(default)]
    children: Vec<LandedChild>,
    #[serde(default)]
    added_repos: Vec<RepoName>,
}

/// A child note that moved with its project note: its hashtag reference,
/// which the move does not change, and where it is now, as for a branch
/// note.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LandedChild {
    reference: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    branch: Option<BranchRow>,
    #[serde(default)]
    repo: Option<RepoName>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LandedNote {
    id: String,
    #[serde(default)]
    title: String,
}

/// A repo by its address. An added repo is answered as this alone.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoName {
    github_repo: String,
    #[serde(default)]
    subpath: Option<String>,
}

impl RepoName {
    fn address(&self) -> String {
        repo_address(&self.github_repo, self.subpath.as_deref())
    }
}

// =============================================================================
// note move / note copy
// =============================================================================

pub fn note_transfer(
    client: &Client,
    transfer: Transfer,
    note: &NoteRef,
    to: &str,
) -> Result<CmdOutput, CtlError> {
    transfer_with(
        |command, args| client.invoke(command, args),
        transfer,
        note,
        to,
    )
}

/// [`note_transfer`] over any `invoke`, so it can be tested without a
/// Client.
///
/// `--to` resolves among the projects `list_projects` returns, which for a
/// session is its own project and the ones it created. Anything else is
/// `not_found`, by name or by id, and the app is asked nothing more. The
/// note is not looked up first: the app finds it, checks that its project
/// is within the credential's reach, and answers for both.
pub(crate) fn transfer_with(
    mut invoke: impl FnMut(&str, Value) -> Result<Value, CtlError>,
    transfer: Transfer,
    note: &NoteRef,
    to: &str,
) -> Result<CmdOutput, CtlError> {
    let projects_raw = invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&projects_raw, "project list")?;
    let project = find_project(&projects, to)?.clone();

    let command = note.command(transfer);
    let args = note.args(&project.id);
    let answer = invoke(command, args.clone())?;

    match parse::<Landed>(&answer, "note transfer answer") {
        Ok(landed) => Ok(render(transfer, note, &project, &landed, &answer)),
        Err(error) => {
            let human = format!(
                "{} {} {} to project {:?}\n  ({}; the app's answer could not be read)\n\
                 next: stagedctl timeline show {}",
                note.kind(),
                note.reference_to(note.id()),
                transfer.past(),
                project.name,
                transfer.past(),
                project.id
            );
            Err(CtlError::Partial {
                error: Box::new(error),
                applied: vec![json!({ "command": command, "args": args, "result": answer })],
                human,
            })
        }
    }
}

/// The output of both verbs: the note's reference, the project it is in
/// now, the branch it landed on, each child note that came with a project
/// note and the branch that landed on, and each repo the transfer added.
/// `--json` is `{ ok, mode, projectId, reference, note, addedRepos }` with
/// the app's rows. A branch note has `branch` and `repo` as well. A project
/// note has `children`, each `{ reference, title, branch, repo }`.
/// `reference` is the hashtag reference of the note where it is now: the
/// same note after a move, the new one after a copy.
fn render(
    transfer: Transfer,
    note: &NoteRef,
    project: &Project,
    landed: &Landed,
    answer: &Value,
) -> CmdOutput {
    let source = note.reference_to(note.id());
    let mut human = vec![match transfer {
        Transfer::Move => format!(
            "{} {source} moved to project {:?}",
            note.kind(),
            project.name
        ),
        Transfer::Copy => format!(
            "{} {source} copied to project {:?} as {}",
            note.kind(),
            project.name,
            note.reference_to(&landed.note.id)
        ),
    }];
    human.push(format!("  project id: {}", project.id));
    if !landed.note.title.trim().is_empty() {
        human.push(format!("  title: {}", landed.note.title));
    }
    let address = landed.repo.as_ref().map(RepoName::address);
    if let Some(place) = landing(landed.branch.as_ref(), address.as_deref()) {
        human.push(format!("  landed on: {place}"));
    }
    for child in &landed.children {
        let mut line = format!("  child note {}", child.reference);
        if !child.title.trim().is_empty() {
            line.push_str(&format!(" {:?}", child.title));
        }
        let address = child.repo.as_ref().map(RepoName::address);
        if let Some(place) = landing(child.branch.as_ref(), address.as_deref()) {
            line.push_str(&format!(" landed on: {place}"));
        }
        human.push(line);
    }
    for repo in &landed.added_repos {
        human.push(format!(
            "  added repo: {} (the project did not track it; its branch is set up in the \
             background)",
            repo.address()
        ));
    }
    human.push(match (&landed.branch, &address) {
        (Some(branch), Some(address)) => format!(
            "next: stagedctl timeline show {} --repo {address} --branch {}",
            project.id, branch.branch_name
        ),
        _ => format!("next: stagedctl timeline show {}", project.id),
    });

    let mut json = Map::new();
    json.insert("ok".into(), json!(true));
    json.insert("mode".into(), json!(transfer.as_str()));
    json.insert("projectId".into(), json!(project.id));
    json.insert(
        "reference".into(),
        json!(note.reference_to(&landed.note.id)),
    );
    for key in ["note", "branch", "repo", "children", "addedRepos"] {
        if let Some(value) = answer.get(key) {
            json.insert(key.into(), value.clone());
        }
    }
    CmdOutput {
        json: Value::Object(json),
        human: human.join("\n"),
    }
}

/// Where a note is, as the output says it: the repo's address and the
/// branch, or the branch alone when the app could not read the repo back.
/// `None` when it could not read the branch.
fn landing(branch: Option<&BranchRow>, address: Option<&str>) -> Option<String> {
    let branch = branch?;
    Some(match address {
        Some(address) => format!("{address}, branch {}", branch.branch_name),
        None => format!("branch {}", branch.branch_name),
    })
}

#[cfg(test)]
mod tests;
