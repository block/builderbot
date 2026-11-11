<!--
  Test stub for AlertDialog.Root: renders its children when `open` and
  exposes the dismiss path (Escape / overlay click in the real component) as
  a button, so a test can close the dialog "any other way" and check what
  the component does with `onOpenChange(false)`.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    open = false,
    onOpenChange,
    children,
  }: {
    open?: boolean;
    onOpenChange?: (open: boolean) => void;
    children?: Snippet;
  } = $props();
</script>

{#if open}
  <button type="button" data-testid="alert-dialog-dismiss" onclick={() => onOpenChange?.(false)}>
    dismiss
  </button>
  {@render children?.()}
{/if}
