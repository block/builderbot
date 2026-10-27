<script lang="ts">
  import { DropdownMenu as DropdownMenuPrimitive } from 'bits-ui';
  import { setMenuRoot } from '$lib/components/ui/menu-sheet/menu-close-context.js';

  let {
    open = $bindable(false),
    onOpenChange,
    ...restProps
  }: DropdownMenuPrimitive.RootProps = $props();

  // The sheet-mode content's scrim closes the menu through here; mirror
  // bits-ui's own close so a consumer's onOpenChange still fires.
  setMenuRoot({
    close() {
      if (!open) return;
      open = false;
      onOpenChange?.(false);
    },
  });
</script>

<DropdownMenuPrimitive.Root bind:open {onOpenChange} {...restProps} />
