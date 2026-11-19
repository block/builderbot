# stagedctl: driving Staged from an agent session

`stagedctl` is the control CLI for the Staged desktop app. Sessions spawned
by Staged receive everything needed to use it with zero configuration:

- `stagedctl` is on `PATH` (the exact binary path is also in `$STAGEDCTL_BIN`);
- `$STAGEDCTL_LOCK` points at this session's credential file (under
  `~/.staged/control/<pid>/sessions/`), so the CLI always talks to the
  instance that spawned the session, even when several are running, and
  authenticates as this session. The credential covers the session's own
  project and the projects the session created with
  `stagedctl project create`, and is revoked when the session ends.

Outside a Staged-spawned session (a terminal, a script, an agent the user
started themselves) there is no `$STAGEDCTL_LOCK`. Run
`stagedctl connect --client <name>` once: the Staged user sees a consent
dialog and, on approval, a grant is stored on this machine
(`~/.staged/control/grants/`, owner-private) that later invocations use
without asking again. It lasts thirty days from its last use; the user can
revoke it any time under Settings ▸ Connected apps, and
`stagedctl disconnect` forgets and revokes it from this side.

Outside a session, `stagedctl` is on `PATH` only where the user put it:

- A source install (`install.sh`) links `/usr/local/bin/stagedctl` to the
  copy inside `/Applications/Staged.app`.
- A release install links nothing. The user presses **Add to PATH** under
  Settings ▸ Connected apps ▸ Command line, and Staged links the copy
  inside the running app into `/usr/local/bin`, or into `~/.local/bin`
  when `/usr/local/bin` cannot be written. It reports anything already at
  that path instead of overwriting it, tells the user when the directory
  is not on their `PATH`, and refuses while macOS runs the app from a
  temporary App Translocation path or from a development build.
- The fallback is a link made by hand:
  `ln -s /Applications/Staged.app/Contents/MacOS/stagedctl <a directory on PATH>/stagedctl`.
  A link, never a copy: `stagedctl` finds the app to start from its own
  location inside the bundle.

Add to PATH is the user's action in the app's Settings. There is no
stagedctl verb for it and a session cannot run it; a session does not
need it, because its own `stagedctl` is already on `PATH`. If the user
reports `stagedctl: command not found` in their own terminal, point them
at that Settings action.

## The boundary for spawned sessions

A spawned session is issued exactly one credential: its own. It covers the
session's own project and the projects the session created, and nothing
else: every other project is refused (exit 4, `scope_denied`) or reads as
`not_found`, and `stagedctl project list` shows only what the credential
covers. A later run of the same session, after a follow-up message or a
resume, gets a fresh credential that still reaches what an earlier run
created. That is the credential to use, as is. Do not try to obtain wider
access from inside a session — not by running `stagedctl connect`, not by
unsetting or emptying `$STAGEDCTL_LOCK` to reach the instance file, and not
by using a connected-app grant the user approved for something else.

The app enforces this where it can see it: `stagedctl connect` and
`disconnect`, and reconnects to a connected-app grant, are refused (exit 4,
`scope_denied`) when the caller presents a session credential *or* when the
calling process is traceable to a running session's agent process. Be
clear about what that is: processes running as the same user are not a hard
security boundary, so this is a strong default and an honest attribution,
not a sandbox. The rule stands regardless of whether a given attempt would
be caught. If a task genuinely needs access beyond this session's project
and the projects it created, say so in your output and let the user run
`stagedctl connect` from their own terminal.

## Discover, don't memorize

The CLI's own help is the single authoritative reference, and the only one
guaranteed to match the binary you are running:

1. `stagedctl --help` — the noun tree, addressing scheme, and exit codes.
2. `stagedctl <noun> --help` — the verbs available on a noun.
3. `stagedctl <noun> <verb> --help` — the authoritative flags for a verb.

