//! The `tag` noun and the tag side of `project`: wire shapes for the app's
//! `TagState`, `<tag>` / `<group>` address resolution, the renderers, and
//! the `project tag` / `project untag` write plans.
//!
//! Like [`crate::commands`], this is a flag→JSON mapper over allowlisted
//! dispatch commands. Everything here reads `get_tag_state` once and works
//! on that snapshot; the app does the real validation. The writes are
//! planned in full against that snapshot before the first call goes out,
//! so an argument the catalog can't answer writes nothing.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli::{GroupTemplate, TagColor};
use crate::client::{Client, CtlError};
use crate::commands::{find_project, parse, CmdOutput, Project};

/// The dispatch commands that only a Staged with tags answers. An older app
/// refuses them as not allowlisted (404), which is indistinguishable from a
/// typo on the wire, so the transport appends an "update Staged" hint to
/// exactly these.
pub const TAG_COMMANDS: &[&str] = &[
    "get_tag_state",
    "set_project_group_tag",
    "set_project_tag_assigned",
    "create_tag",
    "update_tag",
    "move_tag",
    "create_tag_group",
    "rename_tag_group",
    "set_icon_tag_group",
    "set_tag_group_sidebar",
    "reorder_tag_groups",
];

pub fn is_tag_command(command: &str) -> bool {
    TAG_COMMANDS.contains(&command)
}

// =============================================================================
// Wire shapes (the app's `store::tags::TagState`, camelCase)
// =============================================================================

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagStateWire {
    #[serde(default)]
    pub groups: Vec<TagGroupWire>,
    #[serde(default)]
    pub tags: Vec<TagWire>,
    #[serde(default)]
    pub assignments: Vec<ProjectTagWire>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGroupWire {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub is_icon_group: bool,
    #[serde(default)]
    pub show_in_sidebar: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWire {
    pub id: String,
    #[serde(default)]
    pub group_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub position: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTagWire {
    pub project_id: String,
    pub tag_id: String,
}

impl TagStateWire {
    pub fn group(&self, id: &str) -> Option<&TagGroupWire> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn tag(&self, id: &str) -> Option<&TagWire> {
        self.tags.iter().find(|t| t.id == id)
    }

    /// The tags in `group_id`, or the ungrouped tags for `None`, in catalog
    /// order.
    pub fn tags_in(&self, group_id: Option<&str>) -> Vec<&TagWire> {
        self.tags
            .iter()
            .filter(|t| t.group_id.as_deref() == group_id)
            .collect()
    }

    /// The tag ids a project carries.
    pub fn tag_ids_for(&self, project_id: &str) -> Vec<&str> {
        self.assignments
            .iter()
            .filter(|a| a.project_id == project_id)
            .map(|a| a.tag_id.as_str())
            .collect()
    }

    /// The project ids carrying a tag.
    pub fn projects_with(&self, tag_id: &str) -> Vec<&str> {
        self.assignments
            .iter()
            .filter(|a| a.tag_id == tag_id)
            .map(|a| a.project_id.as_str())
            .collect()
    }

    /// How a tag prints: `Group/Name`, or the bare name when ungrouped.
    pub fn label(&self, tag: &TagWire) -> String {
        match tag.group_id.as_deref().and_then(|id| self.group(id)) {
            Some(group) => format!("{}/{}", group.name, tag.name),
            None => tag.name.clone(),
        }
    }
}

fn fetch_state(client: &Client) -> Result<(TagStateWire, Value), CtlError> {
    let raw = client.invoke("get_tag_state", json!({}))?;
    let state: TagStateWire = parse(&raw, "tag state")?;
    Ok((state, raw))
}

/// Projects carrying each tag, by tag id. Tags nobody carries are absent.
pub fn usage_counts(state: &TagStateWire) -> HashMap<&str, usize> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for assignment in &state.assignments {
        *counts.entry(assignment.tag_id.as_str()).or_default() += 1;
    }
    counts
}

/// The raw JSON object with `"id": id` inside `raw[field]`, for `--json`
/// output that must carry the app's full row rather than the CLI's view.
fn raw_row(raw: &Value, field: &str, id: &str) -> Value {
    raw.get(field)
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("id").and_then(Value::as_str) == Some(id))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

// =============================================================================
// Address resolution
// =============================================================================

/// A `<tag>` argument as typed. `a/b` is tried as `<group>/<name>` first and
/// as one whole name second, since a tag name may itself hold a slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagRef {
    Qualified { group: String, name: String },
    Bare(String),
}

impl TagRef {
    pub fn parse(input: &str) -> Self {
        match input.split_once('/') {
            Some((group, name)) if !group.is_empty() && !name.is_empty() => Self::Qualified {
                group: group.to_string(),
                name: name.to_string(),
            },
            _ => Self::Bare(input.to_string()),
        }
    }
}

/// Match a `<group>` argument: exact id, then exact name, then unique
/// case-insensitive name (the backend's uniqueness rule is ASCII-case-
/// insensitive, so at most one group can match that way).
pub fn find_group<'a>(state: &'a TagStateWire, needle: &str) -> Result<&'a TagGroupWire, CtlError> {
    if let Some(group) = state.groups.iter().find(|g| g.id == needle) {
        return Ok(group);
    }
    if let Some(group) = state.groups.iter().find(|g| g.name == needle) {
        return Ok(group);
    }
    let loose: Vec<&TagGroupWire> = state
        .groups
        .iter()
        .filter(|g| g.name.eq_ignore_ascii_case(needle))
        .collect();
    match loose.len() {
        1 => Ok(loose[0]),
        0 => {
            let names: Vec<&str> = state.groups.iter().map(|g| g.name.as_str()).collect();
            Err(CtlError::command(
                "not_found",
                if names.is_empty() {
                    format!("no tag group named {needle:?} — the app has no tag groups yet")
                } else {
                    format!(
                        "no tag group named {needle:?}. Groups: {}",
                        names.join(", ")
                    )
                },
            ))
        }
        _ => Err(CtlError::command(
            "ambiguous",
            format!(
                "{needle:?} matches more than one tag group by case-insensitive name; \
                 use the exact name or the group id from `stagedctl tag group list`"
            ),
        )),
    }
}

/// Match a `<tag>` argument against the catalog: exact id; `<group>/<name>`
/// (group by name or id, name exact then case-insensitive); then the whole
/// input as a name, exact then case-insensitive, which must be unique
/// across the catalog.
pub fn find_tag<'a>(state: &'a TagStateWire, needle: &str) -> Result<&'a TagWire, CtlError> {
    if let Some(tag) = state.tags.iter().find(|t| t.id == needle) {
        return Ok(tag);
    }
    if let TagRef::Qualified { group, name } = TagRef::parse(needle) {
        if let Ok(group) = find_group(state, &group) {
            let in_group = state.tags_in(Some(&group.id));
            if let Some(tag) = in_group
                .iter()
                .find(|t| t.name == name)
                .or_else(|| in_group.iter().find(|t| t.name.eq_ignore_ascii_case(&name)))
            {
                return Ok(tag);
            }
        }
    }
    let exact: Vec<&TagWire> = state.tags.iter().filter(|t| t.name == needle).collect();
    let candidates = if exact.is_empty() {
        state
            .tags
            .iter()
            .filter(|t| t.name.eq_ignore_ascii_case(needle))
            .collect()
    } else {
        exact
    };
    match candidates.len() {
        1 => Ok(candidates[0]),
        0 => {
            let labels: Vec<String> = state.tags.iter().map(|t| state.label(t)).collect();
            Err(CtlError::command(
                "not_found",
                if labels.is_empty() {
                    format!("no tag named {needle:?} — the app has no tags yet")
                } else {
                    format!("no tag named {needle:?}. Tags: {}", labels.join(", "))
                },
            ))
        }
        _ => {
            let labels: Vec<String> = candidates.iter().map(|t| state.label(t)).collect();
            Err(CtlError::command(
                "ambiguous",
                format!(
                    "{needle:?} names more than one tag: {}. Use the <group>/<name> form or \
                     the tag id from `stagedctl tag list`",
                    labels.join(", ")
                ),
            ))
        }
    }
}

// =============================================================================
// Rendering helpers
// =============================================================================

fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// `(project icon, in sidebar)`, or empty when neither flag is set.
fn group_markers(group: &TagGroupWire) -> String {
    let mut markers = Vec::new();
    if group.is_icon_group {
        markers.push("project icon");
    }
    if group.show_in_sidebar {
        markers.push("in sidebar");
    }
    if markers.is_empty() {
        String::new()
    } else {
        format!("  ({})", markers.join(", "))
    }
}

