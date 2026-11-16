<!--
  BoardDragGhost.svelte — the card in hand while it is dragged across the
  board: a copy of the card that follows the pointer, lifted off the page.
  The card itself stays in its column, dimmed, until the drop.

  A copy of the card's DOM rather than a second BoardCard, so it is exactly
  what was picked up. Portaled out of the board, so `position: fixed` is the
  viewport's and no scroller clips it; it takes no pointer events, so what
  the pointer is over is always the board underneath.
-->
<script lang="ts">
  import { portal } from '../../shared/portal';

  interface Props {
    /** The card being dragged. */
    source: HTMLElement;
    width: number;
    /** The ghost's top-left corner, in viewport px. */
    x: number;
    y: number;
    /** Over something that won't take the card. */
    refused: boolean;
  }

  let { source, width, x, y, refused }: Props = $props();

  function copyOf(node: HTMLElement) {
    const copy = source.cloneNode(true) as HTMLElement;
    // The copy is in hand, not left behind, and isn't a card to find or focus.
    copy.classList.remove('dragging');
    copy.removeAttribute('data-board-card');
    copy.tabIndex = -1;
    node.appendChild(copy);
    return portal(node);
  }
</script>

<div
  class="board-drag-ghost"
  class:refused
  aria-hidden="true"
  inert
  style:width="{width}px"
  style:transform="translate3d({x}px, {y}px, 0)"
  use:copyOf
></div>

<style>
  .board-drag-ghost {
    position: fixed;
    top: 0;
    left: 0;
    z-index: var(--z-index-floating);
    display: flex;
    flex-direction: column;
    pointer-events: none;
    transition: opacity 0.1s ease;
  }

  .board-drag-ghost > :global(*) {
    box-shadow: var(--shadow-elevated);
    transform: rotate(2deg);
  }

  .board-drag-ghost.refused {
    opacity: 0.55;
  }
</style>
