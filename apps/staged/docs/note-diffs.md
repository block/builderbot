# Note diffs

When a note has been revised, the note viewer shows what the last revision
changed. Each change has a marker in the note's right margin: green for
added, red for removed, amber for modified. The colours are the code diff
viewer's (see [Colours](#colours)). A marker stands beside its
change, as tall as the changed text, and scrolls with the note. Hovering a
marker highlights that change in the note. Pressing one opens the previous
version in a pane on the left, with the change lit in both panes and the two
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
  and so no markers, until it is next revised.
- Moving a note to another project is not a revision and keeps its versions,
  but it does rewrite the note: references to media the move copied point at
  the copies, and a branch note's title can gain a suffix to stay unique
  where it lands. The move rewrites the note's versions the same way, in its
  transaction (`carry_move_into_note_versions`), so the markers after a move
  show what the last revision changed and nothing the move did. A version's
  references to media the note no longer embeds are left as written. A copy
  is a new note and starts with no versions.
- The diff is over the rendered note's blocks (a paragraph, a list item, a
  table row, a diagram), not the markdown source: `noteDiffUnits.ts` reads the
  blocks, `noteDiff.ts` compares them, and `NoteChangeBar.svelte` and
  `NotePreviousPane.svelte` show the result. All of it is in
  `src/lib/features/notes/`.

## Where the markers stand

The markers are part of what the note scrolls (`NoteDiffMarkers.svelte`,
mounted inside the scroll area), so nothing is computed as it scrolls. Each is
placed from its blocks as measured (`hunkMarkerBox` in `noteDiffScroll.ts`):

- A change with blocks in the note has a marker from the top of its first
  block to the bottom of its last. A reworded three-line paragraph has a
  marker three lines tall; an added section has one that spans the section.
- Removed text has no block in the note. Its marker is 8px tall, centred on
  the red line that is drawn across the gap the text left while the change is
  pointed at.
- The blocks are measured again whenever the note is laid out (an image
  loading, the dialog or the window resized, the previous version opening or
  closing, text wrapping again), and the markers follow.
- They stand in the note's right padding, 12px clear of the text and 8px from
  the scroll area's edge, which leaves the scrollbar and the dialog's resize
  handle free. That distance is the same whether or not the note has a
  scrollbar.

Beside the previous version there is a second set of the same markers, level
with that version's blocks. There the removed text has a marker of its full
height, and added text the short one across its green line. They take hovers
and presses as the note's do; Tab stops only at the note's, which list every
change once.

A marker that scrolls under a pointer at rest is not hovered: only a pointer
that moved onto it lights the change. Otherwise the change a press scrolled
into the pointer's place would take the light from the one that was pressed.

## Colours

A note diff is painted from the code diff viewer's own tokens
(`DiffViewer.svelte` in `packages/diff-viewer`), so the two read as one
system. A block is to a note what a line is to a file:

| In the note diff                      | Token                       | In the code diff                |
| ------------------------------------- | --------------------------- | ------------------------------- |
| An added block                        | `--diff-added-bg`           | An added line, in the new pane  |
| A removed block                       | `--diff-removed-bg`         | A removed line, in the old pane |
| A changed block, in both panes        | `--diff-modified-bg`        | A modified line, in both panes  |
| The words that changed, in both panes | `--diff-modified-inline-bg` | `.char-changed`, in both panes  |

- The viewer has a colour for a modified line: amber, the same on both sides,
  with the changed characters in a stronger amber. It is not red on the old
  side and green on the new. Red and green are for lines only one side has,
  and the note diff follows that block by block: `data-note-diff` on a block
  is its own change, not its hunk's. Where a revision replaced a paragraph
  outright, or the comparing budget below ran out, the old block is red in
  the previous version and the new one green in the note, and only the
  change's marker is amber.
- The tints above are the resting ones, on every changed block while the
  previous version is open. The change being pointed at takes a stronger tint
  of the same colour: `--diff-added-bg-emphasis`,
  `--diff-removed-bg-emphasis` and `--diff-modified-bg-emphasis`. The viewer
  has no such state, so these were added beside its tokens, the first two in
  `theme.ts` and the amber one in `app.css`. The amber emphasis stays under
  the inline colour, so changed words show on it.
- Markers, a changed figure's edge and the gap line of the change being
  pointed at are solid, in the colour the tint is made of: `--status-added`
  and `--status-deleted` (which the diff's file list also uses for a file's
  added and removed lines) and `--diff-modified-accent`, the amber at full
  strength. `--status-modified` is not used: on the light theme it is a dark
  gold meant to be read as text, and beside the amber tint it looks brown.
  The other gap lines are the emphasised tint, and a pointed marker has a
  2px halo of it.
- The amber is one colour in every theme. `app.css` sets the modified tokens
  and `theme.ts` does not, so they are the same in the light and dark chrome
  and under every diff theme.
- The green and red are the chrome's. The viewer's panes are themed apart
  from the rest of the app, by the diff theme chosen in Settings
  (`applyDiffTheme` in `preferences.svelte.ts` scopes a second set of the
  same tokens onto the diff's container), and the note viewer is chrome. With
  a diff theme that has the chrome's git colours (`github-light` on the light
  chrome, `github-dark-default` on the dark) the tokens are equal. One that
  brings its own, such as the default `laserwave`, whose added is teal and
  removed purple, shows those in its panes, while a note diff, like the
  diff's own file list, keeps the chrome's green and red.

