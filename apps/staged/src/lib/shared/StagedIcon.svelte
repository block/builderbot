<!--
  StagedIcon.svelte - The Staged app icon

  Renders the app-only theme-specific icon assets.

  Props:
    size - icon dimensions in px (default 32)
-->
<script lang="ts">
  let { size = 32 }: { size?: number } = $props();
</script>

<span class="staged-icon" style={`--staged-icon-size: ${size}px`} aria-hidden="true">
  <img
    class="icon-layer light"
    src="/icons/staged-monochrome-128.png"
    srcset="/icons/staged-monochrome-64.png 64w, /icons/staged-monochrome-128.png 128w, /icons/staged-monochrome-256.png 256w"
    sizes={`${size}px`}
    width={size}
    height={size}
    alt=""
    decoding="async"
  />
  <img
    class="icon-layer dark"
    src="/icons/staged-clear-128.png"
    srcset="/icons/staged-clear-64.png 64w, /icons/staged-clear-128.png 128w, /icons/staged-clear-256.png 256w"
    sizes={`${size}px`}
    width={size}
    height={size}
    alt=""
    decoding="async"
  />
</span>

<style>
  .staged-icon {
    display: inline-grid;
    width: var(--staged-icon-size);
    height: var(--staged-icon-size);
    line-height: 0;
  }

  .icon-layer {
    grid-area: 1 / 1;
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
    pointer-events: none;
  }

  .light {
    opacity: calc(1 - var(--theme-is-dark, 0));
  }

  .dark {
    opacity: var(--theme-is-dark, 0);
  }
</style>
