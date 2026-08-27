//! The clap command tree. Help prose is written for agents: it teaches the
//! addressing scheme, the hashtag reference forms, and what each verb does
//! to the app, and it is byte-pinned by tests so it can never drift from
//! what ships.

use clap::{Parser, Subcommand, ValueEnum};

const ROOT_LONG_ABOUT: &str = "\
Control a running Staged app from the command line.

stagedctl is built for agents running inside Staged-spawned sessions: the
app injects $STAGEDCTL_LOCK into those sessions, and stagedctl uses it to
find the app with zero configuration. Outside a session it tries each
running Staged instance (control-<pid>.json under ~/.staged), newest
first, until one answers.

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
  3  no running Staged app, or the app was updated (restart Staged)";

#[derive(Parser, Debug)]
#[command(
    name = "stagedctl",
    version,
    about = "Control a running Staged app from the command line",
    long_about = ROOT_LONG_ABOUT
)]
pub struct Cli {
    /// Path to the app's control file (defaults to $STAGEDCTL_LOCK, then
    /// live control-<pid>.json files under ~/.staged, newest first)
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
Show one session: status (queued, running, completed, error, cancelled),
why it ended, its provider, prompt, and working directory.

Session ids come from `session start`, `session list`, or the [session ...]
suffixes in `timeline show`.")]
    Status {
        /// Session id
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
    },

    /// Block until a session finishes (default timeout 240s)
    #[command(long_about = "\
Poll the app until the session reaches a terminal state.

Exit 0 when the session completes. Exits 1 with `session_error` or
`session_cancelled` when it ends any other way, and with `wait_timeout`
when --timeout elapses — in that case the session KEEPS RUNNING; waiting
never cancels anything.")]
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

    /// List queued and running sessions
    #[command(long_about = "\
List every queued and running session in the app, optionally narrowed to
one project or one repo. Completed sessions are not listed — find them
through their artifact in `timeline show`, or directly with
`session status <id>`.")]
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
        for noun in Cli::command()
            .get_subcommands()
            .filter(|c| c.get_name() != "help")
        {
            let aliases: Vec<&str> = noun.get_all_aliases().collect();
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
