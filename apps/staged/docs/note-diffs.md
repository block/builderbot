# Note diffs

When a note has been revised, the note viewer shows what the last revision
changed. Each change has a marker in the note's right margin: green for
added, red for removed, amber for modified. The colours are the code diff
viewer's (see [Colours](#colours)). A marker stands beside its
change, as tall as the changed text, and scrolls with the note. Hovering a
marker highlights that change in the note: one square-cornered rectangle
behind all of its text, ending at the marker (see
[The rectangle behind a change](#the-rectangle-behind-a-change)). Pressing a
marker opens the previous version in a pane on the left, with the change lit
in both panes and the two scrolling together. Between the two panes each
change's old text is joined to its new text by a band, as the two sides of a
change are in the code diff viewer, and the rectangles run to the band so
that the three are one shape (see [The connectors](#the-connectors)).

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

## The rectangle behind a change

A change is painted as one rectangle, whatever it holds: a heading, a
sentence and three list items that were added together are one green block,
not five. `NoteDiffBlocks.svelte` paints it, in the note and in the previous
version alike.

- **One box.** From the top of the change's first block to the bottom of its
  last (`hunkBlock` in `noteDiffScroll.ts`), margins between them included.
  It reaches 4px into the margin above and below (`BLOCK_BLEED`) so that
  text does not sit against its edge, but never past half the room to the
  neighbouring block: two changes side by side meet and do not overlap, and
  a changed table row is exactly its row.
- **One left edge and one right.** The rectangle is as wide as the pane's
  text column and its padding, not as wide as each block: a list item's
  indent, a nested quote or a narrow image does not step it in. In the note
  it starts half-way into the left padding and ends at the left edge of the
  change's marker, which stands as its solid right end. In the previous
  version it runs from the pane's left edge to the gutter. In the
  side-by-side view the note's rectangle starts at the gutter too.
- **Square.** Nothing of a change is rounded: not the rectangle, not the
  marker, not the lines round the change being pointed at. The words that
  changed within a block are painted by the browser's Custom Highlight API,
  which draws them square as well.
- **Behind the text, not in it.** The rendered markdown comes from `{@html}`
  and holds what the author wrote and nothing else. The rectangles are in a
  layer of their own inside the element that scrolls the version, placed in
  its content from the blocks as measured, so they scroll with the text and
  nothing is computed as it scrolls. The layer is under the text
  (`z-index: -1` in a scroll area that is its own stacking context), takes
  no pointer events and is hidden from a screen reader: text is selected,
  links are followed and media is pressed exactly as without it. Quote
  highlights, find-in-note and the changed words are Custom Highlights on
  the text and are not affected.
- **Per-block colours.** A change that holds blocks of more than one kind is
  still one rectangle, filled in parts: a paragraph reworded and another
  added under it is amber, then green, with the seam half-way between the
  two blocks. A block replaced outright is red in the previous version and
  green in the note, under one amber marker.
- **Blocks with a background of their own.** A code block, a table's header
  cells and a diagram's drawing are opaque and would hide the rectangle, so
  those take the same tint in place (`noteDiffStyles.css`). Nothing else in
  the markup is tinted, outlined or rounded for the diff.
- **Diagram and media cards.** A diagram or a media figure is a card with a
  rounded neutral border of its own, which it keeps: the border is the
  card's, not the change's. The change is the square rectangle the card
  stands on, which shows beside it and in its corners. A diagram's drawing
  is tinted; an image or a video, whose colours are the thing to compare,
  is left as it is.
- **Lines.** A 1px line runs along the top and the bottom of a rectangle,
  inside it: the viewer's `--diff-range-border` at rest, as the viewer
  bounds a changed range, and the solid colour of the marker round the
  change being pointed at. There is none down the sides: the right end is
  the marker's, or the gutter's.
- **Where the other version has the text.** Removed text has no block in the
  note, and added text none in the previous version. There the change is a
  2px line across the gap, with the rectangles' left and right ends: the two
  lines of a rectangle with nothing between them.
- **Sideways.** A table or an unbreakable string can make a version scroll
  sideways. The rectangles do not: they keep their two ends, and the text
  moves over them, so a change is still one block from the gutter to its
  marker and its band still ends on it. The previous version scrolls
  sideways in a box of its own inside its scroll area, and its rectangles
  are outside that box. The note's scroll area scrolls both ways itself, so
  its rectangles and its markers are each in a column that sticks sideways
  (`position: sticky`) inside an element as wide as what the area scrolls.
  WebKit and Chromium count a sticky offset from the scroll area's content
  box, so its left padding is taken off (`--note-diff-scroll-padding`).

In the one-pane view only the change being pointed at is painted. With the
previous version open every change is, in both panes, and the one pointed at
more strongly.

## Where the markers stand

The markers are part of what the note scrolls (`NoteDiffMarkers.svelte`,
mounted inside the scroll area), so nothing is computed as it scrolls. Each is
placed from its blocks as measured (`hunkMarkerBox` in `noteDiffScroll.ts`):

- A change with blocks in the note has a marker as tall as the rectangle
  behind them (`hunkBlockExtent`), which it ends. A reworded three-line
  paragraph has a marker three lines tall; an added section has one that
  spans the section.
- Removed text has no block in the note. Its marker is 8px tall, across the
  end of the red line that is drawn where the text was.
- The blocks are measured again whenever the note is laid out (an image
  loading, the dialog or the window resized, the previous version opening or
  closing, text wrapping again), and the markers and rectangles follow.
- They stand in the note's right padding, 12px clear of the text and 8px from
  the scroll area's edge, which leaves the scrollbar and the dialog's resize
  handle free. That distance is the same whether or not the note has a
  scrollbar.
- They are square, and nothing is drawn round the one whose change is lit:
  the rectangle shows that.

Only the note has markers. The previous version has none: the vertical bars
beside it stood between its text and the bands, and without them the
rectangle behind a change runs straight into its band. Nothing is lost by
that. Every change, removed text included, has a marker in the note, which
takes hover, focus, Tab and a press as before, and in the side-by-side view
every change has a band, which takes hover and a press. Those are what the
previous version's markers offered (they were never Tab stops).

A marker that scrolls under a pointer at rest is not hovered: only a pointer
that moved onto it lights the change. Otherwise the change a press scrolled
into the pointer's place would take the light from the one that was pressed.
A pointer that has not been seen to move at all counts as at rest: the bands
between the panes appear when a key opens the previous version, possibly
under a pointer that has been still since before they were drawn. Once
mounted, the markers and the bands follow the pointer whether or not they
have anything drawn, so one that moves into a gutter with no band in it is
still at rest when a band scrolls under it.

A hover ends when the pointer or the focus leaves, and neither is reported
for an element that is removed. So a hover is also let go of when what was
hovered is no longer drawn: a band scrolled out of the gutter, the bands
when the previous version's pane closes, and any marker when its markers
are taken down (`ChangeHover` in `noteDiffPointer.ts`). Closing the
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
the document root, and a note's band takes the colours of the rectangles it
joins; and
a canvas draws nothing in jsdom, where the component tests run. SVG paths are
hit-tested by the browser and styled from CSS.

- **Edges.** The left edge of a band is the edge of the rectangle painted
  behind the change in the previous version, and the right edge that of its
  rectangle in the note: the same top and the same bottom, from one
  function (`hunkBlockExtent`), so there is nothing between a band's end and
  the block it starts from. A side with no blocks (the note's, for removed
  text; the previous version's, for added text) is a point at the middle of
  that side's gap line, so the band tapers to the line's end, as the
  viewer's do for an insertion or a deletion.
- **One shape.** A band is filled with the tint of the rectangles it joins
  and lined along its top and bottom with the same 1px line they carry, half
  a pixel inside its edge so that the line lies within the change and runs
  on from theirs. For the change being pointed at, both rectangles, the
  band and the lines are its kind's stronger tint and solid colour. At
  rest they are the resting tint and the viewer's `--diff-range-border`
  hairline: the hairline along the block's top and bottom continues across
  the gutter, as the viewer's `range-start` and `range-end` lines meet its
  connector. Where the two rectangles are not one colour (a block replaced
  outright: red on the left, green on the right) the band shades from the
  one to the other.
- **Where they come from.** `layoutNoteDiffConnectors` in
  `noteDiffConnectors.ts` is a pure function of the hunks, the blocks as
  measured (`noteDiff.boxes`) and how far each pane is scrolled. The
  measurements are the ones the markers and the rectangles stand on, taken
  by the same `ResizeObserver`s, so whatever moves a block (an image
  loading, a resize, text wrapping again, the pane opening) moves its
  rectangle, its marker and its band together. Nothing is measured for the
  connectors alone.
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
- **One gutter.** A band is 24px wide, over the seam between the panes:
  12px of the previous version's right padding and 12px of the note's left
  padding (the `CONNECTOR_*_REACH` constants), which leaves 12px between it
  and the text on either side. Each pane's rectangles run through the rest
  of that padding and stop where the gutter starts, so no strip of the
  pane's background shows between a block and its band. The previous
  version's scrollbar is on its left, the outer edge, as the viewer's old
  pane has it, so nothing lies between its text and the bands. It is
  there because that scroll area is laid out right to left, which also makes
  it scroll from its right edge: what runs past that edge cannot be reached,
  so the area scrolls up and down only. The version is in a box inside it,
  laid out left to right, and that box scrolls sideways when a table or a
  string that cannot wrap is wider than the pane, as the note's scroll area
  does for the note. Text wraps to the pane's width in both, so the two
  versions are laid out alike. The rectangles and gap lines are outside the
  box and do not move with it. Its sideways scrollbar is under the version's
  last block, where the note's is at the bottom of its pane. The line
  dividing the panes is drawn beside their headers only. The note's own
  markers stay in its right margin, where they are in the one-pane view, so
  opening the previous version moves none of them.
- **Pointing.** Hovering a band lights its change in both panes, as hovering
  a marker does, and pressing it selects the change. Only the bands take the
  pointer; between them a press is the text's. The wheel over a band scrolls
  the note, and so both panes. As with markers, a band that scrolls under a
  pointer at rest is not hovered (`noteDiffPointer.ts`). The bands are the
  only thing on the previous version's side that takes the pointer for a
  change, now that it has no markers.
- **Keyboard and screen readers.** The bands add nothing to either. The
  note's markers name every change and take Tab; the drawing is
  `aria-hidden` and has no Tab stop.

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
  change's marker and the lines round it are amber.
- The tints above are the resting ones, behind every change while the
  previous version is open. The change being pointed at takes a stronger tint
  of the same colour: `--diff-added-bg-emphasis`,
  `--diff-removed-bg-emphasis` and `--diff-modified-bg-emphasis`. The viewer
  has no such state, so these were added beside its tokens, the first two in
  `theme.ts` and the amber one in `app.css`. The amber emphasis stays under
  the inline colour, so changed words show on it.
- Markers, the lines along the rectangle and the band of the change being
  pointed at, and its gap line, are solid, in the colour the tint is made
  of: `--status-added` and `--status-deleted` (which the diff's file list
  also uses for a file's added and removed lines) and
  `--diff-modified-accent`, the amber at full strength. `--status-modified`
  is not used: on the light theme it is a dark gold meant to be read as
  text, and beside the amber tint it looks brown. The other gap lines are
  the emphasised tint.
- At rest a rectangle and a band are lined with `--diff-range-border`, the
  hairline the viewer bounds a changed range and its connector with. The
  viewer fills a connector at rest with one neutral, `--diff-changed-bg`; a
  note's band takes its rectangles' tint instead, because a neutral band
  between two tinted blocks reads as a third thing and not as their join.
  No colour was added for any of this: every token here was in use before.
  The viewer's own hover fill, `--bg-hover`, is a neutral and is not used:
  it would not say which change is lit.
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
- Hovering paints one square rectangle behind the change, from just left of
  the text to the marker, and the changed words in it, in the amber a code
  diff gives a modified line and its changed characters. The green one
  behind _Rollout_ is a single block from above the heading to under the
  last list item, with one left edge: the list does not step it in. A
  changed diagram keeps its rounded card and stands on a square amber
  rectangle. The removed paragraph has no block in the note: its marker is
  a short red one above _Findings_, and hovering it draws a red line across
  the gap the paragraph left, ending at the marker.
- With the previous version open, every change has its rectangle in both
  panes, as the lines of a code diff are tinted, and the pressed change
  more strongly. The previous version has no markers: its rectangles run to
  the gutter, the removed paragraph's in red, with a green line where the
  _Rollout_ section is not.
- A press puts the change a third of the way down both panes, level in each.
  A change too near the top of the note to reach that line stays where it
  is, still level. For the removed paragraph the previous version shows it
  lit and the note shows the red line beside it; for the added section it is
  the other way round, with a green line on the left.
- Between the panes each change has a band from its rectangle in the
  previous version to its rectangle in the note, with no gap at either end:
  the same tint, and the same hairline along the top and bottom, so the two
  blocks and the band are one shape. The pressed change's is stronger and
  lined in its solid colour.
  The band of the removed paragraph narrows to a point at the red line in
  the note, and the added section's to a point at the green line on the
  left. Hover a band: that change lights in both panes. Press one: it is
  selected and scrolled to. Scroll the previous version on its own, or
  scroll to either end of the note: the bands slant to follow both sides,
  and run off the top or bottom of the gutter, never over the pane headers.
  Tab still stops at the note's markers only.
- With the pointer on a band, press Escape: the pane closes and nothing in the note stays lit. Tab to a
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
which is lit green in the note; the band between them shades from red to
green. Press the red marker: the placeholder for the recording is lit, and
the note shows a red line across the gap it left.

To make a version by hand instead, revise a note of your own. An agent
rewriting its note keeps the text it replaced, and so does editing and saving
a written note on a branch.
