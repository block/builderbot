<script lang="ts">
  interface Props {
    size?: number;
    color?: string;
  }

  let { size = 20, color = 'currentColor' }: Props = $props();
</script>

<div class="sine-wave-container" style="width: {size}px; height: {size}px;">
  <svg
    style="width: {size * 8}px; height: {size}px;"
    viewBox="0 0 800 100"
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    class="sine-wave"
  >
    <path
      d="M 0 50 C 12.5 8, 37.5 8, 50 50 S 87.5 92, 100 50 S 137.5 8, 150 50 S 187.5 92, 200 50 S 237.5 8, 250 50 S 287.5 92, 300 50 S 337.5 8, 350 50 S 387.5 92, 400 50 S 437.5 8, 450 50 S 487.5 92, 500 50 S 537.5 8, 550 50 S 587.5 92, 600 50 S 637.5 8, 650 50 S 687.5 92, 700 50 S 737.5 8, 750 50 S 787.5 92, 800 50"
      stroke={color}
      stroke-width="2"
      vector-effect="non-scaling-stroke"
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
    -webkit-mask-image: radial-gradient(ellipse 100% 100% at center, black 40%, transparent 70%);
    mask-image: radial-gradient(ellipse 100% 100% at center, black 40%, transparent 70%);
  }

  .sine-wave {
    will-change: transform;
    animation: scroll-wave 1.5s linear infinite;
  }

  @keyframes scroll-wave {
    from {
      transform: translateX(0);
    }
    to {
      transform: translateX(-12.5%);
    }
  }
</style>
