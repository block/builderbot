<!--
  Test stub for menu items (DropdownMenu.Item): a plain button that renders
  its children, honours `disabled`, and runs `onSelect` on click, so component
  tests can pick an entry without opening a real floating menu.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    disabled = false,
    onSelect,
    children,
  }: {
    disabled?: boolean;
    onSelect?: () => void;
    children?: Snippet;
  } = $props();
</script>

<button type="button" role="menuitem" {disabled} onclick={() => onSelect?.()}>
  {@render children?.()}
</button>
