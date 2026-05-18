<!--
  StagedIcon.svelte - The Staged app icon

  Renders the generated public app icon asset.

  Props:
    size - icon dimensions in px (default 32)
-->
<script lang="ts">
  let { size = 32 }: { size?: number } = $props();
</script>

<img
  class="staged-icon"
  src="/icons/staged-512.png"
  srcset="/icons/staged-192.png 192w, /icons/staged-512.png 512w"
  sizes={`${size}px`}
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
  }
</style>
