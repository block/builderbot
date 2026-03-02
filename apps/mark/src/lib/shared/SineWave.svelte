<script lang="ts">
  interface Props {
    size?: number;
    color?: string;
  }

  let { size = 20, color = 'currentColor' }: Props = $props();
</script>

<div class="sine-wave-container" style="width: {size}px; height: {size}px;">
  <svg
    width={size * 3}
    height={size}
    viewBox="0 0 72 24"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    class="sine-wave"
  >
    <path
      d="M0 12 C4 6, 8 6, 12 12 S20 18, 24 12 C28 6, 32 6, 36 12 S44 18, 48 12 C52 6, 56 6, 60 12 S68 18, 72 12"
      stroke={color}
      stroke-width="2"
      stroke-linecap="round"
      fill="none"
    />
  </svg>
</div>

<style>
  .sine-wave-container {
    overflow: hidden;
    display: inline-flex;
    align-items: center;
  }

  .sine-wave {
    animation: scroll-wave 1s linear infinite;
  }

  @keyframes scroll-wave {
    from {
      transform: translateX(0);
    }
    to {
      transform: translateX(-33.333%);
    }
  }
</style>
