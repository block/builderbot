<!--
  NoteDiffGaps.svelte — where text only the other version has would be.

  A hunk with no block on this side has nothing here to tint (see
  noteDiffHighlights.ts): in the note, text the revision removed; in the
  previous version, text it added. This draws a line across the gap such
  text would fill, in the hunk's colour, between the blocks either side of
  it.

  Mounted inside the element that scrolls the version, which must be
  positioned: the lines are placed in its content and scroll with it. Only
  the hunk being pointed at has its line, unless `showAll`, when every gap
  has one and the pointed hunk's is the strong one. The hunk's marker stands
  across the line's end (NoteDiffMarkers.svelte).
-->
<script lang="ts">
  import type { NoteDiffSide } from './noteDiff';
  import { hunkGapOffset } from './noteDiffScroll';
  import type { NoteDiffState } from './noteDiffState.svelte';

  interface Props {
    noteDiff: NoteDiffState;
    side: NoteDiffSide;
    /** Draw every gap's line, not only the pointed hunk's. */
    showAll?: boolean;
  }

  let { noteDiff, side, showAll = false }: Props = $props();

  let gaps = $derived.by(() => {
    const boxes = noteDiff.boxes[side];
    const pointed = noteDiff.pointedHunkId;
    return noteDiff.hunks.flatMap((hunk) => {
      const here = side === 'previous' ? hunk.previousIndices : hunk.currentIndices;
      if (here.length > 0) return [];
      const active = hunk.id === pointed;
      if (!showAll && !active) return [];
      const top = hunkGapOffset(hunk, side, boxes);
      return top === null ? [] : [{ hunk, top, active }];
    });
  });
</script>

{#each gaps as { hunk, top, active } (hunk.id)}
  <div
    class="note-diff-gap"
    class:is-active={active}
    data-kind={hunk.kind}
    data-hunk-id={hunk.id}
    style:top="{top}px"
    aria-hidden="true"
  ></div>
{/each}

<style>
  /* Across the text column: the scroll area's side padding is 24px. */
  .note-diff-gap {
    position: absolute;
    left: calc(24px + var(--dialog-safe-left, 0px));
    right: calc(24px + var(--dialog-safe-right, 0px));
    height: 2px;
    margin-top: -1px;
    border-radius: 1px;
    background-color: var(--gap-color);
    opacity: 0.45;
    pointer-events: none;
  }

  .note-diff-gap.is-active {
    opacity: 1;
  }

  .note-diff-gap[data-kind='added'] {
    --gap-color: var(--status-added);
  }

  .note-diff-gap[data-kind='removed'] {
    --gap-color: var(--status-deleted);
  }
</style>
