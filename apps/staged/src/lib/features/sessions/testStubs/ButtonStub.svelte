<!--
  Test stub for button-shaped wrapper components (e.g. AlertDialog.Action /
  Cancel): a plain button that renders its children and forwards `onclick`,
  ignoring all other props. `disabled` is not applied, so a test can still
  click through it, but it is reported as `data-disabled` for tests that
  assert on it.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    onclick,
    disabled = false,
    children,
  }: {
    onclick?: (e: MouseEvent) => void;
    disabled?: boolean;
    children?: Snippet;
  } = $props();
</script>

<button type="button" {onclick} data-disabled={disabled ? '' : undefined}>
  {@render children?.()}
</button>
