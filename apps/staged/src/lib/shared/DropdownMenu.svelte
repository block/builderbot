<!--
  DropdownMenu.svelte - Reusable triple-dot dropdown menu

  Renders an ellipsis button that toggles a positioned dropdown.
  Menu items are passed as a prop array. Handles click-outside and Escape to close.

  Usage:
    <DropdownMenu items={[
      { label: 'Delete', icon: Trash2, danger: true, action: () => handleDelete() },
    ]} />
-->
<script lang="ts">
  import { EllipsisVertical } from 'lucide-svelte';

  type IconComponent = typeof EllipsisVertical;

  export interface MenuItem {
    label: string;
    /** A Svelte component (e.g. a lucide-svelte icon) rendered at size 14. */
    icon?: IconComponent;
    danger?: boolean;
    action: () => void;
  }

  interface Props {
    items: MenuItem[];
    /** Which edge the dropdown aligns to. Default "right" (anchored to right edge). */
    align?: 'left' | 'right';
  }

  let { items, align = 'right' }: Props = $props();

  let open = $state(false);
  let menuRef = $state<HTMLDivElement | null>(null);

  function toggle(e: MouseEvent) {
    e.stopPropagation();
    open = !open;
  }

  function handleItemClick(item: MenuItem, e: MouseEvent) {
    e.stopPropagation();
    open = false;
    item.action();
  }

  function handleClickOutside(e: MouseEvent) {
    if (open && menuRef && !menuRef.contains(e.target as Node)) {
      open = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (open && e.key === 'Escape') {
      open = false;
      e.stopPropagation();
    }
  }
</script>

<svelte:window onclick={handleClickOutside} onkeydown={handleKeydown} />

<div class="menu-container" bind:this={menuRef}>
  <button class="menu-trigger" class:menu-open={open} onclick={toggle} title="More options">
    <EllipsisVertical size={16} />
  </button>
  {#if open}
    <div class="menu-dropdown" class:align-left={align === 'left'}>
      {#each items as item, i}
        <button
          class="menu-item"
          class:danger={item.danger}
          onclick={(e) => handleItemClick(item, e)}
        >
          {#if item.icon}
            <item.icon size={14} />
          {/if}
          {item.label}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .menu-container {
    position: relative;
  }

  .menu-trigger {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .menu-trigger:hover,
  .menu-trigger.menu-open {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .menu-dropdown {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow:
      0 4px 12px rgba(0, 0, 0, 0.12),
      0 1px 4px rgba(0, 0, 0, 0.08);
    overflow: hidden;
    z-index: 100;
    min-width: 140px;
    padding: 4px 0;
  }

  .menu-dropdown.align-left {
    right: auto;
    left: 0;
  }

  .menu-item {
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

  .menu-item:hover {
    background-color: var(--bg-hover);
  }

  .menu-item :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .menu-item.danger:hover {
    background-color: var(--ui-danger-bg);
    color: var(--ui-danger);
  }

  .menu-item.danger:hover :global(svg) {
    color: var(--ui-danger);
  }
</style>
