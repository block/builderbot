<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { MoreVertical } from 'lucide-svelte';
  import MenuSurface from './MenuSurface.svelte';
  import type { MenuItem } from './types';
  import {
    closeAllMenus,
    registerMenuCloseListener,
    unregisterMenuCloseListener,
  } from './coordination';

  interface Props {
    items: MenuItem[];
    ariaLabel?: string;
    title?: string;
    align?: 'left' | 'right';
    disabled?: boolean;
    minWidth?: number;
  }

  let {
    items,
    ariaLabel = 'More actions',
    title = 'More options',
    align = 'right',
    disabled = false,
    minWidth = 160,
  }: Props = $props();

  let open = $state(false);
  let positioned = $state(false);
  let triggerEl = $state<HTMLButtonElement | null>(null);
  let surfaceRef = $state<ReturnType<typeof MenuSurface> | undefined>();
  let left = $state(0);
  let top = $state(0);
  let placementToken = 0;
  const viewportPadding = 8;
  const menuGap = 4;

  function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(value, max));
  }

  function closeFromCoordinator() {
    close();
  }

  async function placeMenu(token: number, focusAfterPlacement = false) {
    if (!triggerEl || !open) return;
    positioned = false;

    const triggerRect = triggerEl.getBoundingClientRect();
    left = align === 'left' ? triggerRect.left : triggerRect.right - minWidth;
    top = triggerRect.bottom + menuGap;
    await tick();

    if (token !== placementToken || !open) return;
    const menuRect = surfaceRef?.getRect();
    if (!menuRect) return;

    const preferredLeft = align === 'left' ? triggerRect.left : triggerRect.right - menuRect.width;
    const preferredTop =
      triggerRect.bottom + menuGap + menuRect.height > window.innerHeight - viewportPadding &&
      triggerRect.top - menuGap - menuRect.height >= viewportPadding
        ? triggerRect.top - menuGap - menuRect.height
        : triggerRect.bottom + menuGap;

    left = clamp(
      preferredLeft,
      viewportPadding,
      window.innerWidth - menuRect.width - viewportPadding
    );
    top = clamp(
      preferredTop,
      viewportPadding,
      window.innerHeight - menuRect.height - viewportPadding
    );
    positioned = true;
    await tick();

    if (focusAfterPlacement && token === placementToken) {
      surfaceRef?.focusFirstItem();
    }
  }

  function toggle(event: MouseEvent) {
    event.stopPropagation();
    if (disabled) return;

    if (open) {
      close();
      return;
    }

    closeAllMenus(closeFromCoordinator);
    open = true;
    const token = ++placementToken;
    void placeMenu(token, true);
  }

  function close() {
    if (!open) return;
    open = false;
    positioned = false;
  }

  function handlePointerDown(event: PointerEvent) {
    if (!open) return;
    const target = event.target as Node;
    if (triggerEl?.contains(target) || surfaceRef?.contains(target)) return;
    close();
  }

  function handleKeydown(event: KeyboardEvent) {
    if (!open || event.key !== 'Escape') return;
    event.preventDefault();
    close();
    triggerEl?.focus();
  }

  function closeForViewportChange() {
    if (open) close();
  }

  onMount(() => {
    registerMenuCloseListener(closeFromCoordinator);
    window.addEventListener('pointerdown', handlePointerDown, true);
    window.addEventListener('keydown', handleKeydown, true);
    window.addEventListener('scroll', closeForViewportChange, true);
    window.addEventListener('resize', closeForViewportChange);

    return () => {
      unregisterMenuCloseListener(closeFromCoordinator);
      window.removeEventListener('pointerdown', handlePointerDown, true);
      window.removeEventListener('keydown', handleKeydown, true);
      window.removeEventListener('scroll', closeForViewportChange, true);
      window.removeEventListener('resize', closeForViewportChange);
    };
  });

  $effect(() => {
    items;
    if (open) {
      const token = ++placementToken;
      void placeMenu(token);
    }
  });
</script>

<div class="more-menu-root">
  <button
    type="button"
    class="more-menu-trigger"
    class:open
    {disabled}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={title}
    {title}
    onclick={toggle}
    bind:this={triggerEl}
  >
    <MoreVertical size={16} />
  </button>

  {#if open}
    <MenuSurface
      bind:this={surfaceRef}
      {items}
      {left}
      {top}
      {ariaLabel}
      {minWidth}
      visible={positioned}
      onClose={close}
    />
  {/if}
</div>

<style>
  .more-menu-root {
    display: inline-flex;
    align-items: center;
  }

  .more-menu-trigger {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .more-menu-trigger:hover,
  .more-menu-trigger.open,
  .more-menu-trigger:focus-visible {
    background: var(--bg-hover);
    color: var(--text-primary);
    outline: none;
  }

  .more-menu-trigger:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
</style>
