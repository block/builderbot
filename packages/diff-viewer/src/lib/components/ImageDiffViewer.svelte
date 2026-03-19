<!--
  ImageDiffViewer.svelte - Rich image diff viewer

  Displays before/after images with three viewing modes:
  1. Classic: before and after images in left/right panes
  2. Highlight: canvas-based pixel difference visualization
  3. Slider: draggable vertical divider revealing before vs after
-->
<script lang="ts">
  import { onMount } from 'svelte';

  interface Props {
    beforeSrc: string | null;
    afterSrc: string | null;
  }

  let { beforeSrc, afterSrc }: Props = $props();

  type ViewMode = 'classic' | 'highlight' | 'slider';
  let mode: ViewMode = $state('classic');

  // Slider state
  let sliderPosition = $state(50);
  let isDragging = $state(false);
  let sliderContainer: HTMLDivElement | undefined = $state();

  // Highlight canvas
  let highlightCanvas: HTMLCanvasElement | undefined = $state();
  let highlightReady = $state(false);

  // Shared image dimensions (for slider/highlight modes)
  let sharedWidth = $state(0);
  let sharedHeight = $state(0);

  // Track loaded images for highlight mode
  let beforeImg: HTMLImageElement | null = $state(null);
  let afterImg: HTMLImageElement | null = $state(null);

  function loadImage(src: string): Promise<HTMLImageElement> {
    return new Promise((resolve, reject) => {
      const img = new Image();
      img.onload = () => resolve(img);
      img.onerror = reject;
      img.src = src;
    });
  }

  async function loadImages() {
    const [bImg, aImg] = await Promise.all([
      beforeSrc ? loadImage(beforeSrc) : Promise.resolve(null),
      afterSrc ? loadImage(afterSrc) : Promise.resolve(null),
    ]);
    beforeImg = bImg;
    afterImg = aImg;

    // Compute shared dimensions based on the larger image
    const bw = bImg?.naturalWidth ?? 0;
    const bh = bImg?.naturalHeight ?? 0;
    const aw = aImg?.naturalWidth ?? 0;
    const ah = aImg?.naturalHeight ?? 0;
    sharedWidth = Math.max(bw, aw);
    sharedHeight = Math.max(bh, ah);
  }

  $effect(() => {
    // Re-load when sources change
    beforeSrc;
    afterSrc;
    loadImages();
  });

  // Draw highlight canvas when images or mode change
  $effect(() => {
    if (mode !== 'highlight' || !highlightCanvas) return;
    if (!beforeImg && !afterImg) return;

    drawHighlight();
  });

  function drawHighlight() {
    if (!highlightCanvas) return;
    const ctx = highlightCanvas.getContext('2d');
    if (!ctx) return;

    const w = sharedWidth;
    const h = sharedHeight;
    if (w === 0 || h === 0) return;

    highlightCanvas.width = w;
    highlightCanvas.height = h;

    if (!beforeImg || !afterImg) {
      // Only one image — just draw it with a note
      const img = beforeImg ?? afterImg;
      if (img) ctx.drawImage(img, 0, 0);
      highlightReady = true;
      return;
    }

    // Draw both images onto offscreen canvases
    const offBefore = new OffscreenCanvas(w, h);
    const ctxB = offBefore.getContext('2d')!;
    ctxB.drawImage(beforeImg, 0, 0);

    const offAfter = new OffscreenCanvas(w, h);
    const ctxA = offAfter.getContext('2d')!;
    ctxA.drawImage(afterImg, 0, 0);

    const dataBefore = ctxB.getImageData(0, 0, w, h);
    const dataAfter = ctxA.getImageData(0, 0, w, h);
    const output = ctx.createImageData(w, h);

    for (let i = 0; i < dataBefore.data.length; i += 4) {
      const rDiff = Math.abs(dataBefore.data[i] - dataAfter.data[i]);
      const gDiff = Math.abs(dataBefore.data[i + 1] - dataAfter.data[i + 1]);
      const bDiff = Math.abs(dataBefore.data[i + 2] - dataAfter.data[i + 2]);
      const maxDiff = Math.max(rDiff, gDiff, bDiff);

      if (maxDiff > 10) {
        // Changed pixel: show magenta highlight blended with the after image
        const intensity = Math.min(maxDiff / 255, 1);
        output.data[i] = Math.round(dataAfter.data[i] * (1 - intensity) + 255 * intensity);
        output.data[i + 1] = Math.round(dataAfter.data[i + 1] * (1 - intensity) + 0 * intensity);
        output.data[i + 2] = Math.round(
          dataAfter.data[i + 2] * (1 - intensity) + 255 * intensity
        );
        output.data[i + 3] = 255;
      } else {
        // Unchanged pixel: desaturate and dim
        const avg = (dataAfter.data[i] + dataAfter.data[i + 1] + dataAfter.data[i + 2]) / 3;
        const dimmed = avg * 0.4;
        output.data[i] = dimmed;
        output.data[i + 1] = dimmed;
        output.data[i + 2] = dimmed;
        output.data[i + 3] = 255;
      }
    }

    ctx.putImageData(output, 0, 0);
    highlightReady = true;
  }

  // Slider drag handling
  function onPointerDown(e: PointerEvent) {
    isDragging = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    updateSlider(e);
  }

  function onPointerMove(e: PointerEvent) {
    if (!isDragging) return;
    updateSlider(e);
  }

  function onPointerUp() {
    isDragging = false;
  }

  function updateSlider(e: PointerEvent) {
    if (!sliderContainer) return;
    const rect = sliderContainer.getBoundingClientRect();
    const x = e.clientX - rect.left;
    sliderPosition = Math.max(0, Math.min(100, (x / rect.width) * 100));
  }