/// Aligned tag rows: label, colour, icon, usage, id.
fn tag_rows(state: &TagStateWire, tags: &[&TagWire], usage: &HashMap<&str, usize>) -> Vec<String> {
    let label_w = tags
        .iter()
        .map(|t| state.label(t).chars().count())
        .max()
        .unwrap_or(0);
    let color_w = tags
        .iter()
        .map(|t| t.color.chars().count())
        .max()
        .unwrap_or(0);
    let icon_w = tags
        .iter()
        .map(|t| t.icon.as_deref().unwrap_or("").chars().count())
        .max()
        .unwrap_or(0);
    tags.iter()
        .map(|t| {
            let count = usage.get(t.id.as_str()).copied().unwrap_or(0);
            let mut line = format!("  {:<label_w$}  {:<color_w$}", state.label(t), t.color,);
            if icon_w > 0 {
                line.push_str(&format!("  {:<icon_w$}", t.icon.as_deref().unwrap_or("")));
            }
            line.push_str(&format!("  {}  id:{}", plural(count, "project"), t.id));
            line
        })
        .collect()
}

/// The `tag list` rendering: each group (or only `only`) and its tags, then
/// the ungrouped tags under "Tags".
pub fn render_tag_list(state: &TagStateWire, only: Option<&TagGroupWire>) -> String {
    let usage = usage_counts(state);
    let mut lines = Vec::new();
    let groups: Vec<&TagGroupWire> = match only {
        Some(group) => vec![group],
        None => state.groups.iter().collect(),
    };
    for group in groups {
        lines.push(format!(
            "{}{}  id:{}",
            group.name,
            group_markers(group),
            group.id
        ));
        let tags = state.tags_in(Some(&group.id));
        if tags.is_empty() {
            lines.push("  (no tags)".to_string());
        } else {
            lines.extend(tag_rows(state, &tags, &usage));
        }
    }
    if only.is_none() {
        let ungrouped = state.tags_in(None);
        if !ungrouped.is_empty() {
            lines.push("Tags".to_string());
            lines.extend(tag_rows(state, &ungrouped, &usage));
        }
    }
    if lines.is_empty() {
        "no tags".to_string()
    } else {
        lines.join("\n")
    }
}

/// The `tag group list` rendering.
pub fn render_group_list(state: &TagStateWire) -> String {
    if state.groups.is_empty() {
        return "no tag groups".to_string();
    }
    let name_w = state
        .groups
        .iter()
        .map(|g| g.name.chars().count())
        .max()
        .unwrap_or(0);
    let markers: Vec<String> = state
        .groups
        .iter()
        .map(|g| group_markers(g).trim_start().to_string())
        .collect();
    let marker_w = markers.iter().map(|m| m.chars().count()).max().unwrap_or(0);
    state
        .groups
        .iter()
        .zip(markers)
        .map(|(g, marker)| {
            let tags = state.tags_in(Some(&g.id)).len();
            let mut line = format!("{:<name_w$}", g.name);
            if marker_w > 0 {
                line.push_str(&format!("  {marker:<marker_w$}"));
            }
            line.push_str(&format!("  {}  id:{}", plural(tags, "tag"), g.id));
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `project tags` rendering: one line per group, then the free tags.
pub(crate) fn render_project_tags(state: &TagStateWire, project: &Project) -> String {
    let carried = state.tag_ids_for(&project.id);
    let mut lines = vec![format!("project {}  id:{}", project.name, project.id)];
    let name_w = state
        .groups
        .iter()
        .map(|g| g.name.chars().count() + 1)
        .max()
        .unwrap_or(0)
        .max("tags:".len());
    for group in &state.groups {
        let choice = carried
            .iter()
            .filter_map(|id| state.tag(id))
            .find(|t| t.group_id.as_deref() == Some(group.id.as_str()));
        let value = match choice {
            Some(tag) => tag.name.clone(),
            None => "(none)".to_string(),
        };
        let marker = if group.is_icon_group {
            "  (project icon)"
        } else {
            ""
        };
        lines.push(format!(
            "  {:<name_w$}  {value}{marker}",
            format!("{}:", group.name)
        ));
    }
    let free: Vec<&str> = state
        .tags_in(None)
        .into_iter()
        .filter(|t| carried.contains(&t.id.as_str()))
        .map(|t| t.name.as_str())
        .collect();
    lines.push(format!(
        "  {:<name_w$}  {}",
        "tags:",
        if free.is_empty() {
            "(none)".to_string()
        } else {
            free.join(", ")
        }
    ));
    lines.join("\n")
}

/// The `tag show` rendering.
pub(crate) fn render_tag_show(
    state: &TagStateWire,
    tag: &TagWire,
    projects: &[&Project],
) -> String {
    let mut head = format!("{}  {}", state.label(tag), tag.color);
    if let Some(icon) = &tag.icon {
        head.push_str(&format!("  {icon}"));
    }
    head.push_str(&format!("  id:{}", tag.id));
    let mut lines = vec![head];
    match tag.group_id.as_deref().and_then(|id| state.group(id)) {
        Some(group) => lines.push(format!("  group:    {}  id:{}", group.name, group.id)),
        None => lines.push("  group:    (ungrouped)".to_string()),
    }
    lines.push(format!("  projects: {}", projects.len()));
    for project in projects {
        lines.push(format!("    {}  id:{}", project.name, project.id));
    }
    lines.join("\n")
}

// =============================================================================
// Verbs
// =============================================================================

pub fn tag_list(client: &Client, group: Option<&str>) -> Result<CmdOutput, CtlError> {
    let (state, mut raw) = fetch_state(client)?;
    let only = match group {
        Some(needle) => Some(find_group(&state, needle)?),
        None => None,
    };
    let human = render_tag_list(&state, only);
    let usage: serde_json::Map<String, Value> = usage_counts(&state)
        .into_iter()
        .map(|(id, n)| (id.to_string(), json!(n)))
        .collect();
    if let Some(object) = raw.as_object_mut() {
        object.insert("usage".to_string(), Value::Object(usage));
    }
    Ok(CmdOutput { json: raw, human })
}

pub fn tag_group_list(client: &Client) -> Result<CmdOutput, CtlError> {
    let (state, raw) = fetch_state(client)?;
    let human = render_group_list(&state);
    let json = raw.get("groups").cloned().unwrap_or_else(|| json!([]));
    Ok(CmdOutput { json, human })
}

pub fn tag_show(client: &Client, needle: &str) -> Result<CmdOutput, CtlError> {
    let (state, raw) = fetch_state(client)?;
    let tag = find_tag(&state, needle)?;
    let projects_raw = client.invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&projects_raw, "project list")?;
    let carrying = state.projects_with(&tag.id);
    // Both lists are confined to the credential's scope the same way, so a
    // project id in the assignments but not the list is only a race with a
    // deletion; it is left out rather than printed nameless.
    let carrying: Vec<&Project> = projects
        .iter()
        .filter(|p| carrying.contains(&p.id.as_str()))
        .collect();
    let human = render_tag_show(&state, tag, &carrying);
    let group = tag
        .group_id
        .as_deref()
        .map(|id| raw_row(&raw, "groups", id))
        .unwrap_or(Value::Null);
    let json = json!({
        "tag": raw_row(&raw, "tags", &tag.id),
        "group": group,
        "projects": carrying
            .iter()
            .map(|p| json!({"id": p.id, "name": p.name}))
            .collect::<Vec<_>>(),
    });
    Ok(CmdOutput { json, human })
}

pub fn project_tags(client: &Client, needle: &str) -> Result<CmdOutput, CtlError> {
    let projects_raw = client.invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&projects_raw, "project list")?;
    let project = find_project(&projects, needle)?.clone();
    let (state, raw) = fetch_state(client)?;
    let human = render_project_tags(&state, &project);
    let json = project_tags_json(&state, &raw, &projects_raw, &project);
    Ok(CmdOutput { json, human })
}

/// The `project tags` `--json` shape: `{ project, groups: [{ group, tag }],
/// tags: [Tag] }`, built from the app's raw rows.
fn project_tags_json(
    state: &TagStateWire,
    raw: &Value,
    projects_raw: &Value,
    project: &Project,
) -> Value {
    let carried = state.tag_ids_for(&project.id);
    let groups: Vec<Value> = state
        .groups
        .iter()
        .map(|group| {
            let choice = carried
                .iter()
                .filter_map(|id| state.tag(id))
                .find(|t| t.group_id.as_deref() == Some(group.id.as_str()));
            json!({
                "group": raw_row(raw, "groups", &group.id),
                "tag": choice.map(|t| raw_row(raw, "tags", &t.id)).unwrap_or(Value::Null),
            })
        })
        .collect();
    let tags: Vec<Value> = state
        .tags_in(None)
        .into_iter()
        .filter(|t| carried.contains(&t.id.as_str()))
        .map(|t| raw_row(raw, "tags", &t.id))
        .collect();
    let project_raw = projects_raw
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|v| v.get("id").and_then(Value::as_str) == Some(project.id.as_str()))
        })
        .cloned()
        .unwrap_or(Value::Null);
    json!({ "project": project_raw, "groups": groups, "tags": tags })
}

// =============================================================================
// project tag / project untag
// =============================================================================

