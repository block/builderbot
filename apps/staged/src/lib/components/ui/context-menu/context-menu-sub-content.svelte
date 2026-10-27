<script lang="ts">
  import { ContextMenu as ContextMenuPrimitive } from 'bits-ui';
  import { cn } from '$lib/components/utils.js';
  import MenuSheet from '$lib/components/ui/menu-sheet/menu-sheet.svelte';
  import { getMenuSub } from '$lib/components/ui/menu-sheet/menu-sub-context.js';
  import { viewport } from '$lib/shared/viewport.svelte';

  let {
    ref = $bindable(null),
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
  }: ContextMenuPrimitive.SubContentProps = $props();

  const sub = getMenuSub();
</script>

{#if viewport.sheetMenus}
  <!-- A second sheet layered over the parent's. Portalled so it escapes the
       parent panel (scrolling, transformed while animating) and stacks above
       it; bits-ui keeps the sub's own layer on top, so a tap on this sheet's
       scrim closes only the sub-menu, as does the Back row. -->
  <ContextMenuPrimitive.Portal>
    <ContextMenuPrimitive.SubContentStatic
      bind:ref
      data-slot="context-menu-sub-content"
      {...restProps}
    >
      {#snippet child({ props })}
        <MenuSheet {props} class={className}>
          {@render children?.()}
          {#snippet cancel()}
            <ContextMenuPrimitive.Item
              data-slot="context-menu-item"
              class="menu-sheet-cancel focus:bg-accent focus:text-accent-foreground relative flex cursor-default items-center outline-hidden select-none"
              closeOnSelect={false}
              onSelect={() => sub?.close()}
            >
              Back
            </ContextMenuPrimitive.Item>
          {/snippet}
        </MenuSheet>
      {/snippet}
    </ContextMenuPrimitive.SubContentStatic>
  </ContextMenuPrimitive.Portal>
{:else}
  <ContextMenuPrimitive.SubContent
    bind:ref
    data-slot="context-menu-sub-content"
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
      'data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-32 rounded-md p-1 shadow-lg ring-1 duration-100 max-w-[calc(100vw-16px)]',
      className
    )}
    {...restProps}
  >
    {@render children?.()}
  </ContextMenuPrimitive.SubContent>
{/if}
