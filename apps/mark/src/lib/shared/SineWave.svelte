<script lang="ts">
  interface Props {
    size?: number;
    color?: string;
  }

  let { size = 20, color = 'currentColor' }: Props = $props();

  let phase = $state(0);

  $effect(() => {
    let id: number;
    let t0: number | undefined;

    function tick(ts: number) {
      t0 ??= ts;
      phase = (((ts - t0) % 1500) / 1500) * Math.PI * 2;
      id = requestAnimationFrame(tick);
    }

    id = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(id);
  });

  let pathD = $derived.by(() => {
    const segments = 64;
    const startX = 10;
    const endX = 90;
    const range = endX - startX;
    let d = '';
    for (let i = 0; i <= segments; i++) {
      const x = startX + (i / segments) * range;
      const y = 50 - 42 * Math.sin((3 * Math.PI * x) / 100 + phase);
      d += i === 0 ? `M${x.toFixed(1)} ${y.toFixed(1)}` : `L${x.toFixed(1)} ${y.toFixed(1)}`;
    }
    return d;
  });
</script>

<div class="sine-wave-container" style="width: {size}px; height: {size}px;">
  <svg style="width: {size}px; height: {size}px;" viewBox="0 0 100 100" fill="none">
    <path
      d={pathD}
      stroke={color}
      stroke-width="0.67"
      vector-effect="non-scaling-stroke"
      stroke-linecap="round"
      stroke-linejoin="round"
      fill="none"
    />
  </svg>
</div>

<style>
  .sine-wave-container {
    display: inline-flex;
    align-items: center;
  }
</style>
