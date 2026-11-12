//! The clap command tree. Help prose is written for agents: it teaches the
//! addressing scheme, the hashtag reference forms, and what each verb does
//! to the app, and it is byte-pinned by tests so it can never drift from
//! what ships.

use clap::{Parser, Subcommand, ValueEnum};

const ROOT_LONG_ABOUT: &str = "\
Control a running Staged app from the command line.

stagedctl is built for agents running inside Staged-spawned sessions: the
app injects $STAGEDCTL_LOCK into those sessions — a per-session credential
file — and stagedctl uses it to find the app and authenticate with zero
configuration. The credential is confined to the session's own project
and is revoked when the session ends; it is the only credential a
spawned session is issued, and a session must not try to obtain wider
access (see `stagedctl connect --help`). Outside a session it tries each
running Staged instance (~/.staged/control/<pid>/discovery.json), newest
first, until one answers, and authenticates with a connected-app grant
if `stagedctl connect` has been approved on this machine; without any
credential the app refuses commands (exit 4). Control files are read
only when they are owner-private (0600 inside a 0700 directory, no
symlinks). An empty $STAGEDCTL_LOCK is an error (exit 3), never a
fallback to the instance file.

Addressing:
  <project>   a project name or id, as printed by `stagedctl project list`
  --repo      a repo address of the form <owner>/<repo>[:<subpath>], as
              printed by `stagedctl project get <project>`
  <tag>       a tag id, <group>/<name>, or a bare tag name when only one
              tag has it, as printed by `stagedctl tag list`
  <group>     a tag group name or id, as printed by `stagedctl tag list`

Ids print in hashtag reference form (#note:<id>, #project-note:<id>,
#review:<id>, #commit:<sha>) so they can be pasted directly into session
prompts. Every command also accepts --json for machine-readable output.

Exit codes:
  0  success
  1  command failed (stderr carries `code: message`)
  2  transport fault talking to the app
  3  no running Staged app, or the app was updated (restart Staged)
  4  not authorized: no credential (run `stagedctl connect` outside a
     session), the request was denied, the credential was revoked or
     expired, a grant file no longer matches the app's record (run
     `stagedctl disconnect` then `stagedctl connect`), or the command
     addresses something outside the credential's scope (report it; do
     not retry)";

#[derive(Parser, Debug)]
#[command(
    name = "stagedctl",
    version,
    about = "Control a running Staged app from the command line",
    long_about = ROOT_LONG_ABOUT
)]
pub struct Cli {
    /// Path to the app's control file (defaults to $STAGEDCTL_LOCK, then
    /// live ~/.staged/control/<pid>/discovery.json files, newest first)
    #[arg(long, global = true, value_name = "PATH")]
    pub lock_path: Option<std::path::PathBuf>,

    /// Print machine-readable JSON on stdout instead of human-readable text
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Noun,
}

#[derive(Subcommand, Debug)]
pub enum Noun {
    /// Inspect projects and the repos attached to them
    #[command(alias = "projects", subcommand)]
    Project(ProjectVerb),

    /// Read project and repo timelines (commits, notes, reviews, images)
    #[command(alias = "timelines", subcommand)]
    Timeline(TimelineVerb),

    /// Start, observe, wait for, and cancel agent sessions
    #[command(alias = "sessions", subcommand)]
    Session(SessionVerb),

    /// Attach images to a repo's timeline
    #[command(alias = "images", subcommand)]
    Image(ImageVerb),

    /// Read the tag catalog: tag groups, tags, and which projects carry them
    #[command(alias = "tags", subcommand)]
    Tag(TagVerb),

