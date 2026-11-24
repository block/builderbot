<!--
  NoteDiffConnectors.svelte — the bands joining each change in a note's
  previous version to the same change in the note, in the gutter between
  the two panes.

  They are the code diff viewer's spine connectors: the same outline, from
  the viewer's own geometry (noteDiffConnectors.ts), with a line along the
  top and the bottom. A band is the middle of one shape. Each pane paints a
  rectangle behind the change's text that runs to the gutter
  (NoteDiffBlocks.svelte), and the band starts at one rectangle's edge and
  ends at the other's, at their tops and bottoms, in their colour: the tint
  of a line of that kind at rest, with the viewer's hairline along it, and
  for the change being pointed at the stronger tint, lined in the solid
  colour of its marker. Where the two rectangles are not one colour (text
  replaced outright is red in the previous version and green in the note)
  the band shades from one to the other. On a side with no text of the
  change the band tapers to the end of the line drawn there.

  Mounted over the gutter, in a positioned element as tall as the panes'
  scroll areas and ending at the previous version's right edge. The gutter
  lies over half the padding either side of the seam (the
  `CONNECTOR_*_REACH` constants), which is where the rectangles stop. The
  bands are placed from the blocks as measured (`noteDiff.boxes`, the
  measurements the rectangles and the markers are placed from) and from how
  far each pane is scrolled (`scrollTops`), and are cut off at the gutter's
  edges: a change scrolled out of one pane leads off the top or the bottom.

  A band is part of its change. Hovering it lights the change in both
  panes, and pressing it selects the change (`onSelect`). It is the one
  thing in the previous version's reach that does: that pane has no
  markers. It is there for the pointer only: the note's markers name the
  changes and take the keyboard, so the drawing is hidden from a screen
  reader and has no Tab stop. Only the bands take the pointer; a press
  between them is the text's.
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

  /** Names this drawing's gradients, which the document looks up by id. */
  const uid = $props.id();

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

  // For as long as the gutter is there, bands or none: a pointer that moves
  // into an empty gutter has to be seen there, or the band a pane later
  // scrolls under it would be compared with where the pointer last was while
  // there were bands, and taken for a hover.
  $effect(() => pointer.watch());

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
      {@const blend = connector.before !== connector.after}
      <g
        class="note-diff-connector"
        class:is-pointed={connector.hunkId === noteDiff.pointedHunkId}
        data-kind={connector.kind}
        data-before={connector.before}
        data-after={connector.after}
        data-hunk-id={connector.hunkId}
      >
        {#if blend}
          <linearGradient
            id="{uid}-{connector.hunkId}"
            gradientUnits="userSpaceOnUse"
            x1="0"
            x2={CONNECTOR_WIDTH}
            y1="0"
            y2="0"
          >
            <stop class="connector-stop-before" offset="0" />
            <stop class="connector-stop-after" offset="1" />
          </linearGradient>
        {/if}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <path
          class="connector-fill"
          style:fill={blend ? `url(#${uid}-${connector.hunkId})` : undefined}
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

  /* A band is painted as the rectangles it joins are (NoteDiffBlocks): at
     each end the tint of the change's blocks in that pane, which is one
     colour across it unless the two ends differ, when it shades from the
     one to the other. The lines along its top and bottom run on from the
     rectangles' own: at rest the hairline the code diff viewer bounds a
     changed range and its connector with (`readThemeColors` in
     connectorRendererCanvas.ts). */
  .connector-fill {
    fill: var(--connector-before);
    cursor: pointer;
    pointer-events: auto;
  }

  .connector-stop-before {
    stop-color: var(--connector-before);
  }

  .connector-stop-after {
    stop-color: var(--connector-after);
  }

  .connector-stroke {
    fill: none;
    stroke: var(--connector-stroke, var(--diff-range-border));
    stroke-width: 1px;
    pointer-events: none;
  }

  .note-diff-connector[data-before='added'] {
    --connector-before: var(--diff-added-bg);
  }

  .note-diff-connector[data-before='removed'] {
    --connector-before: var(--diff-removed-bg);
  }

  .note-diff-connector[data-before='modified'] {
    --connector-before: var(--diff-modified-bg);
  }

  .note-diff-connector[data-after='added'] {
    --connector-after: var(--diff-added-bg);
  }

  .note-diff-connector[data-after='removed'] {
    --connector-after: var(--diff-removed-bg);
  }

  .note-diff-connector[data-after='modified'] {
    --connector-after: var(--diff-modified-bg);
  }

  /* The band of the change being pointed at, in the code diff's colours as
     its blocks and its marker have them (see noteDiffStyles.css): filled
     with the tints its rectangles are lit with, and lined in the solid
     colour of its marker, as they are. */
  .note-diff-connector.is-pointed[data-before='added'] {
    --connector-before: var(--diff-added-bg-emphasis);
  }

  .note-diff-connector.is-pointed[data-before='removed'] {
    --connector-before: var(--diff-removed-bg-emphasis);
  }

  .note-diff-connector.is-pointed[data-before='modified'] {
    --connector-before: var(--diff-modified-bg-emphasis);
  }

  .note-diff-connector.is-pointed[data-after='added'] {
    --connector-after: var(--diff-added-bg-emphasis);
  }

  .note-diff-connector.is-pointed[data-after='removed'] {
    --connector-after: var(--diff-removed-bg-emphasis);
  }

  .note-diff-connector.is-pointed[data-after='modified'] {
    --connector-after: var(--diff-modified-bg-emphasis);
  }

  .note-diff-connector.is-pointed[data-kind='added'] {
    --connector-stroke: var(--status-added);
  }

  .note-diff-connector.is-pointed[data-kind='removed'] {
    --connector-stroke: var(--status-deleted);
  }

  .note-diff-connector.is-pointed[data-kind='modified'] {
    --connector-stroke: var(--diff-modified-accent);
  }
</style>
