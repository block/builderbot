# Board performance harness

Measures how the project grid and the kanban board cope with many projects, in two halves:

- **Backend** (`src-tauri/src/board_load_bench.rs`): an `#[ignore]`d Rust test. It seeds a
  throwaway SQLite database per dataset size through the real `Store`, times the reads behind the
  board, and writes what the commands return to `fixture-<n>.json`.
- **Frontend** (`run.mjs`, `mock-tauri.js`): serves a production build to a headless browser.
  `mock-tauri.js` stands in for the Tauri IPC bridge and answers every `invoke` from a fixture, so
  the page has no backend. `attribute.py` breaks a CPU profile from `run.mjs --profile` down by
  app-level work.

Nothing here starts Staged or touches its data under `~/.staged`. The Rust test refuses to run
unless `STAGED_BOARD_BENCH_DIR` is set, and refuses a directory that leads into `~/.staged`,
whether it is spelled that way, climbs in through `..`, or gets there through a symlink. Replies in
the browser carry no backend latency, so the frontend numbers are the frontend's own main-thread
work: a floor for the real app, not an estimate of it.

## Prerequisites

`playwright-core` and a Playwright browser build. Neither is a dependency of this repo. Point
`PLAYWRIGHT_CORE` at any installed `playwright-core` package directory and `BROWSER_PATH` at the
browser executable. The baseline used `playwright-core` 1.62.1 and `chromium_headless_shell-1254`.

## Re-measure

From the repository root:

```sh
eval "$(./bin/hermit env)"
OUT=/tmp/kanban-perf            # any throwaway directory outside ~/.staged
mkdir -p $OUT/data $OUT/logs

# 1. Backend timings and fixtures (about 2 minutes to build, 8 seconds to run).
#    The Tauri crate needs the stagedctl sidecar staged before it compiles.
(cd apps/staged && bash scripts/prepare-stagedctl-sidecar.sh)
(cd apps/staged/src-tauri && STAGED_BOARD_BENCH_DIR=$OUT/data \
  cargo test --lib board_load_bench -- --ignored --nocapture)

# 2. Production frontend build of the tree under test.
(cd apps/staged && node node_modules/vite/bin/vite.js build --outDir $OUT/dist --emptyOutDir)

# 3. Browser run: load and update costs, all sizes and views.
export PLAYWRIGHT_CORE=/path/to/node_modules/playwright-core
export BROWSER_PATH="$HOME/Library/Caches/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-mac-arm64/chrome-headless-shell"
uptime
node apps/staged/scripts/board-perf/run.mjs --dist $OUT/dist --fixtures $OUT/data \
  --sizes 50,200,1000 --views home-grid,home-board,pinned-board --rounds 3 \
  --out $OUT/logs/result.json
```

Step 3 prints the Load and Updates tables as markdown and writes every round to the JSON file.
Step 1 only needs repeating when the backend or the data shape changes. Steps 2 and 3 are the
before and after comparison for a frontend change.

The views are `home-grid` (the home page grid, the comparison point), `home-board` (the home page
in board mode, grouped by the "Status" tag group) and `pinned-board` (a project opened from the
grid, then the pinned "Status board" opened from the sidebar).

`STAGED_BOARD_BENCH_SIZES` (default `50,200,1000`) and `STAGED_BOARD_BENCH_ROUNDS` (default 7) set
the project counts and timed rounds of step 1.

### Attribution

Build with `--minify false`, add `--profile $OUT/profiles` to step 3, then:

```sh
python3 apps/staged/scripts/board-perf/attribute.py $OUT/profiles/1000-home-board-load.cpuprofile
```

Sampling slows the page down, so take timings from a run without `--profile`.

### WebKit cross-check

Add `--browser webkit` and point `BROWSER_PATH` at Playwright's WebKit launcher, for example
`~/Library/Caches/ms-playwright/webkit-2359/pw_run.sh`. The main-thread columns are Chromium only.

## Things to know

- **It does not depend on the call pattern.** Load is finished when every card is in the document,
  every project has been handed its branch list by whichever command the app uses, no reply is
  pending, and no frame has been blocked for 500 ms. Updates finish on the last two conditions.
- **The mock answers both ways the app reads branches and repos.** The app hydrates many projects
  with `list_branches_by_project` and `list_project_repos_by_project`, each returning one map of
  project id to list, and reads a single project with `list_branches_for_project` and
  `list_project_repos`. The fixture holds the maps the two bulk commands return, and the benchmark
  checks them against the per-project lists before writing it. The "per-project calls" column
  counts only the per-project pair, so it shows how much of a load or update still goes one
  project at a time.
- **Check `misses` and `errors` in the progress output.** A command the mock does not know returns
  `null` and is listed under `misses`. Page errors are counted under `errors`. Both should be empty.
- **Numbers move with machine load.** The 1000-project home-board load took 11.2 s at a load
  average near 12 and 15.8 s near 40. Compare before and after in the same sitting, and read
  `uptime` first.
