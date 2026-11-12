<!--
  BoardScrollEdges.svelte — shows that the board's columns run on past the
  window: a fade over each side with columns beyond it, holding a chevron
  that scrolls the next clipped column into view. A trackpad scrolls sideways
  anyway, but a mouse wheel doesn't, and macOS hides the scrollbar until a
  scroll, so without this a clipped last column reads as the end of the board.

  Overlaid on the scroller's frame, which must be positioned. The fade lets
  clicks and drags through to the columns under it, and the chevron stays in
  view (sticky) as the page scrolls down a long board. Off on the phone
  pager, whose strip of column names already shows there's more.
-->
<script lang="ts">
  import ChevronLeft from '@lucide/svelte/icons/chevron-left';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import { overflowEdges, revealStep, SCROLL_FADE_WIDTH } from './boardScroll';

  interface Props {
    /** The row of columns that scrolls sideways. */
    scroller: HTMLElement | null;
    enabled: boolean;
  }

  let { scroller, enabled }: Props = $props();

  let start = $state(false);
  let end = $state(false);

  $effect(() => {
    const el = scroller;
    if (!el || !enabled) {
      start = false;
      end = false;
      return;
    }
    const measure = () => {
      const edges = overflowEdges(el);
      start = edges.start;
      end = edges.end;
    };
    // The overflow changes without a scroll when the window resizes, a column
    // collapses, expands or fills, or columns come and go.
    const resizes = new ResizeObserver(measure);
    const observeColumns = () => {
      resizes.disconnect();
      resizes.observe(el);
      for (const child of el.children) resizes.observe(child);
    };
    const mutations = new MutationObserver(() => {
      observeColumns();
      measure();
    });
    observeColumns();
    mutations.observe(el, { childList: true });
    el.addEventListener('scroll', measure, { passive: true });
    measure();
    return () => {
      resizes.disconnect();
      mutations.disconnect();
      el.removeEventListener('scroll', measure);
    };
  });

  function reveal(direction: 1 | -1) {
    const el = scroller;
    if (!el) return;
    const box = el.getBoundingClientRect();
    const view = { left: box.left + SCROLL_FADE_WIDTH, right: box.right - SCROLL_FADE_WIDTH };
    const columns = [...el.children].map((child) => child.getBoundingClientRect());
    el.scrollBy({ left: revealStep(direction, view, columns), behavior: 'smooth' });
  }
</script>

{#snippet edge(side: 'start' | 'end')}
  <div class="scroll-edge {side}" style:width="{SCROLL_FADE_WIDTH}px" data-board-scroll-edge={side}>
    <!-- Out of the tab order: keyboard focus moving through the cards
         already scrolls each one into view. -->
    <button
      type="button"
      class="scroll-edge-button"
      tabindex="-1"
      aria-label={side === 'start' ? 'Scroll to earlier columns' : 'Scroll to more columns'}
      title={side === 'start' ? 'Earlier columns' : 'More columns'}
      onclick={() => reveal(side === 'start' ? -1 : 1)}
    >
      {#if side === 'start'}
        <ChevronLeft size={14} />
      {:else}
        <ChevronRight size={14} />
      {/if}
    </button>
  </div>
{/snippet}

{#if start}
  {@render edge('start')}
{/if}
{#if end}
  {@render edge('end')}
{/if}

<style>
  /* Clear of the scroller's bottom padding, where its scrollbar sits. */
  .scroll-edge {
    position: absolute;
    top: 0;
    bottom: 8px;
    z-index: 1;
    display: flex;
    flex-direction: column;
    pointer-events: none;
  }

  .scroll-edge.start {
    left: 0;
    align-items: flex-start;
    background: linear-gradient(to right, var(--bg-chrome) 15%, transparent);
  }

  .scroll-edge.end {
    right: 0;
    align-items: flex-end;
    background: linear-gradient(to left, var(--bg-chrome) 15%, transparent);
  }

  /* Level with the column headers. */
  .scroll-edge-button {
    position: sticky;
    top: 10px;
    margin-top: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    background: var(--bg-elevated);
    color: var(--text-muted);
    cursor: pointer;
    pointer-events: auto;
    transition:
      border-color 0.15s ease,
      color 0.15s ease;
  }

  .scroll-edge-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }
</style>
