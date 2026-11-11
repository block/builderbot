# stagedctl: driving Staged from an agent session

`stagedctl` is the control CLI for the Staged desktop app. Sessions spawned
by Staged receive everything needed to use it with zero configuration:

- `stagedctl` is on `PATH` (the exact binary path is also in `$STAGEDCTL_BIN`);
- `$STAGEDCTL_LOCK` points at the control file of the app instance that
  spawned this session (under `~/.staged/control/<pid>/`), so the CLI
  always talks to the right instance even when several are running.

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

## When stagedctl is unavailable

The CLI needs the spawning app instance to be alive and reachable over
its control socket. Exit code 3 means "not running under a Staged app" (or a protocol
mismatch after the app updated); that is not retryable from inside this
session — report it instead of retrying.
