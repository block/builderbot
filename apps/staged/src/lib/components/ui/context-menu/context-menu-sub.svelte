<script lang="ts">
  import { ContextMenu as ContextMenuPrimitive } from 'bits-ui';
  import { setMenuSub } from '$lib/components/ui/menu-sheet/menu-sub-context.js';

  let {
    open = $bindable(false),
    onOpenChange,
    ...restProps
  }: ContextMenuPrimitive.SubProps = $props();

  // The sheet-mode sub-content's Back row closes just this sub-menu through
  // here; mirror bits-ui's own close so a consumer's onOpenChange still fires.
  setMenuSub({
    close() {
      if (!open) return;
      open = false;
      onOpenChange?.(false);
    },
  });
</script>

<ContextMenuPrimitive.Sub bind:open {onOpenChange} {...restProps} />
