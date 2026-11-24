<!--
  NoteDiffConnectors.svelte — the bands joining each change in a note's
  previous version to the same change in the note, in the gutter between
  the two panes.

  They are the code diff viewer's spine connectors: the same outline, from
  the viewer's own geometry (noteDiffConnectors.ts), and at rest the same
  paint, a neutral fill with a hairline along the top and the bottom. The
  band of the change being pointed at is painted in its kind's colour, as
  that change's blocks, marker and gap line are.

  Mounted over the gutter, in a positioned element as tall as the panes'
  scroll areas and ending at the previous version's right edge. A band runs
  from that pane's markers to the tint of the note's blocks, so the gutter
  lies over the padding either side of the seam (the `CONNECTOR_*_REACH`
  constants). The bands are placed from the blocks as measured
  (`noteDiff.boxes`, the measurements the markers stand on) and from how
  far each pane is scrolled (`scrollTops`), and are cut off at the gutter's
  edges: a change scrolled out of one pane leads off the top or the bottom.

  A band is part of its change. Hovering it lights the change in both
  panes, and pressing it selects the change (`onSelect`). It is there for
  the pointer only: the markers name the changes and take the keyboard, so
  the drawing is hidden from a screen reader and has no Tab stop. Only the
  bands take the pointer; a press between them is the text's.
-->
<script lang="ts">
  import {
    CONNECTOR_PREVIOUS_REACH,
    CONNECTOR_WIDTH,
    layoutNoteDiffConnectors,
  } from './noteDiffConnectors';
  import type { NoteDiffSide } from './noteDiff';
  import { untrack } from 'svelte';
  import { ChangeHover, RestingPointer } from './noteDiffPointer';
  import type { NoteDiffState } from './noteDiffState.svelte';

  interface Props {
    noteDiff: NoteDiffState;
    /** How far each pane is scrolled. */
    scrollTops: Record<NoteDiffSide, number>;
    /** The visible height of the panes' scroll areas. */
    height: number;
    /** A band was pressed. */
    onSelect: (hunkId: string) => void;
    /** The wheel was turned over a band, by this many pixels: the gutter scrolls nothing itself. */
    onWheel?: (deltaY: number) => void;
  }

  let { noteDiff, scrollTops, height, onSelect, onWheel }: Props = $props();

  let svgEl = $state<SVGSVGElement>();
  const pointer = new RestingPointer();
  const hover = new ChangeHover();

  let connectors = $derived(
    layoutNoteDiffConnectors(noteDiff.hunks, noteDiff.boxes, {
      scrollTops,
      width: CONNECTOR_WIDTH,
      height,
    })
  );

  $effect(() => {
    if (connectors.length > 0) return pointer.watch();
  });

  // A wheel or a press on a band is the reader taking the panes over, as it
  // is on the text (see `watchReaderScroll`).
  $effect(() => {
    if (svgEl) return noteDiff.watchReaderScroll(svgEl);
  });

  // A band that is no longer drawn was not left by the pointer: one scrolled
  // out of the gutter while hovered, or every band when the pane closes. No
  // `pointerleave` comes for an element that is removed, so the hover it
  // had is ended here.
  $effect(() => {
    const drawn = new Set(connectors.map((connector) => connector.hunkId));
    untrack(() => hover.keep(noteDiff, (hunkId) => drawn.has(hunkId)));
  });

  $effect(() => {
    const state = noteDiff;
    return () => hover.release(state);
  });

  /**
   * The pointer is over a band. Only one that moved there counts, not one
   * at rest that a pane scrolled a band under (noteDiffPointer.ts).
   */
  function pointerOver(event: PointerEvent, hunkId: string) {
    if (pointer.hovers(event)) hover.enter(noteDiff, hunkId);
  }

  function unhover(hunkId: string) {
    hover.leave(noteDiff, hunkId);
  }

  function wheel(event: WheelEvent) {
    const unit =
      event.deltaMode === WheelEvent.DOM_DELTA_LINE
        ? 16
        : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
          ? height
          : 1;
    onWheel?.(event.deltaY * unit);
  }
</script>

{#if connectors.length > 0}
  <svg
    class="note-diff-connectors"
    aria-hidden="true"
    width={CONNECTOR_WIDTH}
    style:left="calc(100% - {CONNECTOR_PREVIOUS_REACH}px)"
    style:width="{CONNECTOR_WIDTH}px"
    bind:this={svgEl}
    onwheel={wheel}
  >
    {#each connectors as connector (connector.hunkId)}
      <g
        class="note-diff-connector"
        class:is-pointed={connector.hunkId === noteDiff.pointedHunkId}
        data-kind={connector.kind}
        data-hunk-id={connector.hunkId}
      >
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <path
          class="connector-fill"
          d={connector.fill}
          onpointerenter={(event) => pointerOver(event, connector.hunkId)}
          onpointermove={(event) => pointerOver(event, connector.hunkId)}
          onpointerleave={() => unhover(connector.hunkId)}
          onclick={() => onSelect(connector.hunkId)}
        />
        <path class="connector-stroke" d={connector.stroke} />
      </g>
    {/each}
  </svg>
{/if}

<style>
  /* Over the padding either side of the seam between the panes, above the
     note's scroll area, which comes after it and is positioned too. Cut off
     at its own box, which is the panes' visible area: the headers above it
     and the bar under it are never drawn over. */
  .note-diff-connectors {
    position: absolute;
    top: 0;
    z-index: 1;
    height: 100%;
    overflow: hidden;
    pointer-events: none;
  }

  /* At rest, the code diff viewer's connector (`readThemeColors` in
     connectorRendererCanvas.ts): the neutral fill of a changed range, and
     the hairline that bounds one. */
  .connector-fill {
    fill: var(--connector-fill, var(--diff-changed-bg));
    cursor: pointer;
    pointer-events: auto;
  }

  .connector-stroke {
    fill: none;
    stroke: var(--connector-stroke, var(--diff-range-border));
    stroke-width: 1px;
    pointer-events: none;
  }

  /* The band of the change being pointed at, in the code diff's colours as
     its blocks and its marker have them (see noteDiffStyles.css): the fill
     is the tint its blocks are lit with, and the lines the solid colour of
     its marker. */
  .note-diff-connector.is-pointed[data-kind='added'] {
    --connector-fill: var(--diff-added-bg-emphasis);
    --connector-stroke: var(--status-added);
  }

  .note-diff-connector.is-pointed[data-kind='removed'] {
    --connector-fill: var(--diff-removed-bg-emphasis);
    --connector-stroke: var(--status-deleted);
  }

  .note-diff-connector.is-pointed[data-kind='modified'] {
    --connector-fill: var(--diff-modified-bg-emphasis);
    --connector-stroke: var(--diff-modified-accent);
  }
</style>
