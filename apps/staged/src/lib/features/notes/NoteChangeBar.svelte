<!--
  NoteChangeBar.svelte — what a note's latest revision changed, as a bar of
  markers beside the note.

  The bar stands at the right edge of the note's scroll area and does not
  scroll with it. Each marker is one hunk of the diff against the previous
  version (noteDiff.ts): green for text that was added, red for text that
  was removed, orange for text that was changed. A marker sits where its
  change is in the note, scaled as the scrollbar is: at the same share of
  the bar as the change is of the note's scrolled height. A note that fits
  without scrolling has its markers beside their blocks.

  Hovering a marker, or moving keyboard focus to it, lights its change in
  the note; pressing it selects the change (`onSelect`), which scrolls the
  note to it. What selecting opens is the viewer's to decide.

  The bar is also where the diff is made, since it is the one piece that is
  there whenever the note is. After each render of the note (`renderKey` is
  what it was rendered from) it reads the note's blocks, has them compared
  with the previous version's (`previousHtml`), and marks the changed ones
  (noteDiffHighlights.ts); it measures them, again whenever the note's
  layout changes. With no previous version, or one that reads the same,
  there are no markers and nothing is rendered.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { NoteDiffHunk } from './noteDiff';
  import {
    applyNoteDiffMarks,
    clearNoteDiffMarks,
    pointAtNoteDiffHunk,
  } from './noteDiffHighlights';
  import { hunkExtent, hunkLead, scrollTopForOffset } from './noteDiffScroll';
  import type { NoteDiffState } from './noteDiffState.svelte';
  import {
    extractNoteDiffUnits,
    measureNoteDiffUnits,
    type NoteDiffDomUnit,
  } from './noteDiffUnits';

  interface Props {
    noteDiff: NoteDiffState;
    /** The rendered note. */
    contentEl: HTMLElement | undefined;
    /** The element that scrolls it. */
    scrollEl: HTMLElement | undefined;
    /** What the note was rendered from; a new render has new blocks. */
    renderKey: unknown;
    /** The previous version, rendered as the note is; `null` when there is none (yet). */
    previousHtml: string | null;
    /** A marker was pressed. */
    onSelect: (hunkId: string) => void;
  }

  let { noteDiff, contentEl, scrollEl, renderKey, previousHtml, onSelect }: Props = $props();

  /** The least a marker is tall, however short its change is in the scaled note. */
  const MIN_MARKER_HEIGHT = 4;

  const KIND_LABELS = { added: 'Added', removed: 'Removed', modified: 'Modified' } as const;

  /** The note's blocks as last read. */
  let units = $state.raw<NoteDiffDomUnit[]>([]);
  /** The scroll area's visible and scrolled heights, as last measured. */
  let track = $state.raw({ clientHeight: 0, scrollHeight: 0 });

  function markerLabel(hunk: NoteDiffHunk): string {
    const blocks = Math.max(hunk.previousIndices.length, hunk.currentIndices.length);
    return `${KIND_LABELS[hunk.kind]}: ${blocks} ${blocks === 1 ? 'block' : 'blocks'}`;
  }

  let markers = $derived.by(() => {
    const boxes = noteDiff.boxes.current;
    const { clientHeight, scrollHeight } = track;
    const scale = clientHeight > 0 && scrollHeight > clientHeight ? clientHeight / scrollHeight : 1;
    return noteDiff.hunks.flatMap((hunk) => {
      const extent = hunkExtent(hunk, 'current', boxes);
      if (!extent) return [];
      const height = Math.max(MIN_MARKER_HEIGHT, extent.height * scale);
      // A change at the very end of the note keeps its marker on the bar.
      const lowest = clientHeight > 0 ? Math.max(0, clientHeight - height) : Infinity;
      return [
        { hunk, top: Math.min(extent.top * scale, lowest), height, label: markerLabel(hunk) },
      ];
    });
  });

  // Read the note's blocks, compare, and mark the changed ones. The markup
  // is rebuilt on every render, so its marks go with it.
  $effect(() => {
    const el = contentEl;
    const html = previousHtml;
    void renderKey;
    if (!el) {
      untrack(() => {
        units = [];
        noteDiff.compare(html, null);
      });
      return;
    }
    untrack(() => {
      const read = extractNoteDiffUnits(el);
      units = read;
      noteDiff.compare(html, read);
      applyNoteDiffMarks(el, read, noteDiff.hunks, 'current');
    });
    return () => clearNoteDiffMarks(el);
  });

  $effect(() => {
    const el = contentEl;
    const hunk = noteDiff.pointedHunk;
    const current = units;
    if (!el) return;
    return untrack(() => pointAtNoteDiffHunk(el, current, hunk, 'current'));
  });

  /**
   * Put the selected change on the note's reference line. One that starts
   * with text only the previous version has is that version's pane to
   * place, while it is shown (see `hunkLead`): the note follows it there.
   */
  function alignActive() {
    const scroller = scrollEl;
    const hunk = noteDiff.activeHunk;
    if (!scroller || !hunk) return;
    const lead = hunkLead(hunk, noteDiff.boxes);
    if (lead?.side === 'current') scroller.scrollTop = scrollTopForOffset(lead.offset, scroller);
  }

  // Measure the blocks, now and whenever the note is laid out again: an
  // image loading, the dialog resized. Opening the previous version changes
  // how far the note scrolls without resizing anything observed, so that
  // measures too. A diff with no changes has nothing to place.
  $effect(() => {
    const el = contentEl;
    const scroller = scrollEl;
    const current = units;
    void noteDiff.open;
    if (!el || !scroller || noteDiff.hunks.length === 0) return;

    const measure = () => {
      noteDiff.setBoxes('current', measureNoteDiffUnits(current, scroller));
      const { clientHeight, scrollHeight } = scroller;
      if (clientHeight !== track.clientHeight || scrollHeight !== track.scrollHeight) {
        track = { clientHeight, scrollHeight };
      }
      if (noteDiff.following) alignActive();
    };
    untrack(measure);
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    observer.observe(scroller);
    return () => observer.disconnect();
  });

  $effect(() => {
    void noteDiff.alignRequest;
    untrack(alignActive);
  });

  $effect(() => {
    if (scrollEl) return noteDiff.watchReaderScroll(scrollEl);
  });

  function unhover(hunkId: string) {
    if (noteDiff.hoveredHunkId === hunkId) noteDiff.hover(null);
  }
