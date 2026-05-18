<!--
  StagedIcon.svelte - The Staged app icon

  Renders the app-only greyscale icon asset.

  Props:
    size - icon dimensions in px (default 32)
-->
<script lang="ts">
  let { size = 32 }: { size?: number } = $props();
</script>

<img
  class="staged-icon"
  src="/icons/staged-greyscale.svg"
  width={size}
  height={size}
  style={`--staged-icon-size: ${size}px`}
  alt=""
  aria-hidden="true"
  decoding="async"
/>

<style>
  .staged-icon {
    display: block;
    width: var(--staged-icon-size);
    height: var(--staged-icon-size);
    object-fit: contain;
    filter: invert(var(--theme-is-dark, 0));
  }
</style>