/// One write of a `project tag` / `project untag` plan. Each maps onto one
/// allowlisted dispatch command; the project is supplied when it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `set_project_group_tag`: make `tag_id` the project's choice in its
    /// group, replacing a sibling.
    Choose { group_id: String, tag_id: String },
    /// `set_project_group_tag` with no tag: clear the choice in a group.
    ClearGroup { group_id: String },
    /// `set_project_tag_assigned`: add or remove exactly `tag_id`.
    Assign { tag_id: String, assigned: bool },
    /// `create_tag` with `assignProjectId`: a new ungrouped tag, assigned
    /// in the same transaction.
    Create { name: String },
}

impl Step {
    fn command(&self) -> &'static str {
        match self {
            Self::Choose { .. } | Self::ClearGroup { .. } => "set_project_group_tag",
            Self::Assign { .. } => "set_project_tag_assigned",
            Self::Create { .. } => "create_tag",
        }
    }

    fn args(&self, project_id: &str) -> Value {
        match self {
            Self::Choose { group_id, tag_id } => {
                json!({ "projectId": project_id, "groupId": group_id, "tagId": tag_id })
            }
            Self::ClearGroup { group_id } => {
                json!({ "projectId": project_id, "groupId": group_id, "tagId": Value::Null })
            }
            Self::Assign { tag_id, assigned } => {
                json!({ "projectId": project_id, "tagId": tag_id, "assigned": assigned })
            }
            Self::Create { name } => json!({ "name": name, "assignProjectId": project_id }),
        }
    }
}

/// Plan `project tag`: resolve every `<tag>` against the catalog, in
/// argument order, into the writes that apply them. A grouped tag becomes
/// a [`Step::Choose`], an ungrouped one a [`Step::Assign`]. A tag named
/// twice is applied once. Two different tags of one group are refused
/// (`invalid_arguments`), since a project holds one tag per group and the
/// second would silently undo the first.
///
/// A `<tag>` nothing answers to is `not_found`, unless `create` is set: then
/// a bare name becomes a [`Step::Create`] of an ungrouped tag. A
/// `<group>/<name>` whose group exists is refused instead, because
/// `--create` never creates a grouped tag. `ambiguous` is an error either
/// way.
pub fn plan_tagging(
    state: &TagStateWire,
    needles: &[String],
    create: bool,
) -> Result<Vec<Step>, CtlError> {
    let mut steps: Vec<Step> = Vec::new();
    // group id → (the tag chosen there, the argument that chose it)
    let mut chosen: HashMap<&str, (&TagWire, &str)> = HashMap::new();
    let mut creating: Vec<String> = Vec::new();
    for needle in needles {
        match find_tag(state, needle) {
            Ok(tag) => match tag.group_id.as_deref() {
                Some(group_id) => {
                    if let Some((earlier, earlier_needle)) = chosen.get(group_id) {
                        if earlier.id == tag.id {
                            continue;
                        }
                        let group_name = state
                            .group(group_id)
                            .map(|g| g.name.as_str())
                            .unwrap_or(group_id);
                        return Err(CtlError::command(
                            "invalid_arguments",
                            format!(
                                "{earlier_needle:?} and {needle:?} are both in {group_name}, and \
                                 a project holds one tag per group; name one of them"
                            ),
                        ));
                    }
                    chosen.insert(group_id, (tag, needle.as_str()));
                    steps.push(Step::Choose {
                        group_id: group_id.to_string(),
                        tag_id: tag.id.clone(),
                    });
                }
                None => {
                    let step = Step::Assign {
                        tag_id: tag.id.clone(),
                        assigned: true,
                    };
                    if !steps.contains(&step) {
                        steps.push(step);
                    }
                }
            },
            Err(CtlError::Command {
                code: "not_found", ..
            }) if create => {
                if let TagRef::Qualified { group, .. } = TagRef::parse(needle) {
                    if let Ok(group) = find_group(state, &group) {
                        return Err(CtlError::command(
                            "invalid_arguments",
                            format!(
                                "--create makes ungrouped tags only, and {needle:?} names the \
                                 group {}; create the tag in that group first (`stagedctl tag \
                                 create --group`), or name one of its existing tags",
                                group.name
                            ),
                        ));
                    }
                }
                let name = needle.trim();
                if name.is_empty() {
                    return Err(CtlError::command(
                        "invalid_arguments",
                        "a tag name must not be empty",
                    ));
                }
                if creating.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                    continue;
                }
                creating.push(name.to_string());
                steps.push(Step::Create {
                    name: name.to_string(),
                });
            }
            Err(err) => return Err(err),
        }
    }
    Ok(steps)
}

/// Plan `project untag`: with `group`, one [`Step::ClearGroup`]; otherwise
/// one [`Step::Assign`] removal per distinct `<tag>`, resolved against the
/// catalog so a typo fails before anything is written. The pairing of the
/// two forms is checked by `validate()` before discovery.
pub fn plan_untagging(
    state: &TagStateWire,
    needles: &[String],
    group: Option<&str>,
) -> Result<Vec<Step>, CtlError> {
    if let Some(group) = group {
        let group = find_group(state, group)?;
        return Ok(vec![Step::ClearGroup {
            group_id: group.id.clone(),
        }]);
    }
    let mut steps: Vec<Step> = Vec::new();
    for needle in needles {
        let tag = find_tag(state, needle)?;
        let step = Step::Assign {
            tag_id: tag.id.clone(),
            assigned: false,
        };
        if !steps.contains(&step) {
            steps.push(step);
        }
    }
    Ok(steps)
}

/// One line per step, against the catalog as it was before the writes, so
/// a step that changed nothing says so. `created` holds the app's rows for
/// the [`Step::Create`] steps, in order.
pub(crate) fn render_steps(
    state: &TagStateWire,
    project: &Project,
    steps: &[Step],
    created: &[Value],
    heading: &str,
) -> String {
    let carried = state.tag_ids_for(&project.id);
    let mut lines = vec![format!("{heading} {}  id:{}", project.name, project.id)];
    let mut created = created.iter();
    for step in steps {
        let line = match step {
            Step::Choose { group_id, tag_id } => {
                let group = state
                    .group(group_id)
                    .map(|g| g.name.as_str())
                    .unwrap_or(group_id);
                let name = state.tag(tag_id).map(|t| t.name.as_str()).unwrap_or(tag_id);
                let note = if carried.contains(&tag_id.as_str()) {
                    "  (unchanged)"
                } else {
                    ""
                };
                format!("  {group}: {name}{note}")
            }
            Step::ClearGroup { group_id } => {
                let group = state
                    .group(group_id)
                    .map(|g| g.name.as_str())
                    .unwrap_or(group_id);
                let had = carried
                    .iter()
                    .filter_map(|id| state.tag(id))
                    .any(|t| t.group_id.as_deref() == Some(group_id.as_str()));
                let note = if had { "" } else { "  (unchanged)" };
                format!("  {group}: (none){note}")
            }
            Step::Assign { tag_id, assigned } => {
                let label = state
                    .tag(tag_id)
                    .map(|t| state.label(t))
                    .unwrap_or_else(|| tag_id.clone());
                let had = carried.contains(&tag_id.as_str());
                let (sign, note) = match (assigned, had) {
                    (true, true) => ("+", "  (already)"),
                    (true, false) => ("+", ""),
                    (false, true) => ("-", ""),
                    (false, false) => ("-", "  (not carried)"),
                };
                format!("  {sign} {label}{note}")
            }
            Step::Create { name } => {
                let id = created
                    .next()
                    .and_then(|row| row.get("id"))
                    .and_then(Value::as_str)
                    .map(|id| format!("  id:{id}"))
                    .unwrap_or_default();
                format!("  + {name}  (created){id}")
            }
        };
        lines.push(line);
    }
    lines.join("\n")
}

/// Run a plan in order and print the project's tags as they stand after
/// it. `--json` is the `project tags` shape under `ok: true`, plus the
/// `created` rows.
fn apply_steps(
    client: &Client,
    before: &TagStateWire,
    projects_raw: &Value,
    project: &Project,
    steps: &[Step],
    heading: &str,
) -> Result<CmdOutput, CtlError> {
    let mut created: Vec<Value> = Vec::new();
    for step in steps {
        let result = client.invoke(step.command(), step.args(&project.id))?;
        if matches!(step, Step::Create { .. }) {
            created.push(result);
        }
    }
    let human = render_steps(before, project, steps, &created, heading);
    let (after, raw) = fetch_state(client)?;
    let mut json = project_tags_json(&after, &raw, projects_raw, project);
    if let Some(object) = json.as_object_mut() {
        object.insert("ok".to_string(), json!(true));
        object.insert("created".to_string(), Value::Array(created));
    }
    Ok(CmdOutput { json, human })
}

