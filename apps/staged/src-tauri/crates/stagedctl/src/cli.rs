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
and is revoked when the session ends. Outside a session it tries each
running Staged instance (~/.staged/control/<pid>/discovery.json), newest
first, until one answers, and authenticates with a connected-app grant
if `stagedctl connect` has been approved on this machine; without any
credential the app refuses commands (exit 4). Control files are read
only when they are owner-private (0600 inside a 0700 directory, no
symlinks).

Addressing:
  <project>   a project name or id, as printed by `stagedctl project list`
  --repo      a repo address of the form <owner>/<repo>[:<subpath>], as
              printed by `stagedctl project get <project>`

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
     expired, or the command addresses something outside the
     credential's scope (report it; do not retry)";

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

Not available from inside a Staged-spawned session: a session's
credential is confined to its own project and cannot request
connected-app access (exit 4, `scope_denied`). If a task needs wider
access, ask the user to run this command from their own terminal.")]
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

Not available from inside a Staged-spawned session (exit 4,
`scope_denied`): a session's credential is its own, not a connected-app
grant.")]
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
id are accepted anywhere a <project> argument is expected.")]
    List,

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

/// Render `--help` for the root and every noun and verb, in a stable order,
/// for the help-pinning test.
pub fn render_all_help() -> String {
    use clap::CommandFactory;

    let mut root = Cli::command();
    root.build();

    let mut out = String::new();
    let mut render = |cmd: &mut clap::Command, path: &str| {
        out.push_str(&format!("=== {path} --help ===\n"));
        out.push_str(&cmd.render_long_help().to_string());
        out.push('\n');
    };

    render(&mut root.clone(), "stagedctl");
    let nouns: Vec<String> = root
        .get_subcommands()
        .filter(|c| c.get_name() != "help")
        .map(|c| c.get_name().to_string())
        .collect();
    for noun in nouns {
        let mut noun_cmd = root.find_subcommand(&noun).unwrap().clone();
        noun_cmd.build();
        render(&mut noun_cmd.clone(), &format!("stagedctl {noun}"));
        let verbs: Vec<String> = noun_cmd
            .get_subcommands()
            .filter(|c| c.get_name() != "help")
            .map(|c| c.get_name().to_string())
            .collect();
        for verb in verbs {
            let mut verb_cmd = noun_cmd.find_subcommand(&verb).unwrap().clone();
            verb_cmd.build();
            render(&mut verb_cmd, &format!("stagedctl {noun} {verb}"));
        }
    }
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
