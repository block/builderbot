# Load-test harness

Drives a real Staged backend and frontend with fake agents and measures both, so the same load can
be re-run on any branch and two runs compared. No model, no network, no credentials.

```sh
# from apps/staged, in the repo's hermit environment
just perf-harness --label before --projects 4 --sessions-per-project 6 --duration 60
just perf-harness --label after  --projects 4 --sessions-per-project 6 --duration 60 \
    --app-dir <other checkout>/apps/staged
just perf-harness compare <results>/before-<stamp>.json <results>/after-<stamp>.json
```

`just perf-harness` is `node scripts/perf-harness/run.mjs`; `--help` lists every parameter. A run
prints a summary and writes `<label>-<stamp>.json` (every figure) and `<label>-<stamp>.md` (the
summary) to `--out`, by default `$TMPDIR/staged-perf-harness/results`. `compare` prints the headline
figures of two result files side by side with the difference; `--all` prints every numeric figure.
There is no stored baseline: measure the two things you want to compare, on the same machine, close
together.

A smoke run is `--projects 2 --sessions-per-project 2 --duration 8 --warmup 3 --scenarios
subtitles,both`: about a minute once built.

Run it with the hermit environment's Node (`bin/node` at the repository root), not a Node under
`~/.staged`: `run.mjs` refuses the latter.

### When a run fails