pub fn project_tag(
    client: &Client,
    needle: &str,
    tags: &[String],
    create: bool,
) -> Result<CmdOutput, CtlError> {
    let projects_raw = client.invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&projects_raw, "project list")?;
    let project = find_project(&projects, needle)?.clone();
    let (state, _) = fetch_state(client)?;
    let steps = plan_tagging(&state, tags, create)?;
    apply_steps(client, &state, &projects_raw, &project, &steps, "tagged")
}

pub fn project_untag(
    client: &Client,
    needle: &str,
    tags: &[String],
    group: Option<&str>,
) -> Result<CmdOutput, CtlError> {
    let projects_raw = client.invoke("list_projects", json!({}))?;
    let projects: Vec<Project> = parse(&projects_raw, "project list")?;
    let project = find_project(&projects, needle)?.clone();
    let (state, _) = fetch_state(client)?;
    let steps = plan_untagging(&state, tags, group)?;
    apply_steps(client, &state, &projects_raw, &project, &steps, "untagged")
}

/// The project ids carrying every one of `needles` (AND). Resolves each
/// `<tag>` against the catalog first, so a typo fails before anything is
/// filtered.
pub fn projects_with_all_tags<'a>(
    state: &'a TagStateWire,
    needles: &[String],
) -> Result<impl Fn(&str) -> bool + 'a, CtlError> {
    let mut ids: Vec<String> = Vec::new();
    for needle in needles {
        let tag = find_tag(state, needle)?;
        if !ids.contains(&tag.id) {
            ids.push(tag.id.clone());
        }
    }
    Ok(move |project_id: &str| {
        let carried = state.tag_ids_for(project_id);
        ids.iter().all(|id| carried.contains(&id.as_str()))
    })
}

// =============================================================================
// The tag catalog: tag create / edit / move, tag group create / rename /
// set-icon / set-sidebar / reorder
// =============================================================================

/// One tag of a group template: `(name, icon, color)`.
pub type TemplateTag = (&'static str, &'static str, &'static str);

/// The Status template: the project statuses Staged shipped before tags.
/// Copied from `src/lib/features/tags/tagTemplates.ts` (the frontend's
/// `TAG_GROUP_TEMPLATES`), which this crate cannot import; a test pins the
/// two against that file's source text.
pub const STATUS_TEMPLATE: &[TemplateTag] = &[
    ("Idea", "lightbulb", "amber"),
    ("In progress", "circle-dot", "blue"),
    ("Polishing", "paintbrush", "cyan"),
    ("Needs review", "eye", "purple"),
    ("Done", "circle-check", "green"),
    ("On hold", "circle-pause", "gray"),
    ("Blocked", "octagon-minus", "red"),
];

/// The Priority template, copied from the same file.
pub const PRIORITY_TEMPLATE: &[TemplateTag] = &[
    ("P0", "chevrons-up", "red"),
    ("P1", "chevron-up", "amber"),
    ("P2", "minus", "blue"),
    ("P3", "chevron-down", "gray"),
];

/// The tags a template creates with its group.
pub fn template_tags(template: GroupTemplate) -> &'static [TemplateTag] {
    match template {
        GroupTemplate::Status => STATUS_TEMPLATE,
        GroupTemplate::Priority => PRIORITY_TEMPLATE,
        GroupTemplate::Blank => &[],
    }
}

/// The `tags` argument of `create_tag_group`: the app's `NewTag` rows.
pub fn template_new_tags(template: GroupTemplate) -> Vec<Value> {
    template_tags(template)
        .iter()
        .map(|(name, icon, color)| json!({ "name": name, "icon": icon, "color": color }))
        .collect()
}

/// A `tag edit` request's icon, which may be left alone, set, or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconChange<'a> {
    Keep,
    Set(&'a str),
    Remove,
}

/// The `update_tag` arguments for `tag edit`: the app takes the whole
/// value, so each flag not given is filled from the tag as it stands. This
/// is the frontend's `editTag` rule.
pub fn plan_edit(
    tag: &TagWire,
    name: Option<&str>,
    color: Option<TagColor>,
    icon: IconChange<'_>,
) -> Value {
    let icon = match icon {
        IconChange::Keep => tag.icon.clone(),
        IconChange::Set(icon) => Some(icon.to_string()),
        IconChange::Remove => None,
    };
    json!({
        "tagId": tag.id,
        "name": name.unwrap_or(&tag.name),
        "color": color.map(|c| c.as_str().to_string()).unwrap_or_else(|| tag.color.clone()),
        "icon": icon,
    })
}

/// A resolved `tag move`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePlan {
    pub tag_id: String,
    /// The destination group, or `None` for the ungrouped list.
    pub group_id: Option<String>,
    /// The zero-based position in the destination.
    pub index: usize,
    /// Whether the tag changes scope (group or ungrouped) at all.
    pub cross_scope: bool,
    /// Projects carrying the tag that already have another tag in the
    /// destination group. The app keeps theirs and drops this tag from
    /// them. Zero unless the tag moves into a group from elsewhere.
    pub conflicts: Vec<String>,
}

impl MovePlan {
    pub fn args(&self) -> Value {
        json!({ "tagId": self.tag_id, "groupId": self.group_id, "index": self.index })
    }
}

/// Plan `tag move` against the catalog: resolve the destination, default
/// `index` to the end of it (the tag itself not counted when it is already
/// there), count the projects that would lose the tag, and refuse a name
/// the destination already has (the app would refuse it too; this gives a
/// clearer message and writes nothing). Mirrors the frontend's
/// `resolveTagDrop`.
pub fn plan_move(
    state: &TagStateWire,
    tag: &TagWire,
    group: Option<&TagGroupWire>,
    index: Option<usize>,
) -> Result<MovePlan, CtlError> {
    let group_id = group.map(|g| g.id.as_str());
    let others: Vec<&TagWire> = state
        .tags_in(group_id)
        .into_iter()
        .filter(|t| t.id != tag.id)
        .collect();
    let cross_scope = tag.group_id.as_deref() != group_id;
    if let Some(other) = others
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(&tag.name))
    {
        let scope = match group {
            Some(group) => format!("the group {}", group.name),
            None => "the ungrouped tags".to_string(),
        };
        return Err(CtlError::command(
            "invalid_arguments",
            format!(
                "{scope} already has a tag named {:?} (id:{}); rename one of them first \
                 (`stagedctl tag edit --name`)",
                other.name, other.id
            ),
        ));
    }
    let index = index.unwrap_or(others.len()).min(others.len());
    let mut conflicts = Vec::new();
    if cross_scope && group_id.is_some() {
        let siblings: Vec<&str> = others.iter().map(|t| t.id.as_str()).collect();
        let mut seen: Vec<&str> = Vec::new();
        for assignment in &state.assignments {
            if assignment.tag_id != tag.id || seen.contains(&assignment.project_id.as_str()) {
                continue;
            }
            seen.push(&assignment.project_id);
            let carried = state.tag_ids_for(&assignment.project_id);
            if carried.iter().any(|id| siblings.contains(id)) {
                conflicts.push(assignment.project_id.clone());
            }
        }
    }
    Ok(MovePlan {
        tag_id: tag.id.clone(),
        group_id: group_id.map(str::to_string),
        index,
        cross_scope,
        conflicts,
    })
}

/// Plan `tag group reorder`: resolve every `<group>` and require the list
/// to name each group exactly once, as the app does, before anything is
/// sent.
pub fn plan_group_order(state: &TagStateWire, needles: &[String]) -> Result<Vec<String>, CtlError> {
    let mut order: Vec<String> = Vec::new();
    for needle in needles {
        let group = find_group(state, needle)?;
        if order.contains(&group.id) {
            return Err(CtlError::command(
                "invalid_arguments",
                format!(
                    "{} is listed twice ({needle:?}); name every group exactly once",
                    group.name
                ),
            ));
        }
        order.push(group.id.clone());
    }
    let missing: Vec<&str> = state
        .groups
        .iter()
        .filter(|g| !order.contains(&g.id))
        .map(|g| g.name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(CtlError::command(
            "invalid_arguments",
            format!(
                "the new order must name every group exactly once; missing: {}",
                missing.join(", ")
            ),
        ));
    }
    Ok(order)
}

/// The `--json` of a catalog write: the app's return value (or the row the
/// write touched, re-read) under `ok: true`.
fn ok_json(fields: Vec<(&str, Value)>) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("ok".to_string(), json!(true));
    for (key, value) in fields {
        object.insert(key.to_string(), value);
    }
    Value::Object(object)
}

/// One tag's row as `tag list` prints it, with a leading word.
fn tag_line(state: &TagStateWire, tag: &TagWire, heading: &str) -> String {
    let usage = usage_counts(state);
    format!("{heading}{}", tag_rows(state, &[tag], &usage).remove(0))
}

