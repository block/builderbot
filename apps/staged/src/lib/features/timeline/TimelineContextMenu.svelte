<!--
  TimelineContextMenu.svelte — Shared context menu for timeline rows.

  Rendered once per timeline (not per row) to avoid N window listeners.
  The parent opens it by calling `open(event)` and provides an
  `onNewSessionReferring` callback for the hashtag action.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Copy, MessageSquarePlus } from 'lucide-svelte';
  import type { ContextMenuEvent } from './TimelineRow.svelte';
  import {
    registerCloseListener,
    unregisterCloseListener,
    broadcastCloseAll,
  } from './contextMenuCoordination';

  interface Props {
    onNewSessionReferring?: (hashtagRef: string) => void;
  }

  let { onNewSessionReferring }: Props = $props();

  /** Menu position and data, or null when closed. */
  let menu = $state<(ContextMenuEvent & { left: number; top: number }) | null>(null);
  let menuEl = $state<HTMLDivElement | null>(null);
  let pendingRaf: number | null = null;

  // Register this instance's close handler in the module-level set.
  function handleCloseAll() {
    menu = null;
  }

  onMount(() => {
    registerCloseListener(handleCloseAll);
  });

  onDestroy(() => {
    unregisterCloseListener(handleCloseAll);
    if (pendingRaf !== null) {
      cancelAnimationFrame(pendingRaf);
      pendingRaf = null;
    }
  });

  /** Open the context menu. Called by the parent when a row emits onContextMenu. */
  export function open(event: ContextMenuEvent) {
    // Close any other open context menu instance (cross-timeline).
    broadcastCloseAll();

    // Cancel any in-flight rAF from a previous open() to avoid stale coordinates clobbering.
    if (pendingRaf !== null) {
      cancelAnimationFrame(pendingRaf);
      pendingRaf = null;
    }

    // Clamp to viewport after a microtask so the menu element is rendered and measurable.
    // Use raw position initially; clamp in the next frame.
    menu = { ...event, left: event.x, top: event.y };

    // Defer clamping until the element is rendered and measurable
    pendingRaf = requestAnimationFrame(() => {
      pendingRaf = null;
      if (!menuEl || !menu) return;
      const rect = menuEl.getBoundingClientRect();
      const clampedLeft = Math.min(event.x, window.innerWidth - rect.width - 8);
      const clampedTop = Math.min(event.y, window.innerHeight - rect.height - 8);
      menu = { ...menu!, left: Math.max(8, clampedLeft), top: Math.max(8, clampedTop) };
    });
  }

  /** Close the context menu. */
  export function close() {
    menu = null;
  }

  function handleCopySha() {
    if (menu?.commitSha) {
      navigator.clipboard.writeText(menu.commitSha).catch(() => {});
    }
    menu = null;
  }

  function handleNewSessionReferring() {
    if (menu?.hashtagRef && onNewSessionReferring) {
      onNewSessionReferring(menu.hashtagRef);
    }
    menu = null;
  }

  function handleWindowClick() {
    if (!menu) return;
    menu = null;
  }

  function handleWindowContextMenu() {
    if (!menu) return;
    // Close when user right-clicks elsewhere (another row or outside)
    menu = null;
  }

  function handleWindowKeydown(e: KeyboardEvent) {
    if (!menu) return;
    if (e.key === 'Escape') {
      menu = null;
      e.stopPropagation();
    }
  }
</script>

<svelte:window
  onclick={handleWindowClick}
  oncontextmenu={handleWindowContextMenu}
  onkeydown={handleWindowKeydown}
/>

{#if menu}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    bind:this={menuEl}
    class="context-menu"
    style="left: {menu.left}px; top: {menu.top}px;"
    onclick={(e) => e.stopPropagation()}
    oncontextmenu={(e) => e.stopPropagation()}
  >
    {#if menu.commitSha}
      <button class="context-menu-item" onclick={handleCopySha}>
        <Copy size={14} />
        Copy SHA
      </button>
    {/if}
    {#if menu.hashtagRef && onNewSessionReferring}
      <button class="context-menu-item" onclick={handleNewSessionReferring}>
        <MessageSquarePlus size={14} />
        New session referring to this
      </button>
    {/if}
  </div>
{/if}

<style>
  .context-menu {
    position: fixed;
    z-index: 1000;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow:
      0 4px 12px rgba(0, 0, 0, 0.12),
      0 1px 4px rgba(0, 0, 0, 0.08);
    overflow: hidden;
    min-width: 140px;
    padding: 4px 0;
  }

  .context-menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 14px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
    text-align: left;
    white-space: nowrap;
  }

  .context-menu-item:hover {
    background-color: var(--bg-hover);
  }

  .context-menu-item :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }
</style>