Never guess flags or replay invocations from prior transcripts or documents
(including this one); if help output disagrees with anything you remember,
the help output wins.

Add `--json` to any command for machine-readable output (the app's raw
payloads). Human-readable output prints ids in hashtag reference form
(`#note:<id>`, `#review:<id>`, …), which can be pasted verbatim into Staged
prompts.

## Safety invariant

Every stagedctl verb corresponds to something visible in the Staged UI and
is either read-only or reversible from that UI. Use this as a self-check:
if a verb you are looking for seems to be missing (deletes, pushes, history
rewrites), it is deliberately absent — do not work around it by editing
files under `~/.staged` (in particular, never write to `~/.staged/data.db`;
the app owns that state).

Verbs that start sessions spawn real agents, which cost real money. They
appear immediately in the UI, where the user can watch and cancel them —
start sessions only when the task genuinely calls for it.

Showing a project (`stagedctl project open`) fits the invariant too: the
window switches in front of the user, who can switch back, and all it
records is the project's read mark and which project was shown last. It
is not available to a spawned session; see "Showing a project in the
app" below.

Creating a project and attaching a repo (`stagedctl project create`,
`stagedctl project add-repo`) fit the invariant: the project and the repo
show at once in the app, where the user can delete the project or remove
the repo. Neither can be undone from the CLI. Both do more than write rows,
on the user's machine and account:

- A local repo is cloned under `~/.staged/repos` on first use, a worktree
  is checked out for its branch, and the repo's setup actions run in that
  worktree. All of it runs in the background after the command returns, so
  a failure shows in the app and its log, not in the command's exit code.
  `stagedctl project get` shows the repo as soon as the command returns and
  its `worktree:` path once the checkout exists. A session started on the
  repo before then is queued and starts when the worktree is ready.
- `--location remote` provisions a Blox workspace for the first repo, and a
  repo added to a remote project is cloned into its running workspace. This
  needs `sq` on the machine (the command is refused without it) and an open
  Staged window, and a workspace costs money while it runs.
- `--pr` saves the pull request's description as a project note. So does
  `--branch` when the repo has an open pull request from that branch.

Create a project only when the task calls for one. The app checks the repo
before it writes anything: it must exist on GitHub, a subpath must be a
directory in it, and the root of a monorepo is refused unless
`--allow-monorepo-root` is passed (prefer a subpath). A project name
already in use is refused. The allowance is shared along a chain of
sessions: a session and every session started from it, directly or through
another, may together hold at most 10 created projects, and the user frees
a slot by deleting one of them in the app. Starting a session to create
more does not help, whether with `stagedctl session start` or with a
project session's `start_repo_session` tool: both are in the starter's
chain. A session may add a repo to its own project and to the
projects it created. A connected app approved for one project can add repos
to that project and cannot create projects.

Moving and copying a note (`stagedctl note move`, `stagedctl note copy`)
fit the invariant: they are "Move to project" and "Copy to project" in a
note's menu in the app, the note shows at once where it was sent, and the
user can move the note back or delete the copy. Name the note by the
reference `stagedctl timeline show` prints, `'#note:<id>'` for a branch
note or `'#project-note:<id>'` for a project note, in quotes or without the
`#`: an unquoted `#` starts a shell comment, so the note never reaches the
command. Both verbs write to both projects, the one the note is in and the
destination, so a session can send notes between its own project and the
projects it created, and nowhere else. Know what each one leaves behind
before choosing:

- A move takes the note itself. It keeps its id and its session, and a
  project note takes its child notes along with theirs. The output lists
  each child by its reference, with the repo and branch it landed on. If
  it says the children could not be read back (`children: null` under
  `--json`), they moved all the same. A branch note moved on its own loses
  its link to the project note it was written under, and moving it back
  does not restore that link. Commits and code reviews stay on their
  branch.
- A copy is a new, standalone note with its own id: the title and body as
  they read now, with no session, no parent or child notes, and no
  suggested next steps. The original is untouched.
