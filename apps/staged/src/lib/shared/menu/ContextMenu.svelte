<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import MenuSurface from './MenuSurface.svelte';
  import type { MenuItem } from './types';
  import {
    closeAllMenus,
    registerMenuCloseListener,
    unregisterMenuCloseListener,
  } from './coordination';

  type OpenOptions = {
    x: number;
    y: number;
    items: MenuItem[];
    ariaLabel?: string;
  };

  interface Props {
    ariaLabel?: string;
    minWidth?: number;
    onClose?: () => void;
  }

  let { ariaLabel = 'Context menu', minWidth = 160, onClose = () => {} }: Props = $props();

  let menu = $state<(OpenOptions & { left: number; top: number }) | null>(null);
  let positioned = $state(false);
  let surfaceRef = $state<ReturnType<typeof MenuSurface> | undefined>();
  let placementToken = 0;
  const viewportPadding = 8;

  function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(value, max));
  }

  async function placeMenu(token: number) {
    if (!menu) return;
    positioned = false;
    menu = { ...menu, left: menu.x, top: menu.y };
    await tick();

    if (token !== placementToken || !menu) return;
    const rect = surfaceRef?.getRect();
    if (!rect) return;

    menu = {
      ...menu,
      left: clamp(menu.x, viewportPadding, window.innerWidth - rect.width - viewportPadding),
      top: clamp(menu.y, viewportPadding, window.innerHeight - rect.height - viewportPadding),
    };
    positioned = true;
    await tick();

    if (token === placementToken) {
      surfaceRef?.focusFirstItem();
    }
  }

  function closeFromCoordinator() {
    close();
  }

  function handlePointerDown(event: PointerEvent) {
    if (!menu) return;
    const target = event.target as Node;
    if (surfaceRef?.contains(target)) return;
    close();
  }

  function handleContextMenu(event: MouseEvent) {
    if (!menu) return;
    const target = event.target as Node;
    if (surfaceRef?.contains(target)) return;
    close();
  }

  function handleKeydown(event: KeyboardEvent) {
    if (!menu || event.key !== 'Escape') return;
    event.preventDefault();
    close();
  }

  function closeForViewportChange() {
    if (menu) close();
  }

  export function open(options: OpenOptions): void {
    closeAllMenus(closeFromCoordinator);
    menu = {
      ...options,
      left: options.x,
      top: options.y,
    };
    const token = ++placementToken;
    void placeMenu(token);
  }

  export function close(): void {
    if (!menu) return;
    menu = null;
    positioned = false;
    onClose();
  }

  onMount(() => {
    registerMenuCloseListener(closeFromCoordinator);
    window.addEventListener('pointerdown', handlePointerDown, true);
    window.addEventListener('contextmenu', handleContextMenu, true);
    window.addEventListener('keydown', handleKeydown, true);
    window.addEventListener('scroll', closeForViewportChange, true);
    window.addEventListener('resize', closeForViewportChange);

    return () => {
      unregisterMenuCloseListener(closeFromCoordinator);
      window.removeEventListener('pointerdown', handlePointerDown, true);
      window.removeEventListener('contextmenu', handleContextMenu, true);
      window.removeEventListener('keydown', handleKeydown, true);
      window.removeEventListener('scroll', closeForViewportChange, true);
      window.removeEventListener('resize', closeForViewportChange);
    };
  });

  onDestroy(() => {
    placementToken++;
  });
</script>

{#if menu}
  <MenuSurface
    bind:this={surfaceRef}
    items={menu.items}
    left={menu.left}
    top={menu.top}
    ariaLabel={menu.ariaLabel ?? ariaLabel}
    {minWidth}
    visible={positioned}
    onClose={close}
  />
{/if}
