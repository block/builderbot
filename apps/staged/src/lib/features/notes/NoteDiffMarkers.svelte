<!--
  NoteDiffMarkers.svelte — the markers of a note diff, beside the note's
  text.

  One marker per hunk of the diff (noteDiff.ts): green for text that was
  added, red for text that was removed, amber for text that was changed.
  A marker stands level with its change: as tall as the hunk's blocks in
  the note, from the top of the first to the bottom of the last, which is
  the height of the rectangle painted behind them (NoteDiffBlocks.svelte).
  It stands at that rectangle's right end, as its solid end. A hunk with no
  block here (text the revision removed) has a short marker across the end
  of the line drawn for it.

  Only the note has markers. Its previous version, beside it, has none: a
  change there is joined to its place in the note by a band
  (NoteDiffConnectors.svelte), which is pointed at and pressed as a marker
  is, and every change, removed text included, has a marker here.

  Mounted inside the element that scrolls the note, which must be
  positioned, as NoteDiffBlocks is: the markers are placed in its content,
  from the blocks as measured (`noteDiff.boxes`), so they scroll with the
  text and nothing is computed as it scrolls. They stand in the text's
  right padding, at one distance from the scroll area's edge whether or not
  it has a scrollbar (`scrollbarWidth`), and stay there when a wide table
  makes the note scroll sideways (`overflowX`). The host sets
  `--note-diff-markers-inset` when its text is inset further than that (a
  full-screen dialog's safe area).

  Hovering a marker, or moving keyboard focus to it, lights its change;
  pressing it selects the change (`onSelect`). Markers move under a pointer
  that is at rest, when the text scrolls: that is not a hover, or the change
  a press scrolled into the pointer's place would take the light from the
  one that was pressed.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { NoteDiffHunk } from './noteDiff';
  import { ChangeHover, RestingPointer } from './noteDiffPointer';
  import { hunkMarkerBox } from './noteDiffScroll';
  import type { NoteDiffState } from './noteDiffState.svelte';

  interface Props {
    noteDiff: NoteDiffState;
    /** Names the group of markers for a screen reader. */
    label: string;
    /** The width a scrollbar takes from the right of the scroll area's content, if one does. */
    scrollbarWidth?: number;
    /** How far the scroll area scrolls sideways, for text that runs past the right of its column. */
    overflowX?: number;
    /** A marker was pressed. */
    onSelect: (hunkId: string) => void;
  }

  let { noteDiff, label, scrollbarWidth = 0, overflowX = 0, onSelect }: Props = $props();

  const KIND_LABELS = { added: 'Added', removed: 'Removed', modified: 'Modified' } as const;

  const pointer = new RestingPointer();
  const hover = new ChangeHover();

  function markerLabel(hunk: NoteDiffHunk): string {
    const blocks = Math.max(hunk.previousIndices.length, hunk.currentIndices.length);
    return `${KIND_LABELS[hunk.kind]}: ${blocks} ${blocks === 1 ? 'block' : 'blocks'}`;
  }

  let markers = $derived.by(() => {
    const boxes = noteDiff.boxes.current;
    return noteDiff.hunks.flatMap((hunk) => {
      const box = hunkMarkerBox(hunk, 'current', boxes);
      return box ? [{ hunk, top: box.top, height: box.height, label: markerLabel(hunk) }] : [];
    });
  });

  // Markers or none, as the connectors do (NoteDiffConnectors.svelte).
  $effect(() => pointer.watch());

  // A marker that is no longer drawn was not left by the pointer or by
  // focus: every one, when a new diff has other changes. No `pointerleave`
  // or `blur` comes for an element that is removed, so the hover it had is
  // ended here.
  $effect(() => {
    const drawn = new Set(markers.map(({ hunk }) => hunk.id));
    untrack(() => hover.keep(noteDiff, (hunkId) => drawn.has(hunkId)));
  });

  $effect(() => {
    const state = noteDiff;
    return () => hover.release(state);
  });

  /**
   * The pointer is over a marker. Only one that moved there counts, not one
   * at rest that the text scrolled a marker under (noteDiffPointer.ts).
   */
  function pointerOver(event: PointerEvent, hunkId: string) {
    if (pointer.hovers(event)) hover.enter(noteDiff, hunkId);
  }

  function unhover(hunkId: string) {
    hover.leave(noteDiff, hunkId);
  }
</script>

{#if markers.length > 0}
  <div
    class="note-change-bar"
    role="group"
    aria-label={label}
    style:--note-diff-scrollbar="{scrollbarWidth}px"
    style:--note-diff-overflow="{overflowX}px"
  >
    <div class="note-change-markers">
      {#each markers as { hunk, top, height, label: name } (hunk.id)}
        <button
          type="button"
          class="note-change-marker"
          class:is-pointed={hunk.id === noteDiff.pointedHunkId}
          data-kind={hunk.kind}
          data-hunk-id={hunk.id}
          style:top="{top}px"
          style:height="{height}px"
          aria-label={name}
          aria-pressed={hunk.id === noteDiff.activeHunkId}
          title={name}
          onpointerenter={(event) => pointerOver(event, hunk.id)}
          onpointermove={(event) => pointerOver(event, hunk.id)}
          onpointerleave={() => unhover(hunk.id)}
          onfocus={() => hover.enter(noteDiff, hunk.id)}
          onblur={() => unhover(hunk.id)}
          onclick={() => onSelect(hunk.id)}
        ></button>
      {/each}
    </div>
  </div>
{/if}

<style>
  /* In the text's right padding (24px), clear of the text and of the
     scrollbar at the edge (6px wide, see `.modal-content` in NoteModal).
     `right` counts from the content's edge, which stops at a scrollbar that
     takes room; taking its width off keeps the markers 8px from the scroll
     area's edge either way. Their left edge, 18px from it, is where the
     rectangles painted behind the changes end (`--note-diff-blocks-right`
     in NoteModal). The bar has no height of its own and takes no presses: a
     press between markers is the text's.

     The markers scroll up and down with the text and hold still sideways,
     as the rectangles they end do (see NoteDiffBlocks.svelte for how): the
     bar is as wide as what the scroll area scrolls, and the markers stand
     at the right of a column as wide as it shows, which sticks to its left
     edge. */
  .note-change-bar {
    position: absolute;
    top: 0;
    left: 0;
    right: calc(
      8px - var(--note-diff-scrollbar, 0px) + var(--note-diff-markers-inset, 0px) -
        var(--note-diff-overflow, 0px)
    );
    z-index: 1;
    height: 0;
    pointer-events: none;
  }

  .note-change-markers {
    position: sticky;
    left: calc(0px - var(--note-diff-scroll-padding, 0px));
    width: calc(100% - var(--note-diff-overflow, 0px));
    height: 0;
  }

  .note-change-marker {
    position: absolute;
    right: 0;
    width: 10px;
    padding: 0;
    border: none;
    border-radius: 0;
    background-color: var(--marker-color);
    cursor: pointer;
    pointer-events: auto;
  }

  /* A short marker is still easy to hit. The room is taken above, below and
     towards the text; on the other side the scrollbar is 2px away, and a
     press on it is the scrollbar's. */
  .note-change-marker::before {
    content: '';
    position: absolute;
    inset: -3px -2px -3px -6px;
  }

  /* The colours are the code diff's (see noteDiffStyles.css). A marker is
     the solid colour its blocks' tint is made of, at full strength: washed
     out over the page it muddies. It is square, the end of the rectangle
     behind its change, and has nothing round it: the change being pointed
     at is shown by that rectangle, which a ring here would cut across. */
  .note-change-marker[data-kind='added'] {
    --marker-color: var(--status-added);
  }

  .note-change-marker[data-kind='removed'] {
    --marker-color: var(--status-deleted);
  }

  .note-change-marker[data-kind='modified'] {
    --marker-color: var(--diff-modified-accent);
  }

  .note-change-marker:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: 1px;
  }
</style>
