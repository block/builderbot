# Note diffs

When a note has been revised, the note viewer shows what the last revision
changed. Each change has a marker in the note's right margin: green for
added, red for removed, amber for modified. The colours are the code diff
viewer's (see [Colours](#colours)). A marker stands beside its
change, as tall as the changed text, and scrolls with the note. Hovering a
marker highlights that change in the note. Pressing one opens the previous
version in a pane on the left, with the change lit in both panes and the two
scrolling together. Between the two panes each change's old text is joined to
its new text by a band, as the two sides of a change are in the code diff
viewer (see [The connectors](#the-connectors)).

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
A pointer that has not been seen to move at all counts as at rest: the bands
between the panes appear when a key opens the previous version, possibly
under a pointer that has been still since before they were drawn. Once
mounted, the markers and the bands follow the pointer whether or not they
have anything drawn, so one that moves into a gutter with no band in it is
still at rest when a band scrolls under it. The halo of a marker goes with
the light, so a marker under a resting pointer has none.

A hover ends when the pointer or the focus leaves, and neither is reported
for an element that is removed. So a hover is also let go of when what was
hovered is no longer drawn: a band scrolled out of the gutter, the previous
version's markers and bands when its pane closes, and any marker when its
markers are taken down (`ChangeHover` in `noteDiffPointer.ts`). Closing the
previous version, by Escape or otherwise, lets go of the selected change
only. A hover whose marker is still there is kept: a marker of the note
that has keyboard focus, or the pointer on it, goes on showing its change
after Escape, and after the second press that lets go of a change in a
window too narrow for two panes, until focus or the pointer leaves it.

## The connectors

While the previous version is open, every change has a band across the gutter
between the panes, from its blocks in the previous version to its blocks in
the note (`NoteDiffConnectors.svelte`, mounted by `NotePreviousPane.svelte`).
There are none in the one-pane view, nor in a window too narrow for two
panes, where the previous version does not open.

They are the code diff viewer's spine connectors, drawn from the same
definition. The viewer's outline (a band whose top and bottom edges leave
each pane level and curve between them, with the control points half-way
across) was in its canvas renderer; it is now in
`packages/diff-viewer/src/lib/utils/connectorGeometry.ts`, written against a
small path interface that a canvas `Path2D` and an SVG path string both
satisfy. `connectorRendererCanvas.ts` traces it onto its canvas and the note
diff into SVG paths, so the two cannot drift apart.

The canvas renderer itself is not reused. It places connectors by line number
times one line height, where a note's blocks have measured tops and heights;
it has no notion of which connector the pointer is over, and a note's
connectors are hovered and pressed; it reads one neutral pair of colours from
the document root, and the band being pointed at takes its kind's colour; and
a canvas draws nothing in jsdom, where the component tests run. SVG paths are
hit-tested by the browser and styled from CSS.

- **Edges.** The left edge of a band is the extent of the change's blocks in
  the previous version, and the right edge their extent in the note, each
  from the top of the first block to the bottom of the last. A side with no
  blocks (the note's, for removed text; the previous version's, for added
  text) is a point at that side's gap line, so the band tapers to it, as the
  viewer's do for an insertion or a deletion.
- **Where they come from.** `layoutNoteDiffConnectors` in
  `noteDiffConnectors.ts` is a pure function of the hunks, the blocks as
  measured (`noteDiff.boxes`) and how far each pane is scrolled. The
  measurements are the ones the markers stand on, taken by the same
  `ResizeObserver`s, so whatever moves a block (an image loading, a resize,
  text wrapping again, the pane opening) moves its marker and its band
  together. Nothing is measured for the connectors alone.
- **Scrolling.** The panes' scroll offsets are read in the scroll handler
  that already keeps the two panes together, and the bands are laid out
  again before that frame is painted. A band is drawn from where each pane
  actually is, so while one pane is held at its end and the other scrolls
  on, or after scrolling the previous version alone, a change's two sides
  sit at different heights and its band slants between them.
- **Clipping.** The drawing is as tall as the panes' scroll areas and starts
  below their headers. A change scrolled out of one pane leads off the top
  or bottom edge; one whose two sides are both above the gutter, or both
  below it, is not drawn.
- **One gutter.** The previous version's markers are the left end of the
  bands, and the tint of the note's blocks the right end. A band is 29px
  wide: it starts at the right edge of the previous version's markers, 8px
  inside that pane, and crosses 21px of the note's left padding to stop
  where the note's blocks are tinted, 3px short of the text. The previous
  version's scrollbar is on its left, the outer edge, as the viewer's old
  pane has it, so nothing lies between its markers and the bands. It is
  there because that scroll area is laid out right to left, which also makes
  it scroll from its right edge: what runs past that edge cannot be reached,
  so the area scrolls up and down only. The version is in a box inside it,
  laid out left to right, and that box scrolls sideways when a table or a
  string that cannot wrap is wider than the pane, as the note's scroll area
  does for the note. Text wraps to the pane's width in both, so the two
  versions are laid out alike. The markers and gap lines are beside the box
  and do not move with it. Its sideways scrollbar is under the version's
  last block, where the note's is at the bottom of its pane. The line
  dividing the panes is drawn beside their headers only. The note's own
  markers stay in its right margin, where they are in the one-pane view, so
  opening the previous version moves none of them.
- **Pointing.** Hovering a band lights its change in both panes, as hovering
  a marker does, and pressing it selects the change. Only the bands take the
  pointer; between them a press is the text's. The wheel over a band scrolls
  the note, and so both panes. As with markers, a band that scrolls under a
  pointer at rest is not hovered (`noteDiffPointer.ts`).
- **Keyboard and screen readers.** The bands add nothing to either. The
  markers name every change and take Tab; the drawing is `aria-hidden` and
  has no Tab stop.

A band between two sides far apart is steep, and thin where it crosses the
gutter, as the viewer's are. It is easiest to point at near either end.

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
- A connector at rest is painted as the viewer's are: `--diff-changed-bg`
  filled, with a 1px `--diff-range-border` line along its top and bottom, in
  every kind of change. The viewer has no pointed-at connector. The band of
  the change being pointed at is filled with its kind's `-emphasis` tint and
  lined in the kind's solid colour, the same pair that lights its blocks and
  its marker. The viewer's own hover fill, `--bg-hover`, is a neutral and is
  not used: it would not say which change is lit.
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
- Between the panes each change has a band from its marker beside the
  previous version to its blocks in the note: grey with a hairline top and
  bottom, as in a code diff, and in the change's colour for the one pressed.
  The band of the removed paragraph narrows to a point at the red line in
  the note, and the added section's to a point at the green line on the
  left. Hover a band: that change lights in both panes. Press one: it is
  selected and scrolled to. Scroll the previous version on its own, or
  scroll to either end of the note: the bands slant to follow both sides,
  and run off the top or bottom of the gutter, never over the pane headers.
  Tab still stops at the note's markers only.
- With the pointer on a band, or on a marker beside the previous version,
  press Escape: the pane closes and nothing in the note stays lit. Tab to a
  marker of the note, press Enter and then Escape: the pane closes and the
  change stays lit while the marker has focus.

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
