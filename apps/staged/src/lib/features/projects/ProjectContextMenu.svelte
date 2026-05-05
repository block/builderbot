<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { Mail, Trash2 } from 'lucide-svelte';

  interface Props {
    x: number;
    y: number;
    onMarkAsUnread: () => void;
    onRemoveProject: () => void;
    onClose: () => void;
  }

  let { x, y, onMarkAsUnread, onRemoveProject, onClose }: Props = $props();

  let menuEl = $state<HTMLDivElement | null>(null);
  let left = $state(0);
  let top = $state(0);
  let positioned = $state(false);
  const viewportPadding = 8;

  async function placeMenu() {
    positioned = false;
    left = x;
    top = y;
    await tick();
    if (!menuEl) return;

    const rect = menuEl.getBoundingClientRect();
    left = Math.max(viewportPadding, Math.min(x, window.innerWidth - rect.width - viewportPadding));
    top = Math.max(
      viewportPadding,
      Math.min(y, window.innerHeight - rect.height - viewportPadding)
    );
    positioned = true;
    await tick();
    focusItem(0);
  }

  function handlePointerDown(event: PointerEvent) {
    if (menuEl?.contains(event.target as Node)) return;
    onClose();
  }

  function getMenuItems(): HTMLButtonElement[] {
    if (!menuEl) return [];
    return Array.from(menuEl.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
  }

  function focusItem(index: number) {
    const items = getMenuItems();
    if (items.length === 0) return;
    const clamped = Math.max(0, Math.min(index, items.length - 1));
    items[clamped].focus();
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      return;
    }

    const items = getMenuItems();
    if (items.length === 0) return;
    const active = document.activeElement as HTMLElement;
    const currentIndex = items.indexOf(active as HTMLButtonElement);

    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        focusItem(currentIndex < 0 ? 0 : (currentIndex + 1) % items.length);
        break;
      case 'ArrowUp':
        event.preventDefault();
        focusItem(
          currentIndex < 0 ? items.length - 1 : (currentIndex - 1 + items.length) % items.length
        );
        break;
      case 'Home':
        event.preventDefault();
        focusItem(0);
        break;
      case 'End':
        event.preventDefault();
        focusItem(items.length - 1);
        break;
      case 'Tab':
        // Trap focus within menu; close instead of tabbing out
        event.preventDefault();
        onClose();
        break;
    }
  }

  function handleMarkAsUnread() {
    onMarkAsUnread();
    onClose();
  }

  function handleRemoveProject() {
    onRemoveProject();
    onClose();
  }

  onMount(() => {
    void placeMenu();
    window.addEventListener('pointerdown', handlePointerDown, true);
    window.addEventListener('keydown', handleKeydown, true);
    window.addEventListener('scroll', onClose, true);
    window.addEventListener('resize', onClose);

    return () => {
      window.removeEventListener('pointerdown', handlePointerDown, true);
      window.removeEventListener('keydown', handleKeydown, true);
      window.removeEventListener('scroll', onClose, true);
      window.removeEventListener('resize', onClose);
    };
  });

  $effect(() => {
    x;
    y;
    void placeMenu();
  });
</script>

<div
  class="project-context-menu"
  role="menu"
  aria-label="Project actions"
  style:left={`${left}px`}
  style:top={`${top}px`}
  style:opacity={positioned ? '1' : '0'}
  bind:this={menuEl}
>
  <button
    type="button"
    class="menu-item"
    role="menuitem"
    tabindex="-1"
    onclick={handleMarkAsUnread}
  >
    <Mail size={14} />
    Mark as Unread
  </button>
  <button
    type="button"
    class="menu-item danger"
    role="menuitem"
    tabindex="-1"
    onclick={handleRemoveProject}
  >
    <Trash2 size={14} />
    Remove Project
  </button>
</div>

<style>
  .project-context-menu {
    position: fixed;
    z-index: 1100;
    min-width: 172px;
    padding: 4px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: var(--bg-elevated);
    box-shadow: var(--shadow-elevated);
  }

  .menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-height: 30px;
    padding: 6px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 500;
    text-align: left;
    cursor: pointer;
    white-space: nowrap;
  }

  .menu-item:hover,
  .menu-item:focus-visible {
    background: var(--bg-hover);
    outline: none;
  }

  .menu-item :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .menu-item.danger {
    color: var(--ui-danger);
  }

  .menu-item.danger :global(svg) {
    color: var(--ui-danger);
  }

  .menu-item.danger:hover,
  .menu-item.danger:focus-visible {
    background: var(--ui-danger-bg);
  }
</style>
