<script lang="ts">
  interface Props {
    size?: number;
    color?: string;
  }

  let { size = 20, color = 'currentColor' }: Props = $props();
</script>

<div class="sine-wave-container" style="width: {size * 2}px; height: {size}px;">
  <svg
    width={size * 4}
    height={size}
    viewBox="0 0 96 24"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    class="sine-wave"
  >
    <path
      d="M0 12 C3 1, 9 1, 12 12 S21 23, 24 12 C27 1, 33 1, 36 12 S45 23, 48 12 C51 1, 57 1, 60 12 S69 23, 72 12 C75 1, 81 1, 84 12 S93 23, 96 12"
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
    clip-path: inset(0);
    display: inline-flex;
    align-items: center;
  }

  .sine-wave {
    animation: scroll-wave 1.5s linear infinite;
  }

  @keyframes scroll-wave {
    from {
      transform: translateX(0);
    }
    to {
      transform: translateX(-25%);
    }
  }
</style>
