<!--
  Test stub for button-shaped wrapper components (e.g. AlertDialog.Action /
  Cancel): a plain button that renders its children and forwards `onclick`,
  ignoring all other props.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    onclick,
    children,
  }: {
    onclick?: (e: MouseEvent) => void;
    children?: Snippet;
  } = $props();
</script>

<button type="button" {onclick}>
  {@render children?.()}
</button>
