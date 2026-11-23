# Note diffs

When a note has been revised, the note viewer shows what the last revision
changed. A thin bar at the right of the note carries one marker per change:
green for added, red for removed, orange for modified. Hovering a marker
highlights that change in the note. Pressing one opens the previous version
in a pane on the left, with the change lit in both panes and the two
scrolling together.

## What is compared

A note's title and body are overwritten in place, so the text a revision
replaces is kept in `note_versions` (migration `0051`), by the three writes
that replace a note's text: `update_note_title_and_content`,
`update_project_note_title_and_content` and `update_written_note`
(`src-tauri/src/store/note_versions.rs`).

- A version is kept only when the outgoing body is not empty and the title or
  body changed. A note keeps its newest 10.
- The viewer compares the note with its **newest** version only, read through
  `get_note_previous_version`. There is no version picker.
- Nothing was backfilled. A note written before the migration has no versions,
  and so no bar, until it is next revised.
- Moving a note to another project is not a revision and keeps its versions,
  but it does rewrite the note: references to media the move copied point at
  the copies, and a branch note's title can gain a suffix to stay unique
  where it lands. The move rewrites the note's versions the same way, in its
  transaction (`carry_move_into_note_versions`), so the bar after a move shows
  what the last revision changed and nothing the move did. A version's
  references to media the note no longer embeds are left as written. A copy
  is a new note and starts with no versions.
- The diff is over the rendered note's blocks (a paragraph, a list item, a
  table row, a diagram), not the markdown source: `noteDiffUnits.ts` reads the
  blocks, `noteDiff.ts` compares them, and `NoteChangeBar.svelte` and
  `NotePreviousPane.svelte` show the result. All of it is in
  `src/lib/features/notes/`.

## Trying it in a scratch data directory

A development build opens `~/.staged`, the directory the installed app uses,
and applies any store migration its branch adds to the database there. Builds
without that migration then refuse the database as too new. When a branch
changes the store, as note diffs did, run it on a scratch data directory
instead. `STAGED_DATA_DIR` moves the directory (see the README's _Data
directory_ section), and the seeder below fills one with a note to look at.

### 1. Seed a note that has been revised

```sh
ROOT=$(mktemp -d /tmp/staged-note-diffs.XXXXXX)

cd apps/staged/src-tauri
STAGED_NOTE_DIFF_SEED_DIR="$ROOT/home/.staged" \
  cargo test --lib note_versions_seed -- --ignored --nocapture
```

The seeder (`src-tauri/src/note_versions_seed.rs`) is an `#[ignore]`d test, so
it runs only when asked for like this. It creates the directory and its
`data.db`, and writes through the code the app runs:

- a project named **Note diffs demo**, with no repository. The app lists and
  opens such a project like any other, with its notes above the place its
  branches would be, so nothing else has to be registered for the note to be
  reachable;
- one project note, **Upload retries: findings and plan**, written and then
  revised three times, which leaves three versions.

The last revision is the one the bar shows. Down the note, it holds:

| Marker | Change                             |
| ------ | ---------------------------------- |
| Orange | A reworded sentence in the summary |
| Orange | An edited diagram                  |
| Red    | A removed paragraph                |
| Orange | A changed list item                |
| Orange | A changed table row                |
| Green  | An added section, _Rollout_        |

The seeder refuses a path that is not absolute and one that leads into
`~/.staged`, through `..` or a symlink as well. It also refuses a directory
that already holds the demo project: seed a fresh one. Run it with no Staged
open on that directory, since the app is not told of rows written behind its
back.

### 2. Start the app on that directory

The short way moves the data directory and nothing else:

```sh
cd apps/staged
STAGED_DATA_DIR="$ROOT/home/.staged" just dev
```

WebKit storage, caches and the log file are derived from the home directory
and the bundle identifier, so this instance shares them with every other
development build. For an instance that shares nothing, give it a bundle
identifier and a home directory of its own:

```sh
mkdir -p "$ROOT/tmp" "$ROOT/acp-empty"

# A changed TAURI_CONFIG rebuilds the Rust crate. Point CARGO_TARGET_DIR at a
# copy of the shared target (cp -Rc) first, so other worktrees keep theirs.
# 5199 is an example: the port in devUrl and Vite's must be the same free one.
cd apps/staged
just stage-sidecar
(cd src-tauri && TAURI_CONFIG='{"identifier":"xyz.block.staged.notediffs","build":{"devUrl":"http://localhost:5199"}}' \
  cargo build)
./node_modules/.bin/vite --port 5199 --strictPort &

env -i PATH=/usr/bin:/bin SHELL=/bin/zsh \
  HOME="$ROOT/home" CFFIXED_USER_HOME="$ROOT/home" TMPDIR="$ROOT/tmp" \
  STAGED_DATA_DIR="$ROOT/home/.staged" \
  STAGED_ACP_TOOLS_DIR="$ROOT/acp-empty" \
  "$CARGO_TARGET_DIR/debug/Staged"
```

- `HOME` and `CFFIXED_USER_HOME` move what the system derives from the home
  directory: WebKit storage, caches and the log.
- `STAGED_ACP_TOOLS_DIR`, set to an empty directory, stops the app installing
  its managed agent bridges into the scratch directory.
- Agents cannot sign in under a scratch home directory. None is needed to
  read a note.

To check that the instance stays out of the real data directory, look at what
the process has open, not at file times, which the installed app changes all
the time: `lsof -p <pid> | grep -F .staged` should list only paths under
`$ROOT`.

### 3. Look at the note

Open **Note diffs demo** from the projects grid and press the note. The bar
at the right of the note should carry the six markers above, in that order.
Hover each one, press an orange one to open the previous version, scroll
either pane, and press Escape twice: once for the previous version, once for
the note.

What to expect:

- The markers are placed as a scrollbar's thumb is, at the same share of the
  bar as the change is of the note's scrolled height. In a note that scrolls,
  a marker therefore stands above its block, further above the lower the
  block is; only in a note that fits do the two sit side by side.
- Hovering tints the changed blocks and paints the changed words. A changed
  diagram is tinted and edged in its marker's colour. The removed paragraph
  has no block in the note: its marker draws a red line across the gap it
  left, above _Findings_.
- A press puts the change a third of the way down both panes, level in each.
  A change too near the top of the note to reach that line stays where it
  is, still level. For the removed paragraph the previous version shows it
  lit and the note shows the red line beside it; for the added section it is
  the other way round, with a green line on the left.

With the find field open (Cmd+F) and focused, Escape closes the search and
nothing else; the previous version stays as it was.

To make a version by hand instead, revise a note of your own. An agent
rewriting its note keeps the text it replaced, and so does editing and saving
a written note on a branch.
