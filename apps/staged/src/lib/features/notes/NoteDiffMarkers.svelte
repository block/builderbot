<!--
  NoteDiffMarkers.svelte — the markers of a note diff, beside one version's
  text.

  One marker per hunk of the diff (noteDiff.ts): green for text that was
  added, red for text that was removed, amber for text that was changed.
  A marker stands level with its change: as tall as the hunk's blocks on
  this side, from the top of the first to the bottom of the last. A hunk
  with no block here (in the note, text the revision removed) has a short
  marker across the line NoteDiffGaps draws for it.

  Mounted inside the element that scrolls the version, which must be
  positioned, as NoteDiffGaps is: the markers are placed in its content,
  from the blocks as measured (`noteDiff.boxes`), so they scroll with the
  text and nothing is computed as it scrolls. They stand in the text's
  right padding, at one distance from the scroll area's edge whether or not
  it has a scrollbar. The host sets `--note-diff-markers-inset` when its
  text is inset further than that (a full-screen dialog's safe area).

  Hovering a marker, or moving keyboard focus to it, lights its change;
  pressing it selects the change (`onSelect`). Markers move under a pointer
  that is at rest, when the text scrolls: that is not a hover, or the change
  a press scrolled into the pointer's place would take the light from the
  one that was pressed.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { NoteDiffHunk, NoteDiffSide } from './noteDiff';
  import { ChangeHover, RestingPointer } from './noteDiffPointer';
  import { hunkMarkerBox } from './noteDiffScroll';
  import type { NoteDiffState } from './noteDiffState.svelte';

  interface Props {
    noteDiff: NoteDiffState;
    side: NoteDiffSide;
    /** Names the group of markers for a screen reader. */
    label: string;
    /**
     * Whether Tab stops at the markers. Off for a second set of the same
     * changes, which is there for the pointer.
     */
    tabbable?: boolean;
    /** A marker was pressed. */
    onSelect: (hunkId: string) => void;
  }

  let { noteDiff, side, label, tabbable = true, onSelect }: Props = $props();

  const KIND_LABELS = { added: 'Added', removed: 'Removed', modified: 'Modified' } as const;

  let barEl = $state<HTMLElement>();
  /** The width a scrollbar takes from the right of the scroll area's content, if one does. */
  let scrollbarWidth = $state(0);
  const pointer = new RestingPointer();
  const hover = new ChangeHover();

  function markerLabel(hunk: NoteDiffHunk): string {
    const blocks = Math.max(hunk.previousIndices.length, hunk.currentIndices.length);
    return `${KIND_LABELS[hunk.kind]}: ${blocks} ${blocks === 1 ? 'block' : 'blocks'}`;
  }

  let markers = $derived.by(() => {
    const boxes = noteDiff.boxes[side];
    return noteDiff.hunks.flatMap((hunk) => {
      const box = hunkMarkerBox(hunk, side, boxes);
      return box ? [{ hunk, top: box.top, height: box.height, label: markerLabel(hunk) }] : [];
    });
  });

  // The markers are placed from the content's right edge, which a scrollbar
  // there that takes room moves in: measured, so they keep their place when
  // the version grows one. The scroll area's content box changes when it does.
  $effect(() => {
    const scroller = barEl?.parentElement;
    if (!scroller) return;
    const measure = () => {
      const style = getComputedStyle(scroller);
      // A scroll area laid out right to left has its scrollbar on the left
      // (the previous version's pane), where it moves nothing the markers
      // are placed from. Asked of the style: WebKit leaves a scrollbar there
      // out of `clientLeft`.
      if (style.direction === 'rtl') {
        scrollbarWidth = 0;
        return;
      }
      const borders =
        (parseFloat(style.borderLeftWidth) || 0) + (parseFloat(style.borderRightWidth) || 0);
      scrollbarWidth = Math.max(0, scroller.offsetWidth - scroller.clientWidth - borders);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(scroller);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (markers.length > 0) return pointer.watch();
  });

  // A marker that is no longer drawn was not left by the pointer or by
  // focus: the previous version's, when its pane closes with the pointer on
  // one. No `pointerleave` or `blur` comes for an element that is removed,
  // so the hover it had is ended here.
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
    data-side={side}
    style:--note-diff-scrollbar="{scrollbarWidth}px"
    bind:this={barEl}
  >
    {#each markers as { hunk, top, height, label: name } (hunk.id)}
      <button
        type="button"
        class="note-change-marker"
        class:is-pointed={hunk.id === noteDiff.pointedHunkId}
        data-kind={hunk.kind}
        data-hunk-id={hunk.id}
        style:top="{top}px"
        style:height="{height}px"
        tabindex={tabbable ? undefined : -1}
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
{/if}

<style>
  /* In the text's right padding (24px), clear of the text and of the
     scrollbar at the edge (6px wide, see `.modal-content` in NoteModal).
     `right` counts from the content's edge, which stops at a scrollbar that
     takes room; taking its width off keeps the markers 8px from the scroll
     area's edge either way. The bar has no height of its own and takes no
     presses: a press between markers is the text's. */
  .note-change-bar {
    position: absolute;
    top: 0;
    right: calc(8px - var(--note-diff-scrollbar, 0px) + var(--note-diff-markers-inset, 0px));
    z-index: 1;
    width: 10px;
    pointer-events: none;
  }

  .note-change-marker {
    position: absolute;
    left: 0;
    width: 100%;
    padding: 0;
    border: none;
    border-radius: 2px;
    background-color: var(--marker-color);
    cursor: pointer;
    pointer-events: auto;
    transition: box-shadow 0.1s;
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
     out over the page it muddies. The marker being pointed at gains a halo
     in a tint of the same colour, the one its blocks are lit with (for a
     modified change, the one its changed words are), kept to the 2px
     between the marker and the scrollbar. The halo goes with the light: it
     is not keyed on the pointer being over the marker, which is also true of
     one the text scrolled under a pointer at rest, whose change is not
     lit. */
  .note-change-marker[data-kind='added'] {
    --marker-color: var(--status-added);
    --marker-halo: var(--diff-added-bg-emphasis);
  }

  .note-change-marker[data-kind='removed'] {
    --marker-color: var(--status-deleted);
    --marker-halo: var(--diff-removed-bg-emphasis);
  }

  .note-change-marker[data-kind='modified'] {
    --marker-color: var(--diff-modified-accent);
    --marker-halo: var(--diff-modified-inline-bg);
  }

  .note-change-marker.is-pointed {
    box-shadow: 0 0 0 2px var(--marker-halo);
  }

  .note-change-marker:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: 1px;
  }

  @media (prefers-reduced-motion: reduce) {
    .note-change-marker {
      transition: none;
    }
  }
</style>
