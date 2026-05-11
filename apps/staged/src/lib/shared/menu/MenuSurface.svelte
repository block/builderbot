<script lang="ts">
  import { tick } from 'svelte';
  import { ChevronRight } from 'lucide-svelte';
  import { selectMenuAction } from './actions';
  import type { MenuActionItem, MenuItem, MenuSubmenuItem } from './types';

  type SubmenuPlacement = 'right' | 'left';

  interface Props {
    items: MenuItem[];
    left: number;
    top: number;
    ariaLabel?: string;
    minWidth?: number;
    visible?: boolean;
    zIndex?: number;
    onClose: () => void;
  }

  let {
    items,
    left,
    top,
    ariaLabel = 'Menu',
    minWidth = 160,
    visible = true,
    zIndex = 1100,
    onClose,
  }: Props = $props();

  let menuEl = $state<HTMLDivElement | null>(null);
  let openSubmenuPath = $state<string | null>(null);
  let submenuPlacements = $state<Record<string, SubmenuPlacement>>({});
  let closeSubmenuTimer: ReturnType<typeof setTimeout> | null = null;

  const submenuContainers = new Map<string, HTMLElement>();
  const submenuElements = new Map<string, HTMLElement>();
  const viewportPadding = 8;
  const submenuGap = 2;

  function isAction(item: MenuItem): item is MenuActionItem {
    return item.type === 'action';
  }

  function isSubmenu(item: MenuItem): item is MenuSubmenuItem {
    return item.type === 'submenu';
  }

  function pathFor(parentPath: string, index: number): string {
    return parentPath ? `${parentPath}.${index}` : `${index}`;
  }

  function parentPathFor(path: string): string {
    const lastSeparator = path.lastIndexOf('.');
    return lastSeparator === -1 ? '' : path.slice(0, lastSeparator);
  }

  function isSubmenuOpen(path: string): boolean {
    return openSubmenuPath === path || !!openSubmenuPath?.startsWith(`${path}.`);
  }

  function clearSubmenuTimer() {
    if (closeSubmenuTimer) {
      clearTimeout(closeSubmenuTimer);
      closeSubmenuTimer = null;
    }
  }

  async function openSubmenu(path: string) {
    clearSubmenuTimer();
    openSubmenuPath = path;
    await updateSubmenuPlacement(path);
  }

  function closeSubmenu(path: string) {
    if (!openSubmenuPath) return;
    if (openSubmenuPath === path || openSubmenuPath.startsWith(`${path}.`)) {
      const parentPath = parentPathFor(path);
      openSubmenuPath = parentPath || null;
    }
  }

  function closeSubmenuSoon(path: string) {
    clearSubmenuTimer();
    closeSubmenuTimer = setTimeout(() => {
      closeSubmenu(path);
      closeSubmenuTimer = null;
    }, 120);
  }

  function closeChildSubmenus(scope: string) {
    if (!openSubmenuPath) return;
    if (!scope) {
      openSubmenuPath = null;
      return;
    }
    if (openSubmenuPath.startsWith(`${scope}.`)) {
      openSubmenuPath = scope;
    }
  }

  async function updateSubmenuPlacement(path: string) {
    await tick();
    const container = submenuContainers.get(path);
    const submenu = submenuElements.get(path);
    if (!container || !submenu) return;

    const containerRect = container.getBoundingClientRect();
    const submenuRect = submenu.getBoundingClientRect();
    const wouldOverflowRight =
      containerRect.right + submenuGap + submenuRect.width > window.innerWidth - viewportPadding;

    submenuPlacements = {
      ...submenuPlacements,
      [path]: wouldOverflowRight ? 'left' : 'right',
    };
  }

  function trackSubmenuContainer(node: HTMLElement, path: string) {
    submenuContainers.set(path, node);
    return {
      update(nextPath: string) {
        submenuContainers.delete(path);
        path = nextPath;
        submenuContainers.set(path, node);
      },
      destroy() {
        submenuContainers.delete(path);
      },
    };
  }

  function trackSubmenu(node: HTMLElement, path: string) {
    submenuElements.set(path, node);
    void updateSubmenuPlacement(path);
    return {
      update(nextPath: string) {
        submenuElements.delete(path);
        path = nextPath;
        submenuElements.set(path, node);
        void updateSubmenuPlacement(path);
      },
      destroy() {
        submenuElements.delete(path);
      },
    };
  }

  function getFocusableItems(scope: string): HTMLButtonElement[] {
    if (!menuEl) return [];
    return Array.from(menuEl.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')).filter(
      (item) => item.dataset.menuScope === scope && !item.disabled
    );
  }

  function focusItem(scope: string, index: number) {
    const items = getFocusableItems(scope);
    if (items.length === 0) return;
    const clamped = Math.max(0, Math.min(index, items.length - 1));
    items[clamped].focus();
  }

  function focusButtonByPath(path: string) {
    if (!menuEl) return;
    const button = Array.from(menuEl.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')).find(
      (item) => item.dataset.menuPath === path
    );
    button?.focus();
  }

  function currentItemIndex(items: HTMLButtonElement[], target: EventTarget | null): number {
    return items.indexOf(target as HTMLButtonElement);
  }

  async function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }

    const target = event.target as HTMLButtonElement | null;
    const scope = target?.dataset.menuScope ?? '';
    const path = target?.dataset.menuPath ?? '';
    const itemsInScope = getFocusableItems(scope);
    const index = currentItemIndex(itemsInScope, target);

    switch (event.key) {
      case 'ArrowDown':
        if (itemsInScope.length === 0) return;
        event.preventDefault();
        focusItem(scope, index < 0 ? 0 : (index + 1) % itemsInScope.length);
        break;
      case 'ArrowUp':
        if (itemsInScope.length === 0) return;
        event.preventDefault();
        focusItem(
          scope,
          index < 0
            ? itemsInScope.length - 1
            : (index - 1 + itemsInScope.length) % itemsInScope.length
        );
        break;
      case 'Home':
        if (itemsInScope.length === 0) return;
        event.preventDefault();
        focusItem(scope, 0);
        break;
      case 'End':
        if (itemsInScope.length === 0) return;
        event.preventDefault();
        focusItem(scope, itemsInScope.length - 1);
        break;
      case 'ArrowRight':
        if (target?.dataset.menuKind === 'submenu' && path) {
          event.preventDefault();
          await openSubmenu(path);
          await tick();
          focusItem(path, 0);
        }
        break;
      case 'ArrowLeft':
        if (scope) {
          event.preventDefault();
          closeSubmenu(scope);
          focusButtonByPath(scope);
        }
        break;
      case 'Tab':
        event.preventDefault();
        onClose();
        break;
    }
  }

  function handleAction(item: MenuActionItem) {
    selectMenuAction(item, onClose);
  }

  export function getRect(): DOMRect | null {
    return menuEl?.getBoundingClientRect() ?? null;
  }

  export function contains(node: Node): boolean {
    return menuEl?.contains(node) ?? false;
  }

  export function focusFirstItem(): void {
    focusItem('', 0);
  }
