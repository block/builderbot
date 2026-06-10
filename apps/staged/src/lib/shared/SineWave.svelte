<script lang="ts">
  interface Props {
    size?: number;
    color?: string;
  }

  let { size = 20, color = 'currentColor' }: Props = $props();

  let phase = $state(0);
  let svgEl: SVGSVGElement;

  // The wave only needs to animate while it's actually on screen. Gating the
  // rAF loop on document visibility and viewport intersection stops it from
  // keeping the compositor warm when the tab is backgrounded or the element is
  // scrolled out of view.
  $effect(() => {
    let id: number | undefined;
    let t0: number | undefined;
    let documentVisible = document.visibilityState !== 'hidden';
    let inViewport = true;

    function tick(ts: number) {
      t0 ??= ts;
      phase = (((ts - t0) % 1500) / 1500) * Math.PI * 2;
      id = requestAnimationFrame(tick);
    }

    function start() {
      if (id === undefined && documentVisible && inViewport) {
        t0 = undefined;
        id = requestAnimationFrame(tick);
      }
    }

    function stop() {
      if (id !== undefined) {
        cancelAnimationFrame(id);
        id = undefined;
      }
    }

    function onVisibilityChange() {
      documentVisible = document.visibilityState !== 'hidden';
      if (documentVisible) start();
      else stop();
    }

    document.addEventListener('visibilitychange', onVisibilityChange);

    const observer = new IntersectionObserver((entries) => {
      inViewport = entries[entries.length - 1].isIntersecting;
      if (inViewport) start();
      else stop();
    });
    observer.observe(svgEl);

    start();

    return () => {
      stop();
      document.removeEventListener('visibilitychange', onVisibilityChange);
      observer.disconnect();
    };
  });

  let pathD = $derived.by(() => {
    const segments = 32;
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
  <svg
    bind:this={svgEl}
    style="width: {size}px; height: {size}px;"
    viewBox="0 0 100 100"
    fill="none"
  >
    <path
      d={pathD}
      stroke={color}
      stroke-width="1.2"
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
