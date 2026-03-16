<!--
  DifferIcon.svelte - The Differ app icon as an inline SVG

  Renders the rounded-rect icon with the branch path inside.
  Uses currentColor for the path and a theme-derived background,
  so it works across light and dark themes.

  Props:
    size - icon dimensions in px (default 32)
-->
<script lang="ts">
  let { size = 32 }: { size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 442 442"
  fill="none"
  xmlns="http://www.w3.org/2000/svg"
  aria-hidden="true"
>
  <g clip-path="url(#differ-icon-clip)">
    <rect width="442" height="442" rx="143" class="icon-bg" />
    <path
      d="M144.621 257C177.415 257 204.001 283.585 204.001 316.38C204.001 349.174 177.415 375.759 144.621 375.759C122.332 375.759 102.911 363.478 92.7566 345.313C86.3324 333.822 75.7553 323.505 62.5901 323.505H-198.5C-202.09 323.505 -205 320.595 -205 317.005C-205 313.415 -202.09 310.505 -198.5 310.505H61.9577C75.3329 310.505 86.0125 299.871 92.3884 288.114C102.437 269.583 122.06 257 144.621 257Z"
      class="icon-path"
    />
  </g>
  <defs>
    <clipPath id="differ-icon-clip">
      <rect width="442" height="442" rx="143" fill="white" />
    </clipPath>
  </defs>
</svg>

<style>
  .icon-bg {
    fill: var(--text-primary);
  }

  .icon-path {
    fill: var(--bg-chrome);
  }
</style>
