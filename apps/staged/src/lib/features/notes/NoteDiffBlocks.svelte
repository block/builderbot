<!--
  NoteDiffBlocks.svelte — what a note diff paints for each change in one
  version: a rectangle behind the change's text, or a line where the text
  is only in the other version.

  One rectangle per hunk (noteDiff.ts), from the top of its first block on
  this side to the bottom of its last (`hunkBlock`), so the margins between
  its blocks are painted with them, and with one left edge and one right
  edge whatever the blocks are: a heading, a paragraph and the items of a
  list are one shape, not five. Its corners are square. Each block keeps
  the colour of its own change (a block the revision replaced outright is
  red in the previous version and green in the note), so a rectangle is
  filled in parts, one per run of blocks of a kind, with no seam but the
  change of colour.

  A hunk with no block on this side (in the note, text the revision
  removed; in the previous version, text it added) has a line across the
  gap such text would fill, with the same two ends as the rectangles.

  Nothing is added to the rendered note for this: the rectangles are placed
  from the blocks as measured (`noteDiff.boxes`), as the markers are, in an
  element of their own. Mounted inside the element that scrolls the
  version, so they scroll with the text and nothing is computed as it
  scrolls. That element must be positioned, and a stacking context of its
  own (`isolation: isolate`): the rectangles are painted under its text,
  which is not positioned, and over its background. Being under the text
  they take no presses, and the text is selected, and its links and media
  pressed, as if they were not there.

  The host says where the two ends are, from the scroll area's padding box:
  `--note-diff-blocks-left` and `--note-diff-blocks-right`. Beside another
  pane the end on that side is the gutter's edge, where the band joining
  the two versions of the change starts (NoteDiffConnectors.svelte); its
  top and bottom are the rectangle's. `scrollbarWidth` is taken off the
  right, as it is for the markers, which stand at that end in the note.
  `overflowX` is how far the scroll area scrolls sideways, when it is the
  one that scrolls text too wide for it. The rectangles do not scroll
  sideways with the text: they hold their two ends, as the markers do, so a
  change stays one block from the gutter to its marker wherever a wide
  table is scrolled to. For that the host also says how wide the scroll
  area's left padding is (`--note-diff-scroll-padding`).

  Only the hunk being pointed at is painted, unless `showAll`, when every
  change is and the pointed one is the strong one.
-->
<script lang="ts">
  import type { NoteDiffHunk, NoteDiffSide } from './noteDiff';
  import { hunkBlock, hunkGapOffset, type NoteDiffBlock } from './noteDiffScroll';
  import type { NoteDiffState } from './noteDiffState.svelte';

  interface Props {
    noteDiff: NoteDiffState;
    side: NoteDiffSide;
    /** Paint every change, not only the pointed hunk's. */
    showAll?: boolean;
    /** The width a scrollbar takes from the right of the scroll area's content, if one does. */
    scrollbarWidth?: number;
    /** How far the scroll area scrolls sideways, for text that runs past the right of its column. */
    overflowX?: number;
  }

  let { noteDiff, side, showAll = false, scrollbarWidth = 0, overflowX = 0 }: Props = $props();

  /** What is painted for a change here: its rectangle, or the line across the gap it left. */
  interface Painted {
    hunk: NoteDiffHunk;
    active: boolean;
    block: NoteDiffBlock | null;
    gap: number | null;
  }

  let painted = $derived.by(() => {
    const boxes = noteDiff.boxes[side];
    const pointed = noteDiff.pointedHunkId;
    return noteDiff.hunks.flatMap((hunk): Painted[] => {
      const active = hunk.id === pointed;
      if (!showAll && !active) return [];
      const block = hunkBlock(hunk, side, boxes);
      if (block) return [{ hunk, active, block, gap: null }];
      const here = side === 'previous' ? hunk.previousIndices : hunk.currentIndices;
      const gap = here.length === 0 ? hunkGapOffset(hunk, side, boxes) : null;
      return gap === null ? [] : [{ hunk, active, block: null, gap }];
    });
  });
</script>