`noteDiffColours.test.ts` holds the note diff's stylesheets to these tokens,
and the viewer's rules to the ones they are matched against, so a change to
either side fails there. `theme.test.ts` checks that text keeps its contrast
on the emphasised tints.

## Limits

### Media a revision dropped

A version keeps the note's text, not its media. Images and videos are deleted
once no note body and no chat message embeds them
(`note_media::cleanup_unreferenced`), and the text kept in `note_versions` is
not read for that. A revision that drops or replaces an image therefore takes
the file with it, and the previous version goes on naming media that is gone.

The previous version shows a placeholder in its place: a dashed box with an
icon and **Image no longer available** or **Video no longer available**, and
the caption under it when the media had one. The box is a fixed 120px tall at
least, whatever the media measured, so the previous version is shorter there
than it was when written. The panes are aligned on the blocks as measured, so
they scroll together as they do past any other block.

The placeholder is drawn when the media fails to load (`missingMedia.ts`), not
from a check of what is stored. Media the note still embeds is never given
one, so an image both versions show cannot be mislabelled while the note's own
copy is on screen.

### How much comparing a note gets

Telling which blocks were reworded, and which words in them changed, costs the
product of the two texts' lengths for every pair of blocks tried. A long note
that was largely rewritten has many such pairs and little to gain from them.
The comparing is therefore given a budget of 12,000,000 table cells
(`NOTE_DIFF_WORK_BUDGET` in `noteDiff.ts`), about 50 ms, and no one comparison
may take more than an eighth of what is left. A comparison that does not fit
is not made: the two blocks are shown as one removed (red, in the previous
version) and one added (green, in the note), in a change whose marker is
amber, without changed words picked out. Cheap comparisons go on
being made after a costly one is refused, so small edits among rewritten
sections keep their words.

Text the two blocks share at their start and end is not counted, so a long
paragraph with one word changed costs next to nothing. Which blocks are
unchanged, added or removed is never affected: only the pairing of changed
blocks and the words within them are.

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
  revised three times, which leaves three versions;
- a second project note, **Settings redesign: review notes**, written with a
  screenshot and a recording and revised once to drop them. No media is
  stored for it, which is how the clean-up above leaves a dropped image: its
  version names two files that are not there.

The last revision is the one the markers show. Down the note, it holds:

| Marker | Change                             |
| ------ | ---------------------------------- |
| Amber  | A reworded sentence in the summary |
| Amber  | An edited diagram                  |
| Red    | A removed paragraph                |
| Amber  | A changed list item                |
| Amber  | A changed table row                |
| Green  | An added section, _Rollout_        |

The second note's revision holds:

| Marker | Change                               |
| ------ | ------------------------------------ |
| Amber  | The screenshot, replaced by a line   |
| Red    | The recording, removed               |
| Green  | An added finding, the last in a list |

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

Open **Note diffs demo** from the projects grid and press the first note,
**Upload retries: findings and plan**. The right margin of the note should
carry the six markers above, in that order, each beside its change. Hover
each one, press an amber one to open the previous version, scroll either
pane, and press Escape twice: once for the previous version, once for the
note.

What to expect:

- Each marker is level with its change and as tall as it: the one for the
  summary's sentence spans its two lines, the one for the diagram spans the
  diagram, and the green one runs from the _Rollout_ heading to the end of
  its list. They move with the text as the note scrolls. In a window too
  short to show the whole note, the markers of changes further down are out
  of view until the note is scrolled to them.
- Hovering tints the changed blocks and paints the changed words, in the
  amber a code diff gives a modified line and its changed characters. A
  changed diagram is tinted and edged in its marker's colour. The removed paragraph
  has no block in the note: its marker is a short red one above _Findings_,
  and hovering it draws a red line across the gap the paragraph left, level
  with the marker.
- With the previous version open, every changed block is tinted in both
  panes, as the lines of a code diff are, and the pressed change more
  strongly. The previous version has markers of its own beside its blocks: a tall red one beside the removed paragraph, and a short green one
  where the _Rollout_ section is not.
- A press puts the change a third of the way down both panes, level in each.
  A change too near the top of the note to reach that line stays where it
  is, still level. For the removed paragraph the previous version shows it
  lit and the note shows the red line beside it; for the added section it is
  the other way round, with a green line on the left.

With the find field open (Cmd+F) and focused, what Escape does depends on
whether anything of the diff is held open. While the previous version is
open, or a change is selected, Escape closes the search and nothing else; the
previous version stays as it was. Otherwise one Escape closes the search and
the note together, as it does in every other viewer.

Then open the second note, **Settings redesign: review notes**, and press its
amber marker. The previous version shows a placeholder where the screenshot
was, lit red as a removed block, level with the sentence that replaced it,
which is lit green in the note. Press the red marker: the placeholder for the recording is lit, and
the note shows a red line across the gap it left.

To make a version by hand instead, revise a note of your own. An agent
rewriting its note keeps the text it replaced, and so does editing and saving
a written note on a branch.
