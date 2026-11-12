# stagedctl: driving Staged from an agent session

`stagedctl` is the control CLI for the Staged desktop app. Sessions spawned
by Staged receive everything needed to use it with zero configuration:

- `stagedctl` is on `PATH` (the exact binary path is also in `$STAGEDCTL_BIN`);
- `$STAGEDCTL_LOCK` points at this session's credential file (under
  `~/.staged/control/<pid>/sessions/`), so the CLI always talks to the
  instance that spawned the session, even when several are running, and
  authenticates as this session. The credential is confined to the
  session's own project and is revoked when the session ends.

Outside a Staged-spawned session (a terminal, a script, an agent the user
started themselves) there is no `$STAGEDCTL_LOCK`. Run
`stagedctl connect --client <name>` once: the Staged user sees a consent
dialog and, on approval, a grant is stored on this machine
(`~/.staged/control/grants/`, owner-private) that later invocations use
without asking again. It lasts thirty days from its last use; the user can
revoke it any time under Settings ▸ Connected apps, and
`stagedctl disconnect` forgets and revokes it from this side.

## The boundary for spawned sessions

A spawned session is issued exactly one credential: its own, scoped to its
project. That is the credential to use, as is. Do not try to obtain wider
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
be caught. If a task genuinely needs access beyond this session's project,
say so in your output and let the user run `stagedctl connect` from their
own terminal.

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