{#if painted.length > 0}
  <div
    class="note-diff-blocks"
    data-side={side}
    aria-hidden="true"
    style:--note-diff-scrollbar="{scrollbarWidth}px"
    style:--note-diff-overflow="{overflowX}px"
  >
    <div class="note-diff-blocks-column">
      {#each painted as { hunk, active, block, gap } (hunk.id)}
        {#if block}
          <div
            class="note-diff-block"
            class:is-active={active}
            data-kind={hunk.kind}
            data-hunk-id={hunk.id}
            style:top="{block.top}px"
            style:height="{block.height}px"
          >
            {#each block.parts as part, index (index)}
              <div
                class="note-diff-block-part"
                data-kind={part.kind}
                style:height="{part.height}px"
              ></div>
            {/each}
          </div>
        {:else}
          <div
            class="note-diff-gap"
            class:is-active={active}
            data-kind={hunk.kind}
            data-hunk-id={hunk.id}
            style:top="{gap}px"
          ></div>
        {/if}
      {/each}
    </div>
  </div>
{/if}

<style>
  /* Under the text, which is in the flow and not positioned: the scroll
     area is a stacking context, so this is still over its background. It
     has no height of its own; what it holds is placed in the scroll area's
     content, so it scrolls up and down with the text.

     Sideways it does not. This element is as wide as what the scroll area
     scrolls: its padding box, and as far again as it scrolls
     (`--note-diff-overflow`), less the room kept at the right. The column
     inside it is as wide as the rectangles, and sticks where it starts:
     scrolled sideways, it slides along this element and stays where it was
     on screen. With nothing to scroll sideways the column fills this
     element and has nowhere to go.

     WebKit and Chromium count a sticky offset from the scroll area's
     content box, not its padding box, so the left padding is taken off
     (`--note-diff-scroll-padding`). */
  .note-diff-blocks {
    position: absolute;
    top: 0;
    left: 0;
    right: calc(
      var(--note-diff-blocks-right, 0px) - var(--note-diff-scrollbar, 0px) -
        var(--note-diff-overflow, 0px)
    );
    z-index: -1;
    height: 0;
    pointer-events: none;
  }

  .note-diff-blocks-column {
    position: sticky;
    left: calc(var(--note-diff-blocks-left, 0px) - var(--note-diff-scroll-padding, 0px));
    width: calc(100% - var(--note-diff-overflow, 0px) - var(--note-diff-blocks-left, 0px));
    height: 0;
    margin-left: var(--note-diff-blocks-left, 0px);
  }

  .note-diff-block,
  .note-diff-gap {
    position: absolute;
    left: 0;
    right: 0;
  }

  .note-diff-block {
    overflow: hidden;
  }

  /* The code diff's colours (see noteDiffStyles.css): a part is the tint the
     viewer gives a line of its kind, and the stronger tint of the same
     colour in the change being pointed at. */
  .note-diff-block-part {
    background-color: var(--part-tint);
  }

  .note-diff-block-part[data-kind='added'] {
    --part-tint: var(--diff-added-bg);
    --part-strong: var(--diff-added-bg-emphasis);
  }

  .note-diff-block-part[data-kind='removed'] {
    --part-tint: var(--diff-removed-bg);
    --part-strong: var(--diff-removed-bg-emphasis);
  }

  .note-diff-block-part[data-kind='modified'] {
    --part-tint: var(--diff-modified-bg);
    --part-strong: var(--diff-modified-bg-emphasis);
  }

  .note-diff-block.is-active > .note-diff-block-part {
    background-color: var(--part-strong);
  }

  /* A line along the top and the bottom, inside the rectangle, as the
     viewer bounds a changed range, and in its colour at rest. They run on
     into the lines along the band that joins the two versions. Around the
     change being pointed at they are the solid colour of its marker. */
  .note-diff-block::after {
    content: '';
    position: absolute;
    inset: 0;
    border-top: 1px solid var(--block-line, var(--diff-range-border));
    border-bottom: 1px solid var(--block-line, var(--diff-range-border));
  }

  .note-diff-block.is-active[data-kind='added'] {
    --block-line: var(--status-added);
  }

  .note-diff-block.is-active[data-kind='removed'] {
    --block-line: var(--status-deleted);
  }

  .note-diff-block.is-active[data-kind='modified'] {
    --block-line: var(--diff-modified-accent);
  }

  /* Where the other version has text: the two lines of a rectangle with
     nothing between them. The band tapers to its end. The line of the
     change being pointed at is the solid colour of its marker, and the
     others the emphasised tint of the same kind. */
  .note-diff-gap {
    height: 2px;
    margin-top: -1px;
    background-color: var(--gap-faint);
  }

  .note-diff-gap.is-active {
    background-color: var(--gap-color);
  }

  .note-diff-gap[data-kind='added'] {
    --gap-color: var(--status-added);
    --gap-faint: var(--diff-added-bg-emphasis);
  }

  .note-diff-gap[data-kind='removed'] {
    --gap-color: var(--status-deleted);
    --gap-faint: var(--diff-removed-bg-emphasis);
  }
</style>