    /// Ask the Staged user to approve this client (outside a spawned session)
    #[command(long_about = "\
Ask the Staged user to approve this client. Use this when stagedctl runs
outside a Staged-spawned session (no $STAGEDCTL_LOCK) — from a terminal,
a script, or an agent the user started themselves.

The request appears in the Staged window as a consent dialog naming
--client, the projects it would reach, and whether it may write; the
command waits (up to --timeout seconds) for the answer. Approval stores
a grant on this machine (~/.staged/control/grants/, owner-private) that
later invocations use without asking again; it lasts thirty days from
its last use and can be revoked at any time in the app under
Settings ▸ Connected apps, or with `stagedctl disconnect`.

Exit 4 with `authorization_denied` when the user declines. A request
left unanswered for five minutes expires in the app; the default
--timeout gives up a minute earlier so an approval never lands after
this command has stopped listening for it.

Not for Staged-spawned sessions. A spawned session is issued only its
own project-scoped credential and must not try to obtain wider access:
the app refuses `connect` (exit 4, `scope_denied`) when the caller
presents a session credential or when its process is traceable to a
running session's agent, and the CLI refuses it locally under a
per-session $STAGEDCTL_LOCK. That check is a strong default, not a
sandbox — same-user processes are not a hard security boundary — and
the rule holds either way. If a task needs wider access, say so and let
the user run this command from their own terminal.")]
    Connect {
        /// The name shown to the user in the consent dialog
        #[arg(long, value_name = "NAME")]
        client: String,

        /// Confine the grant to one project (name or id); default: every
        /// project
        #[arg(long, value_name = "PROJECT")]
        project: Option<String>,

        /// Ask for read access only (no starting, cancelling or attaching)
        #[arg(long)]
        read_only: bool,

        /// Seconds to wait for the user's answer
        #[arg(long, value_name = "SECONDS", default_value_t = 240)]
        timeout: u64,
    },