- A branch note lands on the destination's branch for the repo and subpath
  its own branch tracks. A destination that lacks that repo gets it first,
  the way `stagedctl project add-repo` adds one: a new branch, then a
  worktree or a clone into the remote workspace and the setup actions, in
  the background. The output names each repo added. It stays in the
  project if the transfer then fails, the error names it, and only the
  user can remove it.

The app refuses a transfer it cannot make whole, with exit 1 and its own
reason: a note whose session is still writing it, a move into the project
the note is already in, a note whose branch tracks no repo, a project note
with child notes when either project is remote. Wait for the session, or
pick another destination. Do not work around a refusal.

Exit 4 (`scope_denied`) from either verb is about the credential's reach,
with one trap. A reference that names no note of its kind also exits 4
under a session's credential, with a message that says "not found": an id
that does not exist, or a project note's id written as `#note:<id>` or the
other way round. Check the reference against `stagedctl timeline show`
before reading that as a missing permission. The other reach refusals are
a note in a project the session does not reach, and a note whose body
embeds an image or video from such a project. A moved project note is
refused for media a child note embeds too, since the child travels with
it. Embedded media goes wherever the note goes, so it has to be within
reach as well.

Tag assignments (`stagedctl project tag`, `stagedctl project untag`) fit the
invariant: they show at once as the pills and chips in the project header,
where the user can undo them with a group pill's None or a chip's ×. A tag
name the catalog does not have exits 1 rather than growing the catalog;
pass `--create` only when a new ungrouped tag is genuinely what the task
calls for, since the tag then exists for every project until the user
deletes it in Settings ▸ Tags. Each tag in one call is its own write: if a
later one fails, or the re-read after them fails, the steps that landed
stay applied and are reported (`--json`: `{ ok: false, applied: [...],
error }`, where each `applied` row carries only the request and the app's
raw result). Run `stagedctl project tags` for the project's final state.