pub fn tag_create(
    client: &Client,
    name: &str,
    group: Option<&str>,
    color: Option<TagColor>,
    icon: Option<&str>,
) -> Result<CmdOutput, CtlError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CtlError::command(
            "invalid_arguments",
            "a tag name must not be empty",
        ));
    }
    let (state, _) = fetch_state(client)?;
    let group_id = match group {
        Some(needle) => Some(find_group(&state, needle)?.id.clone()),
        None => None,
    };
    let created = client.invoke(
        "create_tag",
        json!({
            "name": name,
            "color": color.map(|c| c.as_str()),
            "icon": icon,
            "groupId": group_id,
        }),
    )?;
    let (after, _) = fetch_state(client)?;
    let id = created.get("id").and_then(Value::as_str).unwrap_or("");
    let human = match after.tag(id) {
        Some(tag) => tag_line(&after, tag, "created"),
        None => format!("created {name}  id:{id}"),
    };
    Ok(CmdOutput {
        json: ok_json(vec![("tag", created)]),
        human,
    })
}

pub fn tag_edit(
    client: &Client,
    needle: &str,
    name: Option<&str>,
    color: Option<TagColor>,
    icon: IconChange<'_>,
) -> Result<CmdOutput, CtlError> {
    let (state, _) = fetch_state(client)?;
    let tag = find_tag(&state, needle)?;
    let before = state.label(tag);
    let args = plan_edit(tag, name.map(str::trim), color, icon);
    client.invoke("update_tag", args)?;
    let (after, raw) = fetch_state(client)?;
    let human = match after.tag(&tag.id) {
        Some(updated) if after.label(updated) != before => {
            format!("{}  (was {before})", tag_line(&after, updated, "updated"))
        }
        Some(updated) => tag_line(&after, updated, "updated"),
        None => format!("updated {before}  id:{}", tag.id),
    };
    Ok(CmdOutput {
        json: ok_json(vec![("tag", raw_row(&raw, "tags", &tag.id))]),
        human,
    })
}

pub fn tag_move(
    client: &Client,
    needle: &str,
    group: Option<&str>,
    index: Option<usize>,
    yes: bool,
) -> Result<CmdOutput, CtlError> {
    let (state, _) = fetch_state(client)?;
    let tag = find_tag(&state, needle)?;
    let destination = match group {
        Some(needle) => Some(find_group(&state, needle)?),
        None => None,
    };
    let plan = plan_move(&state, tag, destination, index)?;
    if !plan.conflicts.is_empty() && !yes {
        let group_name = destination.map(|g| g.name.as_str()).unwrap_or_default();
        let projects_raw = client.invoke("list_projects", json!({}))?;
        let projects: Vec<Project> = parse(&projects_raw, "project list")?;
        let names: Vec<&str> = plan
            .conflicts
            .iter()
            .filter_map(|id| projects.iter().find(|p| &p.id == id))
            .map(|p| p.name.as_str())
            .collect();
        let listed = if names.is_empty() {
            String::new()
        } else {
            format!(" ({})", names.join(", "))
        };
        return Err(CtlError::command(
            "conflicts",
            format!(
                "moving {} into {group_name} would drop it from {}{listed} that already \
                 {} a {group_name} tag, which they keep; pass --yes to go ahead",
                state.label(tag),
                plural(plan.conflicts.len(), "project"),
                if plan.conflicts.len() == 1 {
                    "has"
                } else {
                    "have"
                },
            ),
        ));
    }
    client.invoke("move_tag", plan.args())?;
    let (after, raw) = fetch_state(client)?;
    let position = after
        .tags_in(plan.group_id.as_deref())
        .iter()
        .position(|t| t.id == tag.id)
        .unwrap_or(plan.index);
    let mut human = match after.tag(&tag.id) {
        Some(moved) => format!(
            "moved {}  (was {})  position {position}",
            after.label(moved),
            state.label(tag)
        ),
        None => format!("moved {}  id:{}", state.label(tag), tag.id),
    };
    if !plan.conflicts.is_empty() {
        human.push_str(&format!(
            "\n  {} lost it: already in {}",
            plural(plan.conflicts.len(), "project"),
            destination.map(|g| g.name.as_str()).unwrap_or_default()
        ));
    }
    Ok(CmdOutput {
        json: ok_json(vec![
            ("tag", raw_row(&raw, "tags", &tag.id)),
            ("conflicts", json!(plan.conflicts)),
        ]),
        human,
    })
}

pub fn tag_group_create(
    client: &Client,
    name: &str,
    template: GroupTemplate,
) -> Result<CmdOutput, CtlError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CtlError::command(
            "invalid_arguments",
            "a group name must not be empty",
        ));
    }
    let created = client.invoke(
        "create_tag_group",
        json!({ "name": name, "tags": template_new_tags(template) }),
    )?;
    let (after, _) = fetch_state(client)?;
    let id = created.get("id").and_then(Value::as_str).unwrap_or("");
    let human = match after.group(id) {
        Some(group) => format!("created {}", render_tag_list(&after, Some(group))),
        None => format!("created {name}  id:{id}"),
    };
    Ok(CmdOutput {
        json: ok_json(vec![("group", created)]),
        human,
    })
}

pub fn tag_group_rename(client: &Client, needle: &str, name: &str) -> Result<CmdOutput, CtlError> {
    let (state, _) = fetch_state(client)?;
    let group = find_group(&state, needle)?;
    client.invoke(
        "rename_tag_group",
        json!({ "groupId": group.id, "name": name.trim() }),
    )?;
    let (after, raw) = fetch_state(client)?;
    let now = after
        .group(&group.id)
        .map(|g| g.name.as_str())
        .unwrap_or(name);
    let human = if now == group.name {
        format!("renamed {}  (unchanged)  id:{}", group.name, group.id)
    } else {
        format!("renamed {} → {now}  id:{}", group.name, group.id)
    };
    Ok(CmdOutput {
        json: ok_json(vec![("group", raw_row(&raw, "groups", &group.id))]),
        human,
    })
}

pub fn tag_group_set_icon(client: &Client, needle: Option<&str>) -> Result<CmdOutput, CtlError> {
    let (state, _) = fetch_state(client)?;
    let group = match needle {
        Some(needle) => Some(find_group(&state, needle)?),
        None => None,
    };
    client.invoke(
        "set_icon_tag_group",
        json!({ "groupId": group.map(|g| g.id.as_str()) }),
    )?;
    let (_, raw) = fetch_state(client)?;
    let human = match group {
        Some(group) => format!("project icon: {}  id:{}", group.name, group.id),
        None => "project icon: (none; projects show their pull-request status)".to_string(),
    };
    Ok(CmdOutput {
        json: ok_json(vec![(
            "group",
            group
                .map(|g| raw_row(&raw, "groups", &g.id))
                .unwrap_or(Value::Null),
        )]),
        human,
    })
}

pub fn tag_group_set_sidebar(
    client: &Client,
    needle: &str,
    shown: bool,
) -> Result<CmdOutput, CtlError> {
    let (state, _) = fetch_state(client)?;
    let group = find_group(&state, needle)?;
    client.invoke(
        "set_tag_group_sidebar",
        json!({ "groupId": group.id, "shown": shown }),
    )?;
    let (_, raw) = fetch_state(client)?;
    let human = format!(
        "{} board in sidebar: {}{}  id:{}",
        group.name,
        if shown { "on" } else { "off" },
        if group.show_in_sidebar == shown {
            "  (unchanged)"
        } else {
            ""
        },
        group.id
    );
    Ok(CmdOutput {
        json: ok_json(vec![("group", raw_row(&raw, "groups", &group.id))]),
        human,
    })
}

