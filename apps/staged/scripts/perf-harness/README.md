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
subtitles,both`: about 20 seconds once built.

## What a run does

1. Builds the tree: `cargo build --features perf-harness` and a production `vite build`. Both are
   copied to a build cache under `$TMPDIR/staged-perf-harness/build`, so `--skip-build` can reuse
   them. The cargo build goes through the tree's usual target directory with a different feature set
   and dev URL, so the next ordinary build of the app crate recompiles it.
2. Copies the binary, the frontend, a Node and the harness itself into a throwaway run directory,
   and starts the driver there under a throwaway `HOME` and a minimal `PATH`.
3. The driver starts the app, seeds projects (local git repositories, one branch per session) through
   the app's own commands, and opens the frontend.
4. For each scenario it starts the sessions, brings the page to the view, waits `--warmup`, measures
   for `--duration`, and cancels the sessions.
5. Ends every process of the run, writes the result, deletes the run directory (`--keep` keeps it).

### The load

Each session's agent is `agent/fake-agent.cjs`, which the app runs as `claude-agent-acp` through
`STAGED_ACP_TOOLS_DIR`. It streams text at a set rate, one line per chunk, with tool calls that
stay in flight while their output grows, occasional large tool bodies, and plan and title updates.

| parameter                                  | default    |                                                               |
| ------------------------------------------ | ---------- | ------------------------------------------------------------- |
| `--projects`                               | 3          | projects; sessions are spread across them (at least 2)        |
| `--sessions-per-project`                   | 4          | sessions per project, one branch each                         |
| `--running`                                | 0          | how many stream at once, spread across the projects (0 = all) |
| `--rate`                                   | 4          | text chunks per second from each background session           |
| `--chat-rate`                              | 20         | text chunks per second from the session whose chat is open    |
| `--chunk-chars`                            | 80         | characters per chunk                                          |
| `--paragraph-every`                        | 8          | a new paragraph every N chunks                                |
| `--tool-every`                             | 25         | a tool call every N chunks                                    |
| `--tool-ms`, `--tool-bytes`                | 2000, 6000 | how long a tool call is in flight, and its body size          |
| `--large-tool-every`, `--large-tool-bytes` | 4, 200000  | every Nth tool call has a large body                          |
| `--plan-every`                             | 40         | a plan and a title update every N chunks                      |
| `--duration`, `--warmup`                   | 30, 5      | seconds measured per scenario, and settled before it          |

### The scenarios (`--scenarios`, default all four)

Every scenario views the first project; the other projects' sessions are load the page is not
looking at.

- `subtitles`: the project's detail with all its sessions running, no chat open.
- `chat`: an open chat on a fast-streaming session, the only one running in its project.
- `both`: the open chat over the project detail with all its sessions running.
- `hidden`: `both`, with the window hidden while it is measured. Reports how long the chat took to
  show current text again once shown.

### Comparing two configurations of one build

`--local-storage key=value` (repeatable) is set in the page's `localStorage` before the app boots,
so a frontend switch can be measured both ways from one build: run once without it and once with
it, the second with `--skip-build`. The harness knows no key by name.

### Comparing two trees

`--app-dir <other checkout>/apps/staged` builds and measures another tree with this harness. That
tree needs the `perf-harness` cargo feature: `src-tauri/src/perf_harness.rs`, the directory beside
it, and the one-line touch points listed at the top of that file. On a tree without the session
stream, leave out `perf_harness/stream_frames.rs` and its call in `session_stream/hub.rs`; the
driver asks the app whether it has the stream, and reports the stream figures as absent when it
does not. The page selectors are all in `driver/ui.mjs` if a tree's markup differs.

### Which frontend

- `--frontend browser` (default): headless Chromium loading the production frontend from the app's
  loopback server, so the web transport (`fetch` and one WebSocket). It needs a Chromium: `--browser`,
  `$BROWSER_PATH`, a Playwright `chromium_headless_shell` in `~/Library/Caches/ms-playwright`, or
  Google Chrome. No npm package is needed; the driver speaks the DevTools protocol itself.
- `--frontend desktop`: the app's own WKWebView over Tauri IPC and channels, driven through the
  harness's eval route. The window is never put on screen (the harness build cannot take focus), so
  WebKit throttles its timers and frames: the latency figures are real, the frame and timer figures
  are not, and there is no long-task or per-frame data. Use it to check the desktop transport, and
  `browser` for main-thread figures.

## What is measured

All figures are per scenario, over the measured window only. A figure that could not be taken is
`null` in the JSON and `absent` in the tables, never zero.

Backend (`backend.*`):

- `process`: the app process's CPU seconds, CPU % of one core and resident memory, sampled once a
  second with `ps`. `childProcesses` is what it started (the fake agents, git).
- `commands`: every command served over the web API by name, with count, errors, response bytes on
  the wire (after compression) and time. Browser frontend only; the driver's own calls are excluded.
- `streamFrames`: session stream frames handed to a client, by level (`summary`, `full`) and kind,
  with their JSON size. Counted at the hub, so it covers both transports.
- `storeLock`: how long each taker waited for the store's one connection lock, and how long it was
  held. Every database access in the app is one take.
- `agentWriteToStoredMs`: from the fake agent writing a piece of output to its row being written.
  `text` includes the message writer's flush interval (150 ms) by design; `tool` rows are written at
  once. This is the figure that shows writes backing up behind the lock.

Frontend (`frontend.*`):

- `commands`: calls the page made, by name, with count and bytes (decoded and on the wire). From the
  DevTools network events in the browser; from a `fetch` wrapper on Tauri's IPC in the desktop.
- `events`, `streamFrames`: WebSocket frames received by event name, and stream frames by level and
  kind (browser only).
- `longTasks`: main-thread tasks over 50 ms (Chromium only).
- `frameGapsMs`: time between animation frames, as percentiles, with the count over 50 ms.
- `timerLagMs`: how late a 50 ms timer fired, the main thread's delay as any engine shows it.
- `agentWriteToDomMs`: from the fake agent writing a piece of output to its text appearing in the
  DOM, for the `subtitle` of a timeline row and for the open `chat`, text and tool calls apart.
  `agentWriteToFrameMs` is the same to the next animation frame. Each piece is counted once, the
  first time it shows; a subtitle shows only the newest line, so pieces it skipped are not counted.
- `renderer`: the renderer's own task, script, layout and style seconds, and `browserProcess`, the
  CPU and memory of the whole browser (browser only).

The agent stamps every piece of output with `pfm-<kind><tag>-<seq>-<epoch ms>`; the app's harness
build reads the stamp when the row is stored and the page probe reads it in the DOM. Agent, app and
page share one clock, so the latencies need no synchronization.

`load` records what the agents really wrote in the window (a loaded machine can fall short of the
requested rate), and `host` the machine's load average at both ends. Read those first.

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
repository) plus `/perf/eval`, `/perf/result(s)`, `/perf/metrics` and `/perf/metrics/reset`, which
need the run's token. It adds no stored field, column or migration.

## Limits

- The browser frontend is Chromium on the web transport, not WKWebView on Tauri IPC. Desktop
  commands are counted in the page only (the backend cannot see IPC without wrapping every handler),
  and WebKit's helper processes, which belong to launchd, are not in the app's CPU figure.
- `hidden` makes the page see `document.hidden` and a `visibilitychange`; the engine's own treatment
  of a hidden window (timer throttling, no frames) is not simulated.
- Percentiles from the app's histograms (store lock, write to stored) are bucket upper bounds, two
  buckets per octave: right to within about 40 %, never under. `max` and `mean` are exact.
- Headless Chromium rasterizes in software, so `browserProcess` CPU is dominated by painting;
  `renderer` is the figure for main-thread work.
- The debug profile is the default because it is what is already built; `--profile release` measures
  an optimized backend.
- macOS only (`ps`, `lsof`, the activation policy), and one run at a time per `--port`.
- The isolation is by environment and checked by observation; it is not a sandbox.