    /// Forget this machine's connected-app grants and revoke them in Staged
    #[command(long_about = "\
Forget the connected-app grants stored on this machine and, when a Staged
app is running, revoke them there too so their live credentials stop
working at once. With --client only the grants approved under that name
are removed. Grants a Staged app cannot be reached for are still removed
locally; revoke them in Settings ▸ Connected apps if needed.

Not for Staged-spawned sessions (exit 4, `scope_denied`, when the
caller presents a session credential or its process is traceable to a
running session): a session's credential is its own, not a
connected-app grant.")]
    Disconnect {
        /// Only the grants approved under this client name
        #[arg(long, value_name = "NAME")]
        client: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProjectVerb {
    /// List every project in the app
    #[command(long_about = "\
List every project in the app.

Each line shows the project name, its primary repo address, where its
branches run (local or remote), and the project id. Both the name and the
id are accepted anywhere a <project> argument is expected.

With --tag, only the projects carrying that tag are listed. Repeating the
flag narrows further: a project must carry every tag named (AND), even
when the tags are in the same group.")]
    List {
        /// Only projects carrying this tag (id, <group>/<name>, or a bare
        /// name); repeat to require several
        #[arg(long, value_name = "TAG")]
        tag: Vec<String>,
    },

    /// Show one project's repos and branches
    #[command(long_about = "\
Show one project: every attached repo with its address, and every branch
with its base branch, PR status, and worktree location.

The printed repo addresses (<owner>/<repo>[:<subpath>]) are exactly what
the --repo flag of `timeline show`, `session start`, `session list`, and
`image add` expects.")]
    Get {
        /// Project name or id (see `stagedctl project list`)
        project: String,
    },

    /// Show one project's tags: its choice in each group, then its free tags
    #[command(long_about = "\
Show one project's tags.

One line per tag group, in the app's group order, with the tag the project
has in that group or (none). A last line lists the project's ungrouped
tags. The group marked as the project icon is the one whose tag draws the
project's icon in the sidebar and grid.")]
    Tags {
        /// Project name or id (see `stagedctl project list`)
        project: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TagVerb {
    /// List every tag group and its tags, then the ungrouped tags
    #[command(long_about = "\
List the tag catalog: every tag group in the app's order with its tags,
then the ungrouped tags under a Tags heading.

A group line shows the group name, whether its tag draws the project icon
and whether its board is pinned to the sidebar, and the group id. A tag
line shows the tag as <group>/<name> (or the bare name for an ungrouped
tag), its colour, its icon if it has one, how many projects carry it, and
the tag id. Any of those forms is accepted where a <tag> argument is
expected; a bare name works when only one tag in the catalog has it.

With --group, only that group's tags are listed.")]
    List {
        /// Only this group's tags (group name or id)
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
    },

    /// Show one tag and the projects carrying it
    #[command(long_about = "\
Show one tag: its group (if any), colour and icon, and every project that
carries it.

Under a project-scoped credential the project list holds at most the
credential's own project.")]
    Show {
        /// Tag id, <group>/<name>, or a bare tag name (see `stagedctl tag list`)
        tag: String,
    },

    /// Inspect tag groups
    #[command(alias = "groups", subcommand)]
    Group(TagGroupVerb),
}

#[derive(Subcommand, Debug)]
pub enum TagGroupVerb {
    /// List every tag group with its flags and tag count
    #[command(long_about = "\
List every tag group in the app's order.

Each line shows the group name, whether its tag draws the project icon
(at most one group does), whether its board is pinned to the projects
sidebar, how many tags it holds, and the group id. Both the name and the
id are accepted anywhere a <group> argument is expected.")]
    List,
}

#[derive(Subcommand, Debug)]
pub enum TimelineVerb {
    /// Show a project's timeline, or a single repo's with --repo
    #[command(long_about = "\
Show a project's timeline, or a single repo's with --repo.

Without --repo this prints the whole project: its project notes, then one
timeline per repo. With --repo it prints only that repo's timeline. Items
are ordered oldest to newest and print with hashtag references (#note:<id>,
#review:<id>, #commit:<sha>, #project-note:<id>) that can be pasted into
session prompts. An item produced by a still-running session carries a
[session <status>] suffix.

Timelines are the way to find finished work: a completed session shows up
here as its artifact (commit, note, review, or image), not in
`session list`.")]
    Show {
        /// Project name or id (see `stagedctl project list`)
        project: String,

        /// Repo address <owner>/<repo>[:<subpath>] (see `stagedctl project get`)
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,

        /// Branch name, only needed when the repo has more than one branch
        /// in this project
        #[arg(long, value_name = "BRANCH")]
        branch: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionVerb {
    /// Start an agent session (spends tokens; visible in the app)
    #[command(long_about = "\
Start an agent session. This spawns a real agent that edits the worktree
and spends tokens; the session appears immediately in the app's timeline
and can be cancelled with `stagedctl session cancel`.

With --repo, starts a session on that repo's branch; --type is required
and picks the artifact the session produces: a commit, a branch note, or
a code review. Without --repo, starts a project-level session, which
researches across the whole project, writes a project note, and may fan
work out into the project's repos; --type is not allowed there.

Prints the new session id and the artifact's hashtag reference. Follow up
with `stagedctl session wait <sessionId>` to block until it finishes.")]
    Start {
        /// Project name or id (see `stagedctl project list`)
        project: String,

        /// Repo address <owner>/<repo>[:<subpath>]; omit to start a
        /// project-level session
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,

        /// Branch name, only needed when the repo has more than one branch
        /// in this project
        #[arg(long, value_name = "BRANCH")]
        branch: Option<String>,

        /// What the session produces (required with --repo, invalid without)
        #[arg(long, value_name = "TYPE", value_enum)]
        r#type: Option<SessionType>,

        /// The prompt for the agent
        #[arg(long, value_name = "TEXT")]
        prompt: String,
    },

    /// Show one session's status (works for finished sessions too)
    #[command(long_about = "\
Show one session: status (queued, running, disconnected, completed, error,
cancelled), why it ended, its provider, prompt, and working directory.

disconnected means Staged lost its connection to a remote (Blox) session
mid-turn. It is not finished: the remote turn may still be running, and
the session returns to running when Staged reattaches it.

Session ids come from `session start`, `session list`, or the [session ...]
suffixes in `timeline show`.")]
    Status {
        /// Session id
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
    },

    /// Block until a session finishes (default timeout 240s)
    #[command(long_about = "\
Poll the app until the session reaches a terminal state: completed, error,
or cancelled. Waiting continues through queued, running, and disconnected
(a remote session whose connection dropped; its turn may still be running).

Exit 0 when the session completes. Exits 1 with `session_error` or
`session_cancelled` when it ends any other way, and with `wait_timeout`
when --timeout elapses — in that case the session KEEPS RUNNING; waiting
never cancels anything. If the session is disconnected when --timeout
elapses, the code is `session_disconnected` instead. A status this
stagedctl doesn't recognize exits 1 with `unknown_status`.")]
    Wait {
        /// Session id
        #[arg(value_name = "SESSION_ID")]
        session_id: String,

        /// Seconds to wait before giving up (the session keeps running)
        #[arg(long, value_name = "SECONDS", default_value_t = 240)]
        timeout: u64,
    },

    /// Cancel a queued or running session
    #[command(long_about = "\
Cancel a queued or running session. The session's pending artifact stays
in the timeline marked as cancelled. Undo by starting a new session.")]
    Cancel {
        /// Session id
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
    },

    /// List queued, running, and disconnected sessions
    #[command(long_about = "\
List every queued, running, and disconnected session in the app,
optionally narrowed to one project or one repo. Completed sessions are not
listed — find them through their artifact in `timeline show`, or directly
with `session status <id>`.")]
    List {
        /// Only sessions in this project (name or id)
        #[arg(long, value_name = "PROJECT")]
        project: Option<String>,

        /// Only sessions on this repo's branches; requires --project
        #[arg(long, value_name = "REPO")]
        repo: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ImageVerb {
    /// Attach an image file to a repo's timeline
    #[command(long_about = "\
Attach a local image file to a repo's timeline, where sessions on that
branch can see it and users can view or delete it.

The file is read from this machine's filesystem: it must exist, be at most
10 MB, and be a png, jpg, jpeg, gif, or webp.")]
    Add {
        /// Project name or id (see `stagedctl project list`)
        project: String,

        /// Repo address <owner>/<repo>[:<subpath>] (see `stagedctl project get`)
        #[arg(long, value_name = "REPO")]
        repo: String,

        /// Branch name, only needed when the repo has more than one branch
        /// in this project
        #[arg(long, value_name = "BRANCH")]
        branch: Option<String>,

        /// Path to the image file
        #[arg(long, value_name = "PATH")]
        file: std::path::PathBuf,
    },
}

/// What a repo-scoped session produces. Mirrors the app's
/// `BranchSessionType` wire values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SessionType {
    /// Investigate and write a note on the branch
    Note,
    /// Make changes and commit them
    Commit,
    /// Review a commit and leave comments
    Review,
}

impl SessionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Commit => "commit",
            Self::Review => "review",
        }
    }
}

/// Render `--help` for the root and every noun, verb and sub-noun verb, in
/// a stable order, for the help-pinning test.
pub fn render_all_help() -> String {
    use clap::CommandFactory;

    fn walk(cmd: &mut clap::Command, path: &str, out: &mut String) {
        cmd.build();
        out.push_str(&format!("=== {path} --help ===\n"));
        out.push_str(&cmd.clone().render_long_help().to_string());
        out.push('\n');
        let children: Vec<String> = cmd
            .get_subcommands()
            .filter(|c| c.get_name() != "help")
            .map(|c| c.get_name().to_string())
            .collect();
        for child in children {
            let mut child_cmd = cmd.find_subcommand(&child).unwrap().clone();
            walk(&mut child_cmd, &format!("{path} {child}"), out);
        }
    }

    let mut out = String::new();
    walk(&mut Cli::command(), "stagedctl", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn plural_noun_aliases_parse_and_stay_hidden() {
        for (plural, singular) in [
            ("projects", "list"),
            ("timelines", "show"),
            ("sessions", "list"),
            ("images", "add"),
            ("tags", "list"),
        ] {
            let args: Vec<&str> = match (plural, singular) {
                ("images", _) => vec![
                    "stagedctl",
                    plural,
                    "add",
                    "proj",
                    "--repo",
                    "o/r",
                    "--file",
                    "x.png",
                ],
                ("timelines", _) => vec!["stagedctl", plural, "show", "proj"],
                _ => vec!["stagedctl", plural, singular],
            };
            assert!(
                Cli::try_parse_from(&args).is_ok(),
                "plural alias {plural} should parse: {args:?}"
            );
        }

        // Hidden: every noun has its plural alias, and none are visible.
        // The verb-shaped top-level commands (connect, disconnect) take no
        // subcommands and have no plural.
        for noun in Cli::command()
            .get_subcommands()
            .filter(|c| c.get_name() != "help")
        {
            let aliases: Vec<&str> = noun.get_all_aliases().collect();
            if !noun.has_subcommands() {
                assert!(
                    aliases.is_empty(),
                    "{} is a verb, not a noun; no alias",
                    noun.get_name()
                );
                continue;
            }
            assert_eq!(
                aliases,
                vec![format!("{}s", noun.get_name())],
                "noun {} should have exactly its plural as a hidden alias",
                noun.get_name()
            );
            assert_eq!(
                noun.get_visible_aliases().count(),
                0,
                "aliases on {} must stay hidden in help",
                noun.get_name()
            );
        }
    }

    #[test]
    fn connect_and_disconnect_parse_their_flags() {
        let cli = Cli::try_parse_from([
            "stagedctl",
            "connect",
            "--client",
            "my agent",
            "--project",
            "Widgets",
            "--read-only",
            "--timeout",
            "30",
        ])
        .unwrap();
        match cli.command {
            Noun::Connect {
                client,
                project,
                read_only,
                timeout,
            } => {
                assert_eq!(client, "my agent");
                assert_eq!(project.as_deref(), Some("Widgets"));
                assert!(read_only);
                assert_eq!(timeout, 30);
            }
            other => panic!("parsed {other:?}"),
        }
        let cli = Cli::try_parse_from(["stagedctl", "connect", "--client", "x"]).unwrap();
        match cli.command {
            Noun::Connect {
                project,
                read_only,
                timeout,
                ..
            } => {
                assert_eq!(project, None);
                assert!(!read_only);
                assert_eq!(
                    timeout, 240,
                    "a minute inside the request's five-minute lifetime in the app, so the \
                     CLI gives up before an approval could land uncollected"
                );
            }
            other => panic!("parsed {other:?}"),
        }
        assert!(
            Cli::try_parse_from(["stagedctl", "connect"]).is_err(),
            "--client is required"
        );
        let cli = Cli::try_parse_from(["stagedctl", "disconnect"]).unwrap();
        assert!(matches!(cli.command, Noun::Disconnect { client: None }));
        let cli = Cli::try_parse_from(["stagedctl", "disconnect", "--client", "x"]).unwrap();
        assert!(matches!(cli.command, Noun::Disconnect { client: Some(c) } if c == "x"));
    }

    #[test]
    fn tag_verbs_parse_their_arguments() {
        let cli = Cli::try_parse_from(["stagedctl", "tag", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Noun::Tag(TagVerb::List { group: None })
        ));
        let cli = Cli::try_parse_from(["stagedctl", "tag", "list", "--group", "Status"]).unwrap();
        assert!(
            matches!(cli.command, Noun::Tag(TagVerb::List { group: Some(g) }) if g == "Status")
        );
        let cli = Cli::try_parse_from(["stagedctl", "tag", "show", "Status/Done"]).unwrap();
        assert!(matches!(cli.command, Noun::Tag(TagVerb::Show { tag }) if tag == "Status/Done"));
        for groups in ["group", "groups"] {
            let cli = Cli::try_parse_from(["stagedctl", "tag", groups, "list"]).unwrap();
            assert!(matches!(
                cli.command,
                Noun::Tag(TagVerb::Group(TagGroupVerb::List))
            ));
        }
        let cli = Cli::try_parse_from(["stagedctl", "project", "tags", "Widgets"]).unwrap();
        assert!(
            matches!(cli.command, Noun::Project(ProjectVerb::Tags { project }) if project == "Widgets")
        );

        // `project list` keeps working bare, and --tag repeats.
        let cli = Cli::try_parse_from(["stagedctl", "project", "list"]).unwrap();
        assert!(matches!(cli.command, Noun::Project(ProjectVerb::List { tag }) if tag.is_empty()));
        let cli = Cli::try_parse_from([
            "stagedctl",
            "project",
            "list",
            "--tag",
            "status/done",
            "--tag",
            "q4",
        ])
        .unwrap();
        match cli.command {
            Noun::Project(ProjectVerb::List { tag }) => assert_eq!(tag, vec!["status/done", "q4"]),
            other => panic!("parsed {other:?}"),
        }
    }

    #[test]
    fn help_rendering_reaches_the_tag_group_verbs() {
        let help = render_all_help();
        assert!(
            help.contains("=== stagedctl tag group --help ==="),
            "{help}"
        );
        assert!(
            help.contains("=== stagedctl tag group list --help ==="),
            "{help}"
        );
    }

    #[test]
    fn session_type_values_match_the_wire() {
        assert_eq!(SessionType::Note.as_str(), "note");
        assert_eq!(SessionType::Commit.as_str(), "commit");
        assert_eq!(SessionType::Review.as_str(), "review");
        // clap's lowercase value names must equal the wire values.
        for (value, expected) in [("note", SessionType::Note), ("commit", SessionType::Commit)] {
            let cli = Cli::try_parse_from([
                "stagedctl",
                "session",
                "start",
                "p",
                "--repo",
                "o/r",
                "--type",
                value,
                "--prompt",
                "x",
            ])
            .unwrap();
            match cli.command {
                Noun::Session(SessionVerb::Start { r#type, .. }) => {
                    assert_eq!(r#type, Some(expected))
                }
                other => panic!("unexpected parse: {other:?}"),
            }
        }
    }
}
