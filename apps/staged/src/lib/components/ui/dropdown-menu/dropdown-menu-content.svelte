<script lang="ts">
  import { cn, type WithoutChildrenOrChild } from '$lib/components/utils.js';
  import DropdownMenuPortal from './dropdown-menu-portal.svelte';
  import MenuSheet from '$lib/components/ui/menu-sheet/menu-sheet.svelte';
  import { viewport, watchViewport } from '$lib/shared/viewport.svelte';
  import { DropdownMenu as DropdownMenuPrimitive } from 'bits-ui';
  import { onMount } from 'svelte';
  import type { ComponentProps } from 'svelte';

  let {
    ref = $bindable(null),
    sideOffset = 4,
    align = 'start',
    portalProps,
    class: className,
    children,
    // Floating-only props, meaningless for the sheet's static content.
    side,
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
  }: DropdownMenuPrimitive.ContentProps & {
    portalProps?: WithoutChildrenOrChild<ComponentProps<typeof DropdownMenuPortal>>;
  } = $props();

  // Keep `viewport.sheetMenus` live even when the host screen never subscribed.
  onMount(() => watchViewport());
</script>

<DropdownMenuPortal {...portalProps}>
  {#if viewport.sheetMenus}
    <DropdownMenuPrimitive.ContentStatic bind:ref data-slot="dropdown-menu-content" {...restProps}>
      {#snippet child({ props })}
        <MenuSheet {props} class={className}>
          {@render children?.()}
          {#snippet cancel()}
            <DropdownMenuPrimitive.Item
              data-slot="dropdown-menu-item"
              class="menu-sheet-cancel focus:bg-accent focus:text-accent-foreground relative flex cursor-default items-center outline-hidden select-none"
            >
              Cancel
            </DropdownMenuPrimitive.Item>
          {/snippet}
        </MenuSheet>
      {/snippet}
    </DropdownMenuPrimitive.ContentStatic>
  {:else}
    <DropdownMenuPrimitive.Content
      bind:ref
      data-slot="dropdown-menu-content"
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
        'data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-32 rounded-md p-1 shadow-md ring-1 duration-100 data-[side=inline-start]:slide-in-from-right-2 data-[side=inline-end]:slide-in-from-left-2 z-(--z-index-floating) w-auto overflow-x-hidden overflow-y-auto outline-none data-closed:overflow-hidden',
        className
      )}
      {...restProps}
    >
      {@render children?.()}
    </DropdownMenuPrimitive.Content>
  {/if}
</DropdownMenuPortal>