Catalog edits (`stagedctl tag create|edit|move`, `stagedctl tag group …`)
show at once in Settings ▸ Tags, where the user can rename, move or delete
what you changed, and they need a credential that reaches every project
(a session's own credential is refused, exit 4). They change what every
project's tags mean, so make them only when the task asks for a catalog
change, not to tidy up. `tag move --yes` is the one edit that drops
assignments (projects already holding a tag in the destination group lose
the moving tag); the command refuses without `--yes` when that would
happen, and the user can re-tag those projects. Deleting tags and groups
is deliberately not available.

## Showing a project in the app

`stagedctl project open` brings a Staged window to the front and switches
it to a project. It is the one verb that moves the user's window, and the
one verb that starts Staged when it is not running, so it is for clients
the user runs themselves: a terminal, a script, an agent they started.

**Not from a spawned session.** The app refuses a session's credential
(exit 4, `scope_denied`), for the session's own project too, so an agent
cannot pull the user's window away. Another project's name, or its
workspace as the current directory, reads `not_found` (exit 1) instead,
because a session's credential lists only its own project. Neither is a
fault to retry or work around, no other credential changes it, and a
session never starts an app: with `$STAGEDCTL_LOCK` set, nothing is
launched. If the user should look at a project, name it in your output
and let them open it.

Outside a session:

- `stagedctl project open <project>` takes a project name or id, as
  printed by `stagedctl project list`. A name that matches nothing
  (`not_found`), or several projects when case is ignored (`ambiguous`),
  exits 1 before any window is touched.
- `stagedctl project open` with no argument opens the project whose
  workspace the current directory is in: any directory under
  `~/.staged/workspaces/<local|remote>/projects/<project-id>/`, where
  Staged keeps a project's worktrees. From any other directory it exits 1
  (`not_in_workspace`) without looking for the app, and never starts it.
  A workspace whose project was deleted exits 1 (`not_found`), as does
  one whose project a grant approved for another project does not reach.
- `--no-launch` exits 3 instead of starting Staged when it is not
  running.
- `staged [<project> | <directory>] [--no-launch]` is the same command,
  where the `staged` launcher script is installed. It hands its arguments
  to `stagedctl project open` unchanged, except that a single argument
  written as a path (`.`, `..`, or anything with a slash, such as
  `staged ./repo`) that is an existing directory runs the command from
  that directory with no project. A bare word is always a project name,
  even beside a directory of that name. It exits 127, starting nothing,
  when it cannot find `stagedctl`.

On success it prints `opened <name>  id:<id>  pid:<pid>`, the pid being
the Staged instance that showed the project (`--json`: `{ outcome,
project: { id, name }, window, pid }`). A project that is already on
screen is a success too.

The window raised is the one the user was last in: the window that holds
focus, or else the one that held it most recently and is still open. It
is raised whether or not it could switch, so the user sees what is in the
way. When it could not switch, the command exits 1:

- `ui_busy`: a dialog is open in the window, or the diff comment being
  written could not be saved. Close or finish it and run the command
  again. Also when another `project open` reached the same window and
  replaced this one first.
- `ui_timeout`: the window did not answer in time, so the project it
  shows may not have changed.
- `not_found`: the project was deleted meanwhile.

**Credentials.** The caller needs a connected-app grant with write access
that reaches the project. On a machine that has none, run `stagedctl
connect --client <name>` once while Staged is running and approve the
consent dialog; until then the command exits 4 (`authorization_required`).
A read-only grant (`connect --read-only`) is refused (exit 4,
`scope_denied`), because showing a project marks it read and records it
as the last one shown. A grant approved for one project (`connect
--project`) opens that project only; any other reads `not_found`, because
that grant does not list it.

**When Staged is not running**, the command starts it, waits up to thirty
seconds for it to come up, and then asks as above; the app holds the
request until its window has loaded. A window that is still loading is
not shown early: it comes to the front once it is ready, which can be
after the command has exited. What it starts is the `Staged.app`
this `stagedctl` is bundled in and nothing else, on macOS only. A line on
stderr says so; stdout is unchanged. A Staged that is running but too
busy to answer is treated the same way: opening the app only brings it
forward, and the command uses it as soon as it answers. A project name is
looked up only once the app is up, so a name that matches nothing still
exits 1 (`not_found`) after Staged has started.

Staged is never started:

- with `--no-launch` (exit 3);
- when `--lock-path` or `$STAGEDCTL_LOCK` names a control file: that
  points at one instance, and when it is gone the command exits 3;
- when this machine holds no unexpired connected-app grant (exit 4,
  `authorization_required`). The consent dialog is not raised on the
  caller's behalf: start Staged and run `stagedctl connect` first;
- from a development build, whose `stagedctl` is not inside a
  `Staged.app` and whose app needs its dev server (exit 3; run
  `just dev`);
- beside a Staged that is running but cannot be used, such as one that
  was updated underneath this CLI (exit 3);
- for a directory outside every project workspace (exit 1,
  `not_in_workspace`).

`stagedctl project open --help` is the authoritative reference for all of
the above.

## When stagedctl is unavailable

The CLI needs the spawning app instance to be alive and reachable over
its control socket. Exit code 3 means "not running under a Staged app" (or a protocol
mismatch after the app updated); that is not retryable from inside this
session — report it instead of retrying. Exit code 4 means the app refused
the credential (none, revoked, expired, a grant file that no longer matches
the app's record, or the user denied a `connect` request) or the command
addressed something outside its scope; that is a deliberate boundary, not a
transient fault — report it, do not retry or work around it. The one
exception is `authorization_required` outside a spawned session, which
simply means `stagedctl connect` has not been approved yet; a mismatched
grant file (`authorization_proof_mismatch`) is cleared by the user running
`stagedctl disconnect` then `stagedctl connect` from their own terminal.
