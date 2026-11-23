<!-- A context menu with one item, opened from a trigger wrapping one row, as
     the project card, sidebar row and board card are. -->
<script lang="ts">
  import * as ContextMenu from '$lib/components/ui/context-menu';

  let {
    open = $bindable(false),
    disabled = false,
    onpointerdown,
  }: {
    open?: boolean;
    disabled?: boolean;
    onpointerdown?: (event: PointerEvent) => void;
  } = $props();
</script>

<ContextMenu.Root bind:open>
  <ContextMenu.Trigger {disabled} {onpointerdown}>
    <div data-testid="row"><span data-testid="label">Row</span></div>
  </ContextMenu.Trigger>
  <ContextMenu.Content>
    <ContextMenu.Item>Remove</ContextMenu.Item>
  </ContextMenu.Content>
</ContextMenu.Root>
