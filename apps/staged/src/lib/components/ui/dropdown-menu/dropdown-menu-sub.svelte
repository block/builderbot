<script lang="ts">
  import { DropdownMenu as DropdownMenuPrimitive } from 'bits-ui';
  import { setMenuSub } from '$lib/components/ui/menu-sheet/menu-close-context.js';

  let {
    open = $bindable(false),
    onOpenChange,
    ...restProps
  }: DropdownMenuPrimitive.SubProps = $props();

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

<DropdownMenuPrimitive.Sub bind:open {onOpenChange} {...restProps} />
