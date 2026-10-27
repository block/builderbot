<script lang="ts">
  import { DropdownMenu as DropdownMenuPrimitive } from 'bits-ui';
  import { cn } from '$lib/components/utils.js';
  import MenuSheet from '$lib/components/ui/menu-sheet/menu-sheet.svelte';
  import {
    getMenuSub,
    returnFocusFromMenuSub,
  } from '$lib/components/ui/menu-sheet/menu-close-context.js';
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
  }: DropdownMenuPrimitive.SubContentProps = $props();

  const sub = getMenuSub();

  // Back (the row, or a tap on this sheet's scrim): put focus back in the
  // parent sheet first, so it does not drop to `body` — on the sub-trigger's
  // row for a keyboard user, on the sheet itself for a tap (see
  // returnFocusFromMenuSub).
  function back() {
    returnFocusFromMenuSub(ref);
    sub?.close();
  }
</script>

{#if viewport.sheetMenus}
  <!-- A second sheet layered over the parent's. Portalled so it escapes the
       parent panel (scrolling, transformed while animating) and stacks above
       it. Its scrim covers the parent sheet, so a tap there must close only
       the sub-menu, like the Back row: bits-ui's default for a sub is to
       defer outside interactions to the parent (right for a floating sub,
       where a click beside it is a click outside the whole menu), which here
       would dismiss both sheets, so the sub claims them as its own. -->
  <DropdownMenuPrimitive.Portal>
    <DropdownMenuPrimitive.SubContentStatic
      bind:ref
      data-slot="dropdown-menu-sub-content"
      interactOutsideBehavior="close"
      {...restProps}
    >
      {#snippet child({ props })}
        <MenuSheet {props} class={className} onDismiss={back}>
          {@render children?.()}
          {#snippet cancel()}
            <DropdownMenuPrimitive.Item
              data-slot="dropdown-menu-item"
              class="menu-sheet-cancel focus:bg-accent focus:text-accent-foreground relative flex cursor-default items-center outline-hidden select-none"
              closeOnSelect={false}
              onSelect={back}
            >
              Back
            </DropdownMenuPrimitive.Item>
          {/snippet}
        </MenuSheet>
      {/snippet}
    </DropdownMenuPrimitive.SubContentStatic>
  </DropdownMenuPrimitive.Portal>
{:else}
  <DropdownMenuPrimitive.SubContent
    bind:ref
    data-slot="dropdown-menu-sub-content"
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
      'data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 ring-foreground/10 bg-popover text-popover-foreground min-w-[96px] rounded-md p-1 shadow-lg ring-1 duration-100 w-auto max-w-[calc(100vw-16px)]',
      className
    )}
    {...restProps}
  >
    {@render children?.()}
  </DropdownMenuPrimitive.SubContent>
{/if}