</script>

{#if markers.length > 0}
  <div class="note-change-bar" role="group" aria-label="Changes since the previous version">
    {#each markers as { hunk, top, height, label } (hunk.id)}
      <button
        type="button"
        class="note-change-marker"
        class:is-pointed={hunk.id === noteDiff.pointedHunkId}
        data-kind={hunk.kind}
        data-hunk-id={hunk.id}
        style:top="{top}px"
        style:height="{height}px"
        aria-label={label}
        aria-pressed={hunk.id === noteDiff.activeHunkId}
        title={label}
        onpointerenter={(event) => {
          // A finger has no hover: a tap's would stay lit until the next tap.
          if (event.pointerType !== 'touch') noteDiff.hover(hunk.id);
        }}
        onpointerleave={() => unhover(hunk.id)}
        onfocus={() => noteDiff.hover(hunk.id)}
        onblur={() => unhover(hunk.id)}
        onclick={() => onSelect(hunk.id)}
      ></button>
    {/each}
  </div>
{/if}

<style>
  /* In the note's right padding, clear of its text and of the scrollbar at
     the edge (6px wide, see `.modal-content` in NoteModal). The bar itself
     takes no presses: a press between markers is the note's. */
  .note-change-bar {
    position: absolute;
    top: 0;
    bottom: 0;
    right: calc(8px + var(--dialog-safe-right, 0px));
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
    opacity: 0.7;
    cursor: pointer;
    pointer-events: auto;
    transition: opacity 0.1s;
  }

  /* A short marker is still easy to hit. The room is taken above, below and
     towards the text; on the other side the scrollbar is 2px away, and a
     press on it is the scrollbar's. */
  .note-change-marker::before {
    content: '';
    position: absolute;
    inset: -3px -2px -3px -6px;
  }

  .note-change-marker[data-kind='added'] {
    --marker-color: var(--status-added);
  }

  .note-change-marker[data-kind='removed'] {
    --marker-color: var(--status-deleted);
  }

  .note-change-marker[data-kind='modified'] {
    --marker-color: var(--status-modified);
  }

  .note-change-marker:hover,
  .note-change-marker.is-pointed {
    opacity: 1;
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