</script>

{#snippet renderMenuItems(itemsToRender: MenuItem[], scope: string)}
  {#each itemsToRender as item, index}
    {@const path = pathFor(scope, index)}
    {#if item.type === 'separator'}
      <div class="menu-separator" role="separator"></div>
    {:else if isAction(item)}
      <button
        type="button"
        class="menu-item"
        class:danger={item.danger}
        role="menuitem"
        tabindex="-1"
        disabled={item.disabled}
        data-menu-scope={scope}
        data-menu-path={path}
        data-menu-kind="action"
        onmouseenter={() => closeChildSubmenus(scope)}
        onclick={(event) => {
          event.stopPropagation();
          handleAction(item);
        }}
      >
        {#if item.iconSrc}
          <img
            src={item.iconSrc}
            alt=""
            width="14"
            height="14"
            draggable="false"
            class="menu-item-icon"
          />
        {:else if item.icon}
          <item.icon size={14} />
        {/if}
        <span class="menu-item-label">{item.label}</span>
      </button>
    {:else if isSubmenu(item)}
      <div
        class="submenu-container"
        role="none"
        use:trackSubmenuContainer={path}
        onmouseenter={() => {
          if (!item.disabled && item.children.length > 0) {
            openSubmenu(path);
          }
        }}
        onmouseleave={() => closeSubmenuSoon(path)}
      >
        <button
          type="button"
          class="menu-item submenu-trigger"
          role="menuitem"
          tabindex="-1"
          disabled={item.disabled || item.children.length === 0}
          aria-haspopup="menu"
          aria-expanded={isSubmenuOpen(path)}
          data-menu-scope={scope}
          data-menu-path={path}
          data-menu-kind="submenu"
          onclick={(event) => {
            event.stopPropagation();
            void openSubmenu(path);
          }}
        >
          {#if item.icon}
            <item.icon size={14} />
          {/if}
          <span class="menu-item-label">{item.label}</span>
          <ChevronRight size={13} class="submenu-chevron" />
        </button>
        {#if isSubmenuOpen(path)}
          <div
            class="submenu"
            class:open-left={submenuPlacements[path] === 'left'}
            role="menu"
            aria-label={item.label}
            use:trackSubmenu={path}
          >
            {@render renderMenuItems(item.children, path)}
          </div>
        {/if}
      </div>
    {/if}
  {/each}
{/snippet}

<div
  class="menu-surface"
  role="menu"
  tabindex="-1"
  aria-label={ariaLabel}
  style:left={`${left}px`}
  style:top={`${top}px`}
  style:min-width={`${minWidth}px`}
  style:z-index={String(zIndex)}
  style:opacity={visible ? '1' : '0'}
  bind:this={menuEl}
  onkeydown={handleKeydown}
  onclick={(event) => event.stopPropagation()}
  oncontextmenu={(event) => event.stopPropagation()}
>
  {@render renderMenuItems(items, '')}
</div>

<style>
  .menu-surface,
  .submenu {
    position: fixed;
    padding: 4px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: var(--bg-primary);
    box-shadow: var(--shadow-elevated);
    color: var(--text-primary);
  }

  .menu-surface {
    max-width: min(320px, calc(100vw - 16px));
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
    font-family: inherit;
    font-size: var(--size-sm);
    font-weight: 500;
    line-height: 1.3;
    text-align: left;
    cursor: pointer;
    white-space: nowrap;
  }

  .menu-item:hover,
  .menu-item:focus-visible {
    background: var(--bg-hover);
    outline: none;
  }

  .menu-item:disabled {
    opacity: 0.42;
    cursor: not-allowed;
  }

  .menu-item:disabled:hover,
  .menu-item:disabled:focus-visible {
    background: transparent;
  }

  .menu-item :global(svg) {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .menu-item-icon {
    flex-shrink: 0;
    border-radius: 3px;
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

  .menu-item-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .menu-separator {
    height: 1px;
    margin: 4px 0;
    background: var(--border-muted);
  }

  .submenu-container {
    position: relative;
  }

  .submenu-trigger {
    padding-right: 8px;
  }

  .menu-item :global(.submenu-chevron) {
    margin-left: auto;
  }

  .submenu {
    position: absolute;
    top: 0;
    left: calc(100% + 2px);
    z-index: 1;
    min-width: 160px;
    max-width: min(320px, calc(100vw - 16px));
    max-height: min(400px, calc(100vh - 16px));
    overflow-y: auto;
  }

  .submenu.open-left {
    right: calc(100% + 2px);
    left: auto;
  }
</style>