</script>

<div class="image-diff-viewer">
  {#if beforeSrc && afterSrc}
    <div class="mode-toolbar">
      <button
        class="mode-btn"
        class:active={mode === 'classic'}
        onclick={() => (mode = 'classic')}
      >
        Classic
      </button>
      <button
        class="mode-btn"
        class:active={mode === 'highlight'}
        onclick={() => (mode = 'highlight')}
      >
        Highlight
      </button>
      <button
        class="mode-btn"
        class:active={mode === 'slider'}
        onclick={() => (mode = 'slider')}
      >
        Slider
      </button>
    </div>
  {/if}

  <div class="image-content" class:slider-active={mode === 'slider'}>
    {#if mode === 'classic'}
      <div class="side-by-side">
        <div class="image-pane">
          {#if beforeSrc}
            <img src={beforeSrc} alt="Before" />
          {:else}
            <div class="no-image">No previous version</div>
          {/if}
        </div>
        <div class="image-pane">
          {#if afterSrc}
            <img src={afterSrc} alt="After" />
          {:else}
            <div class="no-image">File deleted</div>
          {/if}
        </div>
      </div>
    {:else if mode === 'highlight'}
      <div class="highlight-container">
        {#if !beforeSrc || !afterSrc}
          <div class="no-comparison">
            {#if beforeSrc}
              <img src={beforeSrc} alt="Before (no comparison available)" />
            {:else if afterSrc}
              <img src={afterSrc} alt="After (no comparison available)" />
            {/if}
            <p class="no-comparison-note">No comparison available</p>
          </div>
        {:else}
          <canvas
            bind:this={highlightCanvas}
            style="max-width: 100%; height: auto;"
          ></canvas>
        {/if}
      </div>
    {:else if mode === 'slider'}
      <div
        class="slider-container"
        bind:this={sliderContainer}
        onpointerdown={onPointerDown}
        onpointermove={onPointerMove}
        onpointerup={onPointerUp}
        role="slider"
        aria-valuenow={Math.round(sliderPosition)}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label="Image comparison slider"
        tabindex={0}
      >
        {#if sharedWidth > 0 && sharedHeight > 0}
          <div
            class="slider-images"
            style="aspect-ratio: {sharedWidth} / {sharedHeight};"
          >
            {#if afterSrc}
              <img
                src={afterSrc}
                alt="After"
                class="slider-img"
                draggable="false"
              />
            {/if}
            {#if beforeSrc}
              <img
                src={beforeSrc}
                alt="Before"
                class="slider-img"
                draggable="false"
                style="clip-path: inset(0 {100 - sliderPosition}% 0 0);"
              />
            {/if}
          </div>
          <div
            class="slider-divider"
            style="left: {sliderPosition}%;"
          >
            <div class="slider-handle"></div>
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .image-diff-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: auto;
  }

  .mode-toolbar {
    display: flex;
    justify-content: center;
    gap: 2px;
    padding: 8px 12px;
    background: var(--bg-secondary);
    flex-shrink: 0;
  }

  .mode-btn {
    padding: 4px 12px;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    background: var(--bg-primary);
    color: var(--text-muted);
    cursor: pointer;
    font-size: 12px;
    transition: background-color 0.15s, color 0.15s;
  }

  .mode-btn:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .mode-btn.active {
    background: var(--ui-accent);
    color: white;
    border-color: var(--ui-accent);
  }

  .mode-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .image-content {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
    overflow: auto;
  }

  .image-content.slider-active {
    padding-top: 0;
    padding-bottom: 0;
  }

  /* Classic mode */
  .side-by-side {
    display: flex;
    gap: 16px;
    width: 100%;
    height: 100%;
    align-items: center;
    justify-content: center;
  }

  .image-pane {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 0;
    max-height: 100%;
  }

  .image-pane img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: 4px;
    background: var(--bg-checkerboard, repeating-conic-gradient(#80808020 0% 25%, transparent 0% 50%) 50% / 16px 16px);
  }

  .no-image {
    color: var(--text-muted);
    font-size: var(--size-lg);
    text-align: center;
  }

  /* Highlight mode */
  .highlight-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .highlight-container canvas {
    border-radius: 4px;
  }

  .no-comparison {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .no-comparison img {
    max-width: 100%;
    max-height: 60vh;
    object-fit: contain;
    border-radius: 4px;
  }

  .no-comparison-note {
    color: var(--text-muted);
    font-size: 13px;
  }

  /* Slider mode */
  .slider-container {
    position: relative;
    width: 100%;
    height: 100%;
    max-width: 100%;
    cursor: ew-resize;
    user-select: none;
    touch-action: none;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .slider-images {
    position: relative;
    width: 100%;
    max-height: 100%;
  }

  .slider-img {
    position: absolute;
    top: 0;
    left: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .slider-divider {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--ui-accent, #3b82f6);
    transform: translateX(-50%);
    pointer-events: none;
    z-index: 1;
  }

  .slider-handle {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 32px;
    height: 32px;
    border-radius: 50%;
    background: var(--ui-accent, #3b82f6);
    border: 2px solid white;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
    pointer-events: none;
  }

  .slider-handle::before,
  .slider-handle::after {
    content: '';
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 0;
    height: 0;
    border-top: 5px solid transparent;
    border-bottom: 5px solid transparent;
  }

  .slider-handle::before {
    left: 5px;
    border-right: 6px solid white;
  }

  .slider-handle::after {
    right: 5px;
    border-left: 6px solid white;
  }
</style>
