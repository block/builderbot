<!-- A context menu with one item, opened from a trigger wrapping a row. -->
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
    <div data-testid="row">Row</div>
  </ContextMenu.Trigger>
  <ContextMenu.Content>
    <ContextMenu.Item>Remove</ContextMenu.Item>
  </ContextMenu.Content>
</ContextMenu.Root>