The driver writes its result after the app starts, after every scenario, and when it fails, so a run
that fails or is interrupted still writes both files, with every scenario measured up to then. Such
a result has `complete: false`, a `failure` (the phase, the message and, for sessions that never
started, each one's state), the scenarios `notRun`, and a summary that says so at the top. `compare`
warns when either of its files is incomplete.

| exit | meaning                                                             |
| ---- | ------------------------------------------------------------------- |
| 0    | every scenario measured, every check passed                         |
| 1    | the run failed; the result holds what was measured before           |
| 2    | bad parameters, or the build failed (no result)                     |
| 3    | measured, and a safety check or a shown-against-stored check failed |
| 130  | interrupted; the result holds what was measured before              |

A run stops by itself, with a partial result, when:

- a session is not streaming `--start-timeout` seconds (240) after it was started. The failure lists
  each such session with its status, its message count, whether the app wrote its first message, and
  whether its agent got a turn, so a session the app never started is told from an agent that never
  wrote. Nothing is measured with a session missing.
- the machine cannot carry the load: over a window, the agents wrote less than `--min-rate-share`
  (0.5) of the rate asked for, or the once-a-second process sampler got less than that share of its
  samples. A machine that far behind measures itself, not the app. `0` turns the rule off.
- the app or the browser exits, or a DevTools or app request gets no answer (two minutes for a
  DevTools call or a command, thirty seconds for a harness route).

## What a run does

1. Builds the tree: `cargo build --features perf-harness` and a production `vite build`. Both are
   copied to a build cache under `$TMPDIR/staged-perf-harness/build`, so `--skip-build` can reuse
   them. The cargo build uses a target directory of the harness's own
   (`$TMPDIR/staged-perf-harness/cargo-target`, or `--target-dir`), whatever `CARGO_TARGET_DIR` is
   set to, and refuses the one the tree's ordinary builds use: that one may be shared between
   worktrees, and building there with another feature set and dev URL replaces the binary a running
   dev app was started from. The first build of a tree is therefore a full one. The harness's
   directory is shared by every tree it builds, so a lock in it (`perf-harness-build.lock`) is held
   from before `cargo build` until the binary has been copied to the tree's cache: a build of
   another tree at the same time waits, and each run stages the binary it built. A lock left by a
   build that was killed is taken over, and the next build to take the lock removes what a build
   killed in the middle of taking it left beside it. The wait is at most `--build-wait` seconds (3600; 0 fails
   at once): after that the run exits 2 with the holder's process id, its command line and when it
   took the lock. That is what ends a wait on a lock nobody will release, which is what a killed
   build's lock becomes once its process id has been given to another process; the message says
   which directory to remove. An interrupt ends the wait like any other part of a run. The binary
   has its port compiled in, and the cache keeps one binary per profile with a marker per port;
   replacing the binary removes every marker of that profile, so `--skip-build` works only for the
   port the cached binary was built for.
2. Copies the binary, the frontend, a Node and the harness itself into a throwaway run directory,
   and starts the driver there under a throwaway `HOME` and a minimal `PATH`.
3. The driver starts the app, seeds projects (local git repositories, one branch per session) through
   the app's own commands, and opens the frontend.
4. For each scenario it starts the sessions and waits until every one is streaming, brings the page
   to the view, waits `--warmup`, measures for `--duration`, checks what the page shows against the
   store, and cancels the sessions.
5. Ends every process of the run, writes the result, deletes the run directory (`--keep` keeps it).

### The load

Each session's agent is `agent/fake-agent.cjs`, which the app runs as `claude-agent-acp` through
`STAGED_ACP_TOOLS_DIR`. It streams text at a set rate, one line per chunk, with tool calls that
stay in flight while their output grows, occasional large tool bodies, and plan and title updates.
While a tool call is in flight the agent writes no text, so with the defaults a session writes
25 chunks every 8 s (6 s of text at 4 a second, then a 2 s tool call): 3.1 a second, not 4.
`load.expectedChunksPerSecond` is that figure for the whole run and `load.rateShare` what the
agents wrote against it.

| parameter                                  | default    |                                                               |
| ------------------------------------------ | ---------- | ------------------------------------------------------------- |
| `--projects`                               | 3          | projects running at once: the viewed one and the rest (≥ 2)   |
| `--sessions-per-project`                   | 4          | sessions per project, one branch each                         |
| `--running`                                | 0          | how many stream at once, spread across the projects (0 = all) |
| `--rate`                                   | 4          | text chunks per second from each background session           |
| `--chat-rate`                              | 20         | text chunks per second from the session whose chat is open    |
| `--chunk-chars`                            | 80         | characters per chunk                                          |
| `--paragraph-every`                        | 8          | a new paragraph every N chunks                                |
| `--tool-every`                             | 25         | a tool call every N chunks                                    |
| `--tool-ms`, `--tool-bytes`                | 2000, 6000 | how long a tool call is in flight, and its body size          |
| `--tool-step-ms`                           | 500        | how often an in-flight tool call's output grows               |
| `--large-tool-every`, `--large-tool-bytes` | 4, 200000  | every Nth tool call has a large body                          |
| `--plan-every`                             | 40         | a plan and a title update every N chunks                      |
| `--prompt-bytes`                           | 0          | pad each opening prompt to this size (0 = two words)          |
| `--duration`, `--warmup`                   | 30, 5      | seconds measured per scenario, and settled before it          |

### The scenarios (`--scenarios`, default all four)

Every scenario views a project of its own, seeded for it and not used by any other scenario, so no
scenario starts on rows and transcripts an earlier one left. The other `--projects - 1` projects are
shared, and their sessions are load the page is not looking at. A run therefore seeds one project
per scenario plus the background ones, and `--projects 3` means three projects with running
sessions in every scenario.

- `subtitles`: the project's detail with all its sessions running, no chat open.
- `chat`: an open chat on a fast-streaming session, the only one running in its project.
- `both`: the open chat over the project detail with all its sessions running.
- `hidden`: `both`, with the window hidden while it is measured. Reports how long the chat took to
  show current text again once shown.

### How much of the project the page shows

The app mounts a branch card's timeline only while the card is near the visible part of the scroll
area (within one and a half viewports), and unmounts it when it leaves. The driver finds a session's
row through its branch card, scrolling the card into view first, so a project of any size can be
opened and its chat reached. What is _measured_ is the rows that stay mounted: with the default
1000 px window that is about twelve sessions. `load.rows` says how many of the viewed project's
sessions had a row on the page at both ends of the window (`mounted`) and which did not
(`notMounted`); the summary prints both counts and a warning when they differ.

`--viewport-height` (browser frontend only) sets the window's height. About 100 px per session
mounts them all: 2500 for a project of 25. That measures a page rendering every row, which is more
than a person's window does, so say which one a figure is from.

### Comparing two configurations of one build

`--local-storage key=value` (repeatable) is set in the page's `localStorage` before the app boots,
so a frontend switch can be measured both ways from one build: run once without it and once with
it, the second with `--skip-build`. The harness knows no key by name, and the result records what
was set (`config.localStorage`). The tree has no such switch at the moment: the one this was first
used for, `staged:session-stream`, went away with the polling path, and setting a key the page does
not read changes nothing.

### Comparing two trees

`--app-dir <other checkout>/apps/staged` builds and measures another tree with this harness. That
tree needs the `perf-harness` cargo feature: `src-tauri/src/perf_harness.rs`, the directory beside
it, and the touch points listed at the top of that file. On a tree without the session
stream, leave out `perf_harness/stream_frames.rs` and its call in `session_stream/hub.rs`; the
driver asks the app whether it has the stream, and reports the stream figures as absent when it
does not. The page selectors are all in `driver/ui.mjs` if a tree's markup differs.

`compare` warns that two runs are not like for like when their parameters differ, and names the
ones that do. Parameters that only say how a run was named, built or served are left out of that:
`--label`, `--out`, `--keep`, `--skip-build`, `--app-dir`, `--target-dir`, `--build-wait` and
`--port`. So the before/after recipe at the top compares without a warning, and a warning means the
load, the window, the frontend or the profile really differed. `--local-storage` is shown on each
side instead.

### Which frontend

- `--frontend browser` (default): headless Chromium loading the production frontend from the app's
  loopback server, so the web transport (`fetch` and one WebSocket). It needs a Chromium: `--browser`,
  `$BROWSER_PATH`, a Playwright `chromium_headless_shell` in `~/Library/Caches/ms-playwright`, or
  Google Chrome. No npm package is needed; the driver speaks the DevTools protocol itself.
- `--frontend desktop`: the app's own WKWebView over Tauri IPC and channels, driven through the
  harness's eval route. The window is never put on screen (the harness build cannot take focus), so
  WebKit throttles its timers and frames: the latency figures are real, the frame and timer figures
  are not, and there is no long-task or per-frame data. Use it to check the desktop transport, and
  `browser` for main-thread figures. Anything the page does on a timer runs as late as the timers
  do; the summary warns when the median timer lag is over 100 ms. The page no longer polls for
  session data (the stream pushes it), so this does not change how many commands a run makes, which
  it did while the page polled.

## What is measured

All figures are per scenario, over the measured window only. A figure that could not be taken is
`null` in the JSON and `absent` in the tables, never zero.

Backend (`backend.*`):

- `process`: the app process's CPU seconds, CPU % of one core and resident memory, sampled once a
  second with `ps`. The CPU is what was used between the first and the last sample of the window,
  and the percentage is of the time between those two (`spanSeconds`), which can be up to a second
  shorter or longer than the window. `childProcesses` is what it started (the fake agents, git),
  added up process by process: each adds what it used since the sample before, so one that exits
  inside the window keeps what it had used. A process that starts and exits between two samples
  (most of git's) is never seen, and neither is what a process uses after its last sample.
- `commands`: every command served over the web API by name, with count, errors, response body
  bytes on the wire (after compression) and time. Browser frontend only; the driver's own calls are
  excluded.
- `streamFrames`: session stream frames a client's sink took, by level (`summary`, `full`) and
  kind, with their size as JSON before the transport. Counted at the hub once the sink has
  answered, so it covers both transports. `dropped` counts the frames the hub sent that no sink
  took (a browser client's queue was full, or the client had gone): those reached nobody and are
  in neither `frames` nor `bytes`, so the backend's total and the page's still agree when a queue
  overflows. `streamTotals.dropped` is their sum, and `null` from hooks before version 3.
- `storeLock`: how long each taker waited for the store's one connection lock, and how long it was
  held. Every database access in the app is one take.
- `agentWriteToStoredMs`: from the fake agent writing a piece of output to its row being written.
  `text` includes the wait for the message writer, which writes a session's text at most once every
  150 ms, by design. `tool` is a tool call's
  row and the growing output of one in flight, both written at once. This is the figure that shows
  writes backing up behind the lock. A piece is counted once per write however often its row
  holds its stamp (a Bash call's row holds it twice, in the command and in the description).

Frontend (`frontend.*`):

- `commands`: calls the page made, by name, with count and bytes (decoded and on the wire). From the
  DevTools network events in the browser; from a `fetch` wrapper on Tauri's IPC in the desktop.
- `events`, `streamFrames`: WebSocket frames received by event name, and stream frames by level and
  kind (browser only).
- `received`: everything the page received, calls and socket together, counted one way: `bytes`
  decoded and `wireBytes` as transferred. Use this pair to compare a tree that fetches with one that
  pushes. The wire figure of a call includes its response headers and reflects compression; a socket
  frame's is its payload, and is `null` if the socket negotiated compression (`socketCompressed`),
  which the app's server does not. The backend's `commands` bytes are bodies after compression and
  its `streamFrames` bytes are JSON before the transport, so those two are not the same kind of
  figure and should not be added.
- `longTasks`: main-thread tasks over 50 ms (Chromium only).
- `frameGapsMs`: time between animation frames, as percentiles, with the count over 50 ms.
- `timerLagMs`: how late a 50 ms timer fired, the main thread's delay as any engine shows it.
- `agentWriteToDomMs`: from the fake agent writing a piece of output to its text appearing in the
  DOM, for the `subtitle` of a timeline row and for the open `chat`, text and tool calls apart.
  `agentWriteToFrameMs` is the same to the next animation frame. Each piece is counted once, the
  first time it shows; a subtitle shows only the newest line, so pieces it skipped are not counted.
  For a subtitle this is therefore the latency _of the updates that were shown_: a row that updates
  once every few seconds, each time promptly, has a low figure here. Read it with its `count` and
  with the next figure.
- `subtitleStaleness`: how out of date the rows were. Twice a second the driver reads which piece
  each mounted row of the viewed project shows and asks the app for the first piece of that session
  it stored _after_ that one which a subtitle would show (text or a tool call). `outdatedMs` is how
  long before the reading that piece was stored: for how long the store had held something newer
  than the row showed. It is 0 for a row showing the newest piece. One sample is one row at one
  reading, so the percentiles are over rows and time alike, and a row that stops updating raises
  them for as long as it is stuck. `piecesBehind` is the same in pieces: how many pieces a subtitle
  would show (text and tool calls) the store held at the reading that are newer than the one
  shown, counted by the app. It is not a difference of sequence numbers, which would add a tool
  call's output steps, and those never show in a subtitle. `upToDate` and `over1s` are the
  share of samples at 0 and above a second, `nothingShown` samples of a row with no stamp in it yet,
  and `floors` samples where the row was further behind than the 256 pieces per session the app
  keeps (then both figures are lower bounds). `null` when the app's hooks cannot answer
  (`piecesBehind` alone when they are version 2).
  In `hidden` the page is not updated while it is hidden, so the figure grows with the window: there
  it says how far behind a hidden page gets, and `catchUpAfterShownMs` how fast it recovers.
- `renderer`: the renderer's own task, script, layout and style seconds, and `browserProcess`, the
  CPU and memory of the whole browser (browser only).

The agent stamps every piece of output with `pfm-<kind><tag>-<seq>-<epoch ms>`; the app's harness
build reads the stamp when the row is stored and the page probe reads it in the DOM. Agent, app and
page share one clock, so the latencies need no synchronization. The kinds are `x` (a chunk of
text), `t` (a tool call) and `o` (the output of a tool call in flight). A session's numbers run in
one sequence across the kinds, and a row's subtitle shows the newest `x` or `t`, never an `o`.

`load` records what the agents really wrote in the window against what was asked for (`rateShare`),
how many of the viewed project's rows were mounted (`rows`), and the size of the sessions' rows in
the store (`sessionRowBytes`, which is mostly the stored prompt: the app wraps even a two-word
prompt in about 7 kB of its own, and `--prompt-bytes` adds to it). `host` is the machine's load
average at both ends of the scenario's window, and the result's top level has it for the whole run.
Read those, and the summary's "Read before trusting the figures", first.

### Shown against stored

After each window, with the sessions still streaming, the driver checks that the page shows what
the store holds (`checks` in each scenario; `--skip-verify` leaves them out):

- `subtitles`: three rounds a second apart. Each reads the newest showable piece the store holds for
  every mounted row (`get_session_messages`), then waits up to 8 s for each row to show that piece
  or a later one, and checks the store holds whatever the row shows. A row that never catches up, or
  shows a piece the store does not have, fails the check (`late`, `unknown`). `caughtUpMsMax` is the
  slowest row of the three rounds.
- `chat` (scenarios with a chat open): once the sessions are stopped, the text pieces in the open
  chat against the session's stored assistant rows. Every piece shown must be stored, and every
  stored piece from the oldest one shown on must be shown, up to the newest.

A failed check is listed in the summary and makes the run exit 3. The figures are kept.

## Safety

The app keeps everything in `$HOME/.staged` and has no override, so isolation is by `HOME`:

- `run.mjs` refuses to start unless the data directory resolves (symlinks followed) inside its own
  throwaway run directory and not inside the real `~/.staged`, and unless the Node it runs on is
  outside the real `~/.staged` too.
- The app's harness build refuses to start (exit 78) unless its data directory is inside the
  directory named by `STAGED_PERF_HARNESS_ROOT`, and the driver checks the directory and process id
  the app reports before it writes anything through it.
- Everything a run executes or reads is copied into the run directory first, so nothing is opened
  in the checkout, which may itself live under `~/.staged/workspaces`.
- The environment is built from nothing: a `HOME` and `TMPDIR` in the run directory, `PATH` of the
  run's own `bin` plus `/usr/bin:/bin:/usr/sbin:/sbin`, and a stand-in `$SHELL` without login files
  (the app captures a login shell's environment). No agent tool of the user's is on that `PATH`,
  and `STAGED_ACP_TOOLS_DIR` holds only the fake agent.
- The app's harness port is on `127.0.0.1` and serves nothing, neither a command nor the event
  socket nor the frontend, to a caller without the run's token (see "The hooks in the app"). The
  token is made per run and kept in the run directory, which only its owner can read.
- While it runs, every third second `lsof` lists the open files of the app, everything it started,
  the browser and the driver. A file under the real `~/.staged`, or a program from outside the run
  directory and the system directories, is recorded in the result's `safety` section and fails the
  run (exit 3).
- The driver runs in its own process group. On exit, failure or a signal, `run.mjs` ends the group
  and then every process whose command line or working directory is in the run directory, and
  records any survivor as a failure. If `run.mjs` itself is killed outright, the driver notices
  within a second and stops the app and the browser; only the run directory is then left behind.

## The hooks in the app

Everything is in `src-tauri/src/perf_harness.rs` and `src-tauri/src/perf_harness/`, behind the
`perf-harness` cargo feature, which no dev, build or release recipe enables. A binary built with it
does nothing until `STAGED_PERF_HARNESS_PORT` is set. Then it serves, on `127.0.0.1` only and only
for that `Host` and `Origin`, the app's web API and frontend (`web_server::start` is a stub in this
repository) plus `/perf/eval`, `/perf/result(s)`, `/perf/metrics`, `/perf/metrics/reset` and
`/perf/outdated`. It adds no stored field, column or migration: what it keeps (histograms,
counters, and the last 256 showable stamps of each session with the time each was stored) is in
memory.

Every route needs the run's token: `/api/invoke/*` and `/api/events` run commands and carry session
text, and the web server in this repository puts no credential of its own on them. A request
without the token gets 401, and one for another `Host` or from another `Origin` gets 403 whatever
it sends.

- The driver sends the token as the `x-perf-token` header on every request.
- The page cannot: its calls are the app's own code, which knows of no token, and a browser sets no
  header on a WebSocket. It gets the token as a cookie (`staged_perf_token`), which the browser
  then adds by itself to the document, its assets, every `fetch` and the handshake of the event
  socket, where the app checks it. For headless Chromium the driver sets the cookie through
  DevTools (`Network.setCookie`) before the page first loads: `HttpOnly`, so no script reads it,
  the page's included; `SameSite=Strict`; for the run's origin; in a browser profile made for the
  run and deleted with it. For the desktop webview there is no such door, so the driver sets it
  with a script through `/perf/eval` (which needs the header) as a session cookie. Until then the
  webview's own request for the frontend is refused, which in a browser run leaves the browser as
  the app's only client. With either frontend, a page that is still refused once the cookie is set
  fails the run at once with "the app refused the page".
- `/perf/*` takes the header only, never the cookie.

The metrics snapshot carries `hooks`, the version of these hooks (3). A tree measured through
`--app-dir` with older hooks still runs: the driver sends the token to hooks that do not ask for
it, the result records their version, and the summary says what is missing or coarser. Version 1
has no `subtitleStaleness`; version 2 has it without `piecesBehind`, counts a stream frame before
it is sent (no `dropped`), counts a tool call whose row holds its stamp twice as two, and puts
only `/perf/*` behind the token.

## Limits

- The browser frontend is Chromium on the web transport, not WKWebView on Tauri IPC. Desktop
  commands are counted in the page only (the backend cannot see IPC without wrapping every handler),
  and WebKit's helper processes, which belong to launchd, are not in the app's CPU figure.
- `hidden` makes the page see `document.hidden` and a `visibilitychange`; the engine's own treatment
  of a hidden window (timer throttling, no frames) is not simulated.
- Percentiles from the app's histograms (store lock, write to stored) are bucket upper bounds,
  sixteen buckets per octave: never under, and over by less than 6.25 %. `max` and `mean` are exact.
- The subtitle staleness is sampled twice a second and reads the page and then the app, so one
  sample can be off by the time between the two reads (a few ms); a piece stored in between counts
  as 0.
- Sessions started together make their tool calls together, so a window shorter than a few tool
  cycles (8 s each with the defaults) can hold a little more or less than its share of chunks; the
  summary warns only below 80 %.
- A session that never starts stops the run (see "When a run fails"); the harness reports it and
  does not work around it.
- Headless Chromium rasterizes in software, so `browserProcess` CPU is dominated by painting;
  `renderer` is the figure for main-thread work.
- The debug profile is the default because it builds fastest; `--profile release` measures an
  optimized backend.
- macOS only (`ps`, `lsof`, the activation policy), and one run at a time per `--port`. Two runs
  of one tree at once are not supported even on two ports: the tree's cache holds one binary per
  profile, and the second build replaces it.
- The isolation is by environment and checked by observation; it is not a sandbox.