pub fn tag_group_reorder(client: &Client, needles: &[String]) -> Result<CmdOutput, CtlError> {
    let (state, _) = fetch_state(client)?;
    let order = plan_group_order(&state, needles)?;
    client.invoke("reorder_tag_groups", json!({ "groupIds": order }))?;
    let (after, raw) = fetch_state(client)?;
    Ok(CmdOutput {
        json: ok_json(vec![(
            "groups",
            raw.get("groups").cloned().unwrap_or_else(|| json!([])),
        )]),
        human: render_group_list(&after),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, name: &str, icon: bool, sidebar: bool) -> TagGroupWire {
        TagGroupWire {
            id: id.to_string(),
            name: name.to_string(),
            is_icon_group: icon,
            show_in_sidebar: sidebar,
            ..Default::default()
        }
    }

    fn tag(id: &str, group: Option<&str>, name: &str, color: &str, icon: Option<&str>) -> TagWire {
        TagWire {
            id: id.to_string(),
            group_id: group.map(str::to_string),
            name: name.to_string(),
            color: color.to_string(),
            icon: icon.map(str::to_string),
            ..Default::default()
        }
    }

    fn assigned(project: &str, tag: &str) -> ProjectTagWire {
        ProjectTagWire {
            project_id: project.to_string(),
            tag_id: tag.to_string(),
        }
    }

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.to_string(),
            name: name.to_string(),
            github_repo: None,
            subpath: None,
            location: None,
        }
    }

    /// Status (icon, sidebar) and Priority groups, both holding a "High",
    /// plus ungrouped q4-launch and infra.
    fn catalog() -> TagStateWire {
        TagStateWire {
            groups: vec![
                group("g-status", "Status", true, true),
                group("g-prio", "Priority", false, false),
            ],
            tags: vec![
                tag(
                    "t-prog",
                    Some("g-status"),
                    "In progress",
                    "blue",
                    Some("loader"),
                ),
                tag("t-done", Some("g-status"), "Done", "green", Some("check")),
                tag("t-high-s", Some("g-status"), "High", "red", None),
                tag("t-high-p", Some("g-prio"), "High", "red", Some("flame")),
                tag("t-q4", None, "q4-launch", "purple", None),
                tag("t-infra", None, "infra", "gray", None),
                tag("t-slash", None, "a/b", "cyan", None),
            ],
            assignments: vec![
                assigned("p1", "t-prog"),
                assigned("p1", "t-q4"),
                assigned("p2", "t-prog"),
                assigned("p2", "t-high-p"),
                assigned("p3", "t-done"),
                assigned("p3", "t-q4"),
            ],
        }
    }

    #[test]
    fn wire_shapes_parse_the_apps_tag_state() {
        // Shape from `store/tags.rs` (`TagState`, camelCase); extra fields
        // such as timestamps are tolerated, so a newer app still parses.
        let raw = json!({
            "groups": [{
                "id": "g1", "name": "Status", "position": 0,
                "isIconGroup": true, "showInSidebar": false,
                "createdAt": 1, "updatedAt": 2
            }],
            "tags": [{
                "id": "t1", "groupId": "g1", "name": "Done", "color": "green",
                "icon": "check", "position": 0, "createdAt": 1, "updatedAt": 2
            }, {
                "id": "t2", "groupId": null, "name": "q4", "color": "purple",
                "icon": null, "position": 0, "createdAt": 1, "updatedAt": 2
            }],
            "assignments": [{ "projectId": "p1", "tagId": "t1" }]
        });
        let state: TagStateWire = serde_json::from_value(raw).unwrap();
        assert!(state.groups[0].is_icon_group);
        assert!(!state.groups[0].show_in_sidebar);
        assert_eq!(state.tags[0].group_id.as_deref(), Some("g1"));
        assert_eq!(state.tags[0].icon.as_deref(), Some("check"));
        assert_eq!(state.tags[1].group_id, None);
        assert_eq!(state.tags[1].icon, None);
        assert_eq!(state.tags[1].color, "purple");
        assert_eq!(state.assignments[0].project_id, "p1");
        assert_eq!(state.label(&state.tags[0]), "Status/Done");
        assert_eq!(state.label(&state.tags[1]), "q4");
    }

    #[test]
    fn tag_ref_parses_id_group_slash_name_and_bare_name() {
        assert_eq!(TagRef::parse("t-done"), TagRef::Bare("t-done".into()));
        assert_eq!(TagRef::parse("Done"), TagRef::Bare("Done".into()));
        assert_eq!(
            TagRef::parse("Status/Done"),
            TagRef::Qualified {
                group: "Status".into(),
                name: "Done".into()
            }
        );
        // Only the first slash splits; the rest belongs to the name.
        assert_eq!(
            TagRef::parse("Status/a/b"),
            TagRef::Qualified {
                group: "Status".into(),
                name: "a/b".into()
            }
        );
        // A leading or trailing slash can't be a group/name pair.
        assert_eq!(TagRef::parse("/x"), TagRef::Bare("/x".into()));
        assert_eq!(TagRef::parse("x/"), TagRef::Bare("x/".into()));
    }

    #[test]
    fn find_tag_prefers_id_then_group_name_then_unique_name() {
        let state = catalog();
        assert_eq!(find_tag(&state, "t-done").unwrap().id, "t-done");
        // group/name: by group name, group id, and case-insensitively.
        assert_eq!(find_tag(&state, "Status/Done").unwrap().id, "t-done");
        assert_eq!(find_tag(&state, "g-prio/High").unwrap().id, "t-high-p");
        assert_eq!(find_tag(&state, "status/high").unwrap().id, "t-high-s");
        assert_eq!(find_tag(&state, "priority/HIGH").unwrap().id, "t-high-p");
        // bare name, exact then case-insensitive, unique across the catalog.
        assert_eq!(find_tag(&state, "Done").unwrap().id, "t-done");
        assert_eq!(find_tag(&state, "DONE").unwrap().id, "t-done");
        assert_eq!(find_tag(&state, "q4-launch").unwrap().id, "t-q4");
        // A name holding a slash is tried whole once group/name fails.
        assert_eq!(find_tag(&state, "a/b").unwrap().id, "t-slash");
    }

    #[test]
    fn find_tag_reports_ambiguity_in_group_slash_form() {
        let state = catalog();
        let err = find_tag(&state, "High").unwrap_err();
        assert_eq!(err.exit_code(), 1);
        let text = err.to_string();
        assert!(text.starts_with("ambiguous: "), "{text}");
        assert!(text.contains("Status/High"), "{text}");
        assert!(text.contains("Priority/High"), "{text}");

        let err = find_tag(&state, "nope").unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("not_found: "), "{text}");
        assert!(
            text.contains("Status/In progress"),
            "lists the catalog: {text}"
        );
        assert!(
            text.contains("q4-launch"),
            "lists ungrouped tags bare: {text}"
        );

        // A group that exists but holds no such tag falls through to the
        // whole-name lookup, then reports not found.
        let err = find_tag(&state, "Status/q4-launch").unwrap_err();
        assert!(err.to_string().starts_with("not_found: "), "{err}");

        let empty = TagStateWire::default();
        let err = find_tag(&empty, "x").unwrap_err();
        assert!(err.to_string().contains("no tags yet"), "{err}");
    }

    #[test]
    fn find_group_by_name_or_id() {
        let state = catalog();
        assert_eq!(find_group(&state, "g-prio").unwrap().id, "g-prio");
        assert_eq!(find_group(&state, "Status").unwrap().id, "g-status");
        assert_eq!(find_group(&state, "priority").unwrap().id, "g-prio");
        let err = find_group(&state, "Owner").unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("not_found: "), "{text}");
        assert!(text.contains("Status, Priority"), "lists groups: {text}");
    }

    #[test]
    fn usage_counts_count_assignments_per_tag() {
        let state = catalog();
        let usage = usage_counts(&state);
        assert_eq!(usage["t-prog"], 2);
        assert_eq!(usage["t-q4"], 2);
        assert_eq!(usage["t-done"], 1);
        assert_eq!(usage.get("t-infra"), None);
    }

    #[test]
    fn tag_list_renders_groups_then_ungrouped_with_counts() {
        let state = catalog();
        let text = render_tag_list(&state, None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Status  (project icon, in sidebar)  id:g-status");
        assert_eq!(
            lines[1],
            "  Status/In progress  blue   loader  2 projects  id:t-prog"
        );
        assert_eq!(
            lines[2],
            "  Status/Done         green  check   1 project  id:t-done"
        );
        assert_eq!(
            lines[3],
            "  Status/High         red            0 projects  id:t-high-s"
        );
        assert_eq!(lines[4], "Priority  id:g-prio");
        assert_eq!(
            lines[5],
            "  Priority/High  red  flame  1 project  id:t-high-p"
        );
        assert_eq!(lines[6], "Tags");
        // No ungrouped tag has an icon, so that column is left out.
        assert_eq!(lines[7], "  q4-launch  purple  2 projects  id:t-q4");
        assert_eq!(lines[8], "  infra      gray    0 projects  id:t-infra");
        assert_eq!(lines[9], "  a/b        cyan    0 projects  id:t-slash");
        assert_eq!(lines.len(), 10);

        // --group prints that group's section only.
        let only = render_tag_list(&state, Some(&state.groups[1]));
        assert_eq!(
            only,
            "Priority  id:g-prio\n  Priority/High  red  flame  1 project  id:t-high-p"
        );

        // An empty group says so; an empty catalog says so.
        let mut bare = TagStateWire::default();
        bare.groups.push(group("g", "Owner", false, false));
        assert_eq!(render_tag_list(&bare, None), "Owner  id:g\n  (no tags)");
        assert_eq!(render_tag_list(&TagStateWire::default(), None), "no tags");
    }

    #[test]
    fn group_list_renders_flags_and_tag_counts() {
        let text = render_group_list(&catalog());
        assert_eq!(
            text,
            "Status    (project icon, in sidebar)  3 tags  id:g-status\n\
             Priority                              1 tag  id:g-prio"
        );
        assert_eq!(render_group_list(&TagStateWire::default()), "no tag groups");
    }

    #[test]
    fn project_tags_renders_each_group_and_the_free_tags() {
        let state = catalog();
        let text = render_project_tags(&state, &project("p1", "Widgets"));
        assert_eq!(
            text,
            "project Widgets  id:p1\n\
             \x20 Status:    In progress  (project icon)\n\
             \x20 Priority:  (none)\n\
             \x20 tags:      q4-launch"
        );
        let text = render_project_tags(&state, &project("p9", "Bare"));
        assert!(text.ends_with("  tags:      (none)"), "{text}");
    }

    #[test]
    fn tag_show_renders_the_group_and_the_carrying_projects() {
        let state = catalog();
        let tag = find_tag(&state, "Status/In progress").unwrap();
        let p1 = project("p1", "Widgets");
        let p2 = project("p2", "Gadgets");
        let text = render_tag_show(&state, tag, &[&p1, &p2]);
        assert_eq!(
            text,
            "Status/In progress  blue  loader  id:t-prog\n\
             \x20 group:    Status  id:g-status\n\
             \x20 projects: 2\n\
             \x20   Widgets  id:p1\n\
             \x20   Gadgets  id:p2"
        );
        let tag = find_tag(&state, "infra").unwrap();
        let text = render_tag_show(&state, tag, &[]);
        assert_eq!(
            text,
            "infra  gray  id:t-infra\n  group:    (ungrouped)\n  projects: 0"
        );
    }

    #[test]
    fn project_list_tag_filter_ands() {
        let state = catalog();
        let keep = projects_with_all_tags(&state, &["Status/In progress".into()]).unwrap();
        assert!(keep("p1") && keep("p2") && !keep("p3"));
        let keep =
            projects_with_all_tags(&state, &["In progress".into(), "q4-launch".into()]).unwrap();
        assert!(keep("p1") && !keep("p2") && !keep("p3"));
        // Two tags of one group AND too: nothing carries both.
        let keep =
            projects_with_all_tags(&state, &["Status/Done".into(), "t-prog".into()]).unwrap();
        assert!(!keep("p1") && !keep("p2") && !keep("p3"));
        // A typo fails before filtering.
        let err = projects_with_all_tags(&state, &["q4-launch".into(), "nope".into()])
            .err()
            .expect("a typo must fail before filtering");
        assert!(err.to_string().starts_with("not_found: "), "{err}");
    }

    fn choose(group: &str, tag: &str) -> Step {
        Step::Choose {
            group_id: group.into(),
            tag_id: tag.into(),
        }
    }

    fn assign(tag: &str, assigned: bool) -> Step {
        Step::Assign {
            tag_id: tag.into(),
            assigned,
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plan_assignments_routes_grouped_and_ungrouped_tags() {
        let state = catalog();
        // Argument order is kept; every address form resolves; a grouped
        // tag is a group choice and an ungrouped one an assignment.
        let steps = plan_tagging(
            &state,
            &strings(&["q4-launch", "Status/Done", "t-high-p", "infra"]),
            false,
        )
        .unwrap();
        assert_eq!(
            steps,
            vec![
                assign("t-q4", true),
                choose("g-status", "t-done"),
                choose("g-prio", "t-high-p"),
                assign("t-infra", true),
            ]
        );
        // The same tag twice, by two addresses, is one step.
        let steps = plan_tagging(
            &state,
            &strings(&["Done", "status/done", "q4-launch", "t-q4"]),
            false,
        )
        .unwrap();
        assert_eq!(
            steps,
            vec![choose("g-status", "t-done"), assign("t-q4", true)]
        );
        // The wire shapes the app's arms read.
        assert_eq!(
            choose("g-status", "t-done").command(),
            "set_project_group_tag"
        );
        assert_eq!(
            choose("g-status", "t-done").args("p1"),
            json!({ "projectId": "p1", "groupId": "g-status", "tagId": "t-done" })
        );
        assert_eq!(assign("t-q4", true).command(), "set_project_tag_assigned");
        assert_eq!(
            assign("t-q4", false).args("p1"),
            json!({ "projectId": "p1", "tagId": "t-q4", "assigned": false })
        );
    }

    #[test]
    fn plan_assignments_rejects_two_tags_of_one_group() {
        let state = catalog();
        let err = plan_tagging(
            &state,
            &strings(&["q4-launch", "Done", "In progress"]),
            false,
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 1);
        let text = err.to_string();
        assert!(text.starts_with("invalid_arguments: "), "{text}");
        assert!(
            text.contains("\"Done\" and \"In progress\" are both in Status"),
            "{text}"
        );
        // The same group's tag under two names is still two tags.
        let err = plan_tagging(&state, &strings(&["t-done", "status/HIGH"]), false).unwrap_err();
        assert!(err.to_string().contains("both in Status"), "{err}");
        // Different groups are fine, and so is an ambiguous name's error.
        assert!(plan_tagging(&state, &strings(&["Status/High", "Priority/High"]), false).is_ok());
        let err = plan_tagging(&state, &strings(&["High"]), true).unwrap_err();
        assert!(err.to_string().starts_with("ambiguous: "), "{err}");
    }

    #[test]
    fn plan_assignments_creates_missing_names_only_with_create() {
        let state = catalog();
        // Without --create an unknown name fails and plans nothing.
        let err = plan_tagging(&state, &strings(&["infra", "q4"]), false).unwrap_err();
        assert!(err.to_string().starts_with("not_found: "), "{err}");
        // With it, the unknown bare name is created, ungrouped, in order;
        // the known ones resolve as before.
        let steps = plan_tagging(&state, &strings(&["infra", "q4", "Done", "Q4"]), true).unwrap();
        assert_eq!(
            steps,
            vec![
                assign("t-infra", true),
                Step::Create { name: "q4".into() },
                choose("g-status", "t-done"),
            ],
            "a second spelling of a name being created is not a second tag"
        );
        assert_eq!(steps[1].command(), "create_tag");
        assert_eq!(
            steps[1].args("p1"),
            json!({ "name": "q4", "assignProjectId": "p1" }),
            "no groupId: --create makes ungrouped tags with an automatic colour"
        );
        // A name a tag already has, in any case, is that tag, not a new one.
        let steps = plan_tagging(&state, &strings(&["INFRA"]), true).unwrap();
        assert_eq!(steps, vec![assign("t-infra", true)]);
        // group/name into an existing group is refused: --create never
        // makes a grouped tag.
        let err = plan_tagging(&state, &strings(&["Status/Blocked"]), true).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("invalid_arguments: "), "{text}");
        assert!(text.contains("names the group Status"), "{text}");
        // A slash with no such group is a whole name, like `a/b` is.
        let steps = plan_tagging(&state, &strings(&["team/x"]), true).unwrap();
        assert_eq!(
            steps,
            vec![Step::Create {
                name: "team/x".into()
            }]
        );
        let err = plan_tagging(&state, &strings(&["  "]), true).unwrap_err();
        assert!(err.to_string().contains("must not be empty"), "{err}");
    }

    #[test]
    fn untag_group_clears_the_choice() {
        let state = catalog();
        let steps = plan_untagging(&state, &[], Some("status")).unwrap();
        assert_eq!(
            steps,
            vec![Step::ClearGroup {
                group_id: "g-status".into()
            }]
        );
        assert_eq!(steps[0].command(), "set_project_group_tag");
        assert_eq!(
            steps[0].args("p1"),
            json!({ "projectId": "p1", "groupId": "g-status", "tagId": null })
        );
        let err = plan_untagging(&state, &[], Some("Owner")).unwrap_err();
        assert!(err.to_string().starts_with("not_found: "), "{err}");

        // Named tags, grouped or not, are removed one by one, once each.
        let steps = plan_untagging(
            &state,
            &strings(&["q4-launch", "Status/Done", "t-q4"]),
            None,
        )
        .unwrap();
        assert_eq!(steps, vec![assign("t-q4", false), assign("t-done", false)]);
        let err = plan_untagging(&state, &strings(&["q4-launch", "nope"]), None).unwrap_err();
        assert!(err.to_string().starts_with("not_found: "), "{err}");
    }

    #[test]
    fn step_lines_say_what_changed_against_the_state_before() {
        let state = catalog();
        let p1 = project("p1", "Widgets");
        // p1 has Status/In progress and q4-launch.
        let steps = vec![
            choose("g-status", "t-done"),
            choose("g-prio", "t-high-p"),
            assign("t-q4", true),
            assign("t-infra", true),
            Step::Create { name: "q4".into() },
        ];
        let created = vec![json!({ "id": "t-new", "name": "q4", "color": "cyan" })];
        assert_eq!(
            render_steps(&state, &p1, &steps, &created, "tagged"),
            "tagged Widgets  id:p1\n\
             \x20 Status: Done\n\
             \x20 Priority: High\n\
             \x20 + q4-launch  (already)\n\
             \x20 + infra\n\
             \x20 + q4  (created)  id:t-new"
        );
        let steps = vec![
            choose("g-status", "t-prog"),
            Step::ClearGroup {
                group_id: "g-prio".into(),
            },
            Step::ClearGroup {
                group_id: "g-status".into(),
            },
            assign("t-q4", false),
            assign("t-done", false),
        ];
        assert_eq!(
            render_steps(&state, &p1, &steps, &[], "untagged"),
            "untagged Widgets  id:p1\n\
             \x20 Status: In progress  (unchanged)\n\
             \x20 Priority: (none)  (unchanged)\n\
             \x20 Status: (none)\n\
             \x20 - q4-launch\n\
             \x20 - Status/Done  (not carried)"
        );
    }

    #[test]
    fn edit_merges_flags_over_the_current_value() {
        let state = catalog();
        let done = find_tag(&state, "Status/Done").unwrap();
        // The app takes the whole value: an unset flag keeps the tag's own.
        assert_eq!(
            plan_edit(done, None, Some(TagColor::Red), IconChange::Keep),
            json!({ "tagId": "t-done", "name": "Done", "color": "red", "icon": "check" })
        );
        assert_eq!(
            plan_edit(done, Some("Shipped"), None, IconChange::Keep),
            json!({ "tagId": "t-done", "name": "Shipped", "color": "green", "icon": "check" })
        );
        assert_eq!(
            plan_edit(done, None, None, IconChange::Remove),
            json!({ "tagId": "t-done", "name": "Done", "color": "green", "icon": null })
        );
        let q4 = find_tag(&state, "q4-launch").unwrap();
        assert_eq!(
            plan_edit(q4, None, None, IconChange::Set("rocket")),
            json!({ "tagId": "t-q4", "name": "q4-launch", "color": "purple", "icon": "rocket" })
        );
        assert_eq!(
            plan_edit(q4, None, None, IconChange::Keep)["icon"],
            Value::Null,
            "a tag without an icon keeps none"
        );
    }

    #[test]
    fn move_counts_conflicts_and_needs_yes() {
        let state = catalog();
        let q4 = find_tag(&state, "q4-launch").unwrap();
        let status = find_group(&state, "Status").unwrap();
        // q4-launch is on p1 (In progress) and p3 (Done): both already hold
        // a Status tag, so both would lose q4-launch.
        let plan = plan_move(&state, q4, Some(status), None).unwrap();
        assert!(plan.cross_scope);
        assert_eq!(plan.conflicts, vec!["p1".to_string(), "p3".to_string()]);
        assert_eq!(
            plan.args(),
            json!({ "tagId": "t-q4", "groupId": "g-status", "index": 3 })
        );
        // infra is carried by nobody: no conflicts.
        let infra = find_tag(&state, "infra").unwrap();
        let plan = plan_move(&state, infra, Some(status), None).unwrap();
        assert!(plan.conflicts.is_empty());
        // Into Priority: p1 has no Priority tag, p3 neither; nothing lost.
        let prio = find_group(&state, "Priority").unwrap();
        let plan = plan_move(&state, q4, Some(prio), None).unwrap();
        assert!(plan.conflicts.is_empty());
        // Out of a group never drops anything, even from busy projects.
        let prog = find_tag(&state, "Status/In progress").unwrap();
        let plan = plan_move(&state, prog, None, None).unwrap();
        assert!(plan.cross_scope && plan.conflicts.is_empty());
        assert_eq!(plan.args()["groupId"], Value::Null);
        // A name the destination already has is refused before any write.
        let high_p = find_tag(&state, "Priority/High").unwrap();
        let err = plan_move(&state, high_p, Some(status), None).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("invalid_arguments: "), "{text}");
        assert!(text.contains("already has a tag named \"High\""), "{text}");
        // Case-insensitively, as the app's unique index is.
        let mut lower = state.clone();
        lower.tags.push(tag("t-done2", None, "done", "gray", None));
        let done2 = find_tag(&lower, "t-done2").unwrap();
        let status = find_group(&lower, "Status").unwrap();
        assert!(plan_move(&lower, done2, Some(status), None).is_err());
    }

    #[test]
    fn move_index_defaults_to_the_end_of_the_destination() {
        let state = catalog();
        let q4 = find_tag(&state, "q4-launch").unwrap();
        let status = find_group(&state, "Status").unwrap();
        // Status holds three tags: the end is index 3.
        assert_eq!(plan_move(&state, q4, Some(status), None).unwrap().index, 3);
        // An explicit index is kept, and clamped to the end.
        assert_eq!(
            plan_move(&state, q4, Some(status), Some(1)).unwrap().index,
            1
        );
        assert_eq!(
            plan_move(&state, q4, Some(status), Some(99)).unwrap().index,
            3
        );
        // Within its own scope the tag itself is not counted: Status has two
        // others, so Done's end is 2, and "where it is" is not a conflict.
        let done = find_tag(&state, "Status/Done").unwrap();
        let plan = plan_move(&state, done, Some(status), None).unwrap();
        assert!(!plan.cross_scope);
        assert_eq!(plan.index, 2);
        assert!(plan.conflicts.is_empty());
        let plan = plan_move(&state, done, Some(status), Some(0)).unwrap();
        assert_eq!(
            plan.args(),
            json!({ "tagId": "t-done", "groupId": "g-status", "index": 0 })
        );
        // To the ungrouped list: three others there.
        assert_eq!(plan_move(&state, done, None, None).unwrap().index, 3);
        // Reordering the ungrouped list counts the others only.
        assert_eq!(plan_move(&state, q4, None, None).unwrap().index, 2);
    }

    #[test]
    fn reorder_requires_every_group_once() {
        let state = catalog();
        assert_eq!(
            plan_group_order(&state, &strings(&["priority", "g-status"])).unwrap(),
            vec!["g-prio".to_string(), "g-status".to_string()]
        );
        let err = plan_group_order(&state, &strings(&["Priority"])).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("invalid_arguments: "), "{text}");
        assert!(text.contains("missing: Status"), "{text}");
        let err =
            plan_group_order(&state, &strings(&["Priority", "Status", "g-prio"])).unwrap_err();
        assert!(
            err.to_string().contains("Priority is listed twice"),
            "{err}"
        );
        let err = plan_group_order(&state, &strings(&["Priority", "Owner"])).unwrap_err();
        assert!(err.to_string().starts_with("not_found: "), "{err}");
    }

    #[test]
    fn group_templates_match_the_frontend() {
        // The templates are copied from the frontend, which this crate
        // cannot import; read that file's `TAG_GROUP_TEMPLATES` and compare.
        let source = include_str!("../../../../src/lib/features/tags/tagTemplates.ts");
        let start = source.find("export const TAG_GROUP_TEMPLATES").unwrap();
        let source = &source[start..];
        let section = |id: &str| -> Vec<(String, String, String)> {
            let from = source.find(&format!("id: '{id}'")).unwrap();
            let body = &source[from..];
            let end = body[1..].find("id: '").map(|i| i + 1).unwrap_or(body.len());
            body[..end]
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    let line = line.strip_prefix("{ name: '")?;
                    let (name, rest) = line.split_once("', icon: '")?;
                    let (icon, rest) = rest.split_once("', color: '")?;
                    let (color, _) = rest.split_once('\'')?;
                    Some((name.to_string(), icon.to_string(), color.to_string()))
                })
                .collect()
        };
        let as_owned = |template: &[TemplateTag]| -> Vec<(String, String, String)> {
            template
                .iter()
                .map(|(n, i, c)| (n.to_string(), i.to_string(), c.to_string()))
                .collect()
        };
        let status = section("status");
        assert_eq!(status.len(), 7, "the parser found the Status rows");
        assert_eq!(as_owned(STATUS_TEMPLATE), status);
        let priority = section("priority");
        assert_eq!(priority.len(), 4, "the parser found the Priority rows");
        assert_eq!(as_owned(PRIORITY_TEMPLATE), priority);
        assert!(section("blank").is_empty());
        assert_eq!(template_tags(GroupTemplate::Status), STATUS_TEMPLATE);
        assert_eq!(template_tags(GroupTemplate::Priority), PRIORITY_TEMPLATE);
        assert!(template_tags(GroupTemplate::Blank).is_empty());
        // The wire rows the app's `NewTag` reads.
        assert_eq!(
            template_new_tags(GroupTemplate::Priority)[0],
            json!({ "name": "P0", "icon": "chevrons-up", "color": "red" })
        );
    }

    #[test]
    fn tag_commands_match_the_control_server_allowlist() {
        // Every tag command the app allowlists is one the 404 hint names,
        // and nothing else is.
        let source = include_str!("../../../src/control_server.rs");
        let start = source.find("pub const ALLOWED_COMMANDS").unwrap();
        let end = start + source[start..].find("];").unwrap();
        let listed: Vec<&str> = source[start..end]
            .lines()
            .filter_map(|line| line.trim().strip_prefix('"'))
            .filter_map(|rest| rest.split('"').next())
            .collect();
        for command in TAG_COMMANDS {
            assert!(listed.contains(command), "{command} is not allowlisted");
        }
        let allowlisted_tag_commands: Vec<&str> = listed
            .iter()
            .copied()
            .filter(|c| c.contains("tag"))
            .collect();
        assert_eq!(allowlisted_tag_commands, TAG_COMMANDS.to_vec());
    }
}
