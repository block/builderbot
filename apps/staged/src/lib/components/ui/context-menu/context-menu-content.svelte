<script lang="ts">
  import { ContextMenu as ContextMenuPrimitive } from 'bits-ui';
  import { onMount } from 'svelte';
  import { cn } from '$lib/components/utils.js';
  import ContextMenuPortal from './context-menu-portal.svelte';
  import MenuSheet from '$lib/components/ui/menu-sheet/menu-sheet.svelte';
  import { getMenuRoot } from '$lib/components/ui/menu-sheet/menu-close-context.js';
  import { viewport, watchViewport } from '$lib/shared/viewport.svelte';
  import type { ComponentProps } from 'svelte';
  import type { WithoutChildrenOrChild } from '$lib/components/utils.js';

  let {
    ref = $bindable(null),
    portalProps,
    class: className,
    children,
    // Floating-only props, meaningless for the sheet's static content.
    side,
    sideOffset,
    align,
    alignOffset,
    arrowPadding,
    avoidCollisions,
    collisionBoundary,
    collisionPadding,
    customAnchor,
    hideWhenDetached,
    sticky,
    strategy,
    updatePositionStrategy,
    ...restProps
  }: ContextMenuPrimitive.ContentProps & {
    portalProps?: WithoutChildrenOrChild<ComponentProps<typeof ContextMenuPortal>>;
  } = $props();

  // Keep `viewport.sheetMenus` live even when the host screen never subscribed.
  onMount(() => watchViewport());

  const root = getMenuRoot();
</script>

<ContextMenuPortal {...portalProps}>
  {#if viewport.sheetMenus}
    <ContextMenuPrimitive.ContentStatic bind:ref data-slot="context-menu-content" {...restProps}>
      {#snippet child({ props })}
        <MenuSheet {props} class={className} onDismiss={() => root?.close()}>
          {@render children?.()}
          {#snippet cancel()}
            <ContextMenuPrimitive.Item
              data-slot="context-menu-item"
              class="menu-sheet-cancel focus:bg-accent focus:text-accent-foreground relative flex cursor-default items-center outline-hidden select-none"
            >
              Cancel
            </ContextMenuPrimitive.Item>
          {/snippet}
        </MenuSheet>
      {/snippet}
    </ContextMenuPrimitive.ContentStatic>
  {:else}
    <ContextMenuPrimitive.Content
      bind:ref
      data-slot="context-menu-content"
      {side}
      {sideOffset}
      {align}
      {alignOffset}
      {arrowPadding}
      {avoidCollisions}
      {collisionBoundary}
      {collisionPadding}
      {customAnchor}
      {hideWhenDetached}
      {sticky}
      {strategy}
      {updatePositionStrategy}
      class={cn(
        'data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-36 rounded-md p-1 shadow-md ring-1 duration-100 z-(--z-index-floating) overflow-x-hidden overflow-y-auto outline-none',
        className
      )}
      {...restProps}
    >
      {@render children?.()}
    </ContextMenuPrimitive.Content>
  {/if}
</ContextMenuPortal>
