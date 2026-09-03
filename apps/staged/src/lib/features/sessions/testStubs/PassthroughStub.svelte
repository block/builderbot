<!--
  Test stub for wrapper components (e.g. Dialog.Root/Content/Title): renders
  its children in a plain div and honours a bindable `ref`, ignoring all other
  props.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    ref = $bindable(null),
    children,
  }: {
    ref?: HTMLElement | null;
    children?: Snippet;
  } = $props();
</script>

<div bind:this={ref}>
  {@render children?.()}
</div>
