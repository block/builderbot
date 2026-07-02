<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import ZoomIn from '@lucide/svelte/icons/zoom-in';
  import ZoomOut from '@lucide/svelte/icons/zoom-out';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { viewport } from '../../shared/viewport.svelte';

  interface Props {
    open: boolean;
    svgMarkup: string | null;
    onClose: () => void;
  }

  let { open, svgMarkup, onClose }: Props = $props();

  const MIN_SCALE = 0.2;
  const MAX_SCALE = 8;
  const ZOOM_STEP = 1.25;

  let viewportEl: HTMLDivElement;
  let scale = $state(1);
  let offsetX = $state(0);
  let offsetY = $state(0);
  let dragging = $state(false);
  let activePointerId = $state<number | null>(null);
  let lastPointerX = 0;
  let lastPointerY = 0;

  let transformStyle = $derived(
    `transform: matrix(${scale}, 0, 0, ${scale}, ${offsetX}, ${offsetY});`
  );
  let resetKey = $derived(`${open}\0${svgMarkup ?? ''}`);

  $effect(() => {
    resetKey;
    resetView();
  });

  function resetView() {
    if (activePointerId !== null && viewportEl?.hasPointerCapture(activePointerId)) {
      viewportEl.releasePointerCapture(activePointerId);
    }
    scale = 1;
    offsetX = 0;
    offsetY = 0;
    dragging = false;
    activePointerId = null;
  }

  function clampScale(value: number): number {
    return Math.min(MAX_SCALE, Math.max(MIN_SCALE, value));
  }

  function zoomBy(multiplier: number) {
    const rect = viewportEl?.getBoundingClientRect();
    if (!rect) {
      scale = clampScale(scale * multiplier);
      return;
    }

    zoomAt(multiplier, rect.left + rect.width / 2, rect.top + rect.height / 2);
  }

  function zoomAt(multiplier: number, clientX: number, clientY: number) {
    const currentScale = scale;
    const nextScale = clampScale(currentScale * multiplier);
    if (nextScale === currentScale) return;

    const rect = viewportEl?.getBoundingClientRect();
    if (!rect) {
      scale = nextScale;
      return;
    }

    const pointerX = clientX - rect.left - rect.width / 2;
    const pointerY = clientY - rect.top - rect.height / 2;

    offsetX = pointerX - ((pointerX - offsetX) / currentScale) * nextScale;
    offsetY = pointerY - ((pointerY - offsetY) / currentScale) * nextScale;
    scale = nextScale;
  }

  function handleWheel(event: WheelEvent) {
    if (!svgMarkup) return;
    event.preventDefault();
    zoomAt(event.deltaY > 0 ? 1 / ZOOM_STEP : ZOOM_STEP, event.clientX, event.clientY);
  }

  function handlePointerDown(event: PointerEvent) {
    if (event.button !== 0 || !svgMarkup) return;
    event.preventDefault();
    dragging = true;
    activePointerId = event.pointerId;
    lastPointerX = event.clientX;
    lastPointerY = event.clientY;
    viewportEl?.setPointerCapture(event.pointerId);
  }

  function handlePointerMove(event: PointerEvent) {
    if (!dragging || activePointerId !== event.pointerId) return;
    event.preventDefault();

    offsetX += event.clientX - lastPointerX;
    offsetY += event.clientY - lastPointerY;
    lastPointerX = event.clientX;
    lastPointerY = event.clientY;
  }

  function finishPointerPan(event: PointerEvent) {
    if (activePointerId !== event.pointerId) return;
    if (viewportEl?.hasPointerCapture(event.pointerId)) {
      viewportEl.releasePointerCapture(event.pointerId);
    }
    dragging = false;
    activePointerId = null;
  }

  function closeViewer() {
    resetView();
    onClose();
  }
</script>

<Dialog.Root {open} onOpenChange={(v) => !v && closeViewer()}>
  <Dialog.Content
    class="h-screen max-h-none w-screen max-w-none rounded-none border-0 bg-background p-0 gap-0 overflow-hidden flex flex-col"
    showCloseButton={false}
    onOpenAutoFocus={(e) => e.preventDefault()}
  >
    <Dialog.Header
      class="flex-row items-center justify-between gap-3 px-4 py-3 border-b border-[var(--border-subtle)] flex-shrink-0"
    >
      <Dialog.Title
        class="flex-1 min-w-0 text-[var(--size-sm)] font-medium text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
      >
        Diagram
      </Dialog.Title>
      <div class="header-actions">
        <Button
          variant="ghost"
          size="icon-sm"
          title="Zoom out"
          aria-label="Zoom out"
          disabled={!svgMarkup || scale <= MIN_SCALE}
          onclick={() => zoomBy(1 / ZOOM_STEP)}
          class="size-8 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
        >
          <ZoomOut size={16} />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          title="Reset zoom"
          aria-label="Reset zoom"
          disabled={!svgMarkup || (scale === 1 && offsetX === 0 && offsetY === 0)}
          onclick={resetView}
          class="size-8 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
        >
          <RotateCcw size={16} />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          title="Zoom in"
          aria-label="Zoom in"
          disabled={!svgMarkup || scale >= MAX_SCALE}
          onclick={() => zoomBy(ZOOM_STEP)}
          class="size-8 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-4"
        >
          <ZoomIn size={16} />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
          aria-label="Close"
          onclick={closeViewer}
          class="size-8 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-[18px]"
        >
          <X size={18} />
        </Button>
      </div>
    </Dialog.Header>

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="viewer-stage"
      class:dragging
      bind:this={viewportEl}
      onwheel={handleWheel}
      onpointerdown={handlePointerDown}
      onpointermove={handlePointerMove}
      onpointerup={finishPointerPan}
      onpointercancel={finishPointerPan}
    >
      {#if svgMarkup}
        <div class="diagram-surface" style={transformStyle}>
          {@html svgMarkup}
        </div>
      {:else}
        <div class="placeholder error">Failed to load diagram</div>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .viewer-stage {
    position: relative;
    display: flex;
    flex: 1;
    align-items: center;
    justify-content: center;
    min-height: 0;
    overflow: hidden;
    background: var(--diagram-canvas-bg);
    cursor: grab;
    touch-action: none;
    user-select: none;
  }

  .viewer-stage.dragging {
    cursor: grabbing;
  }

  .diagram-surface {
    transform-origin: center center;
    will-change: transform;
  }

  .diagram-surface :global(svg) {
    display: block;
    width: auto;
    height: auto;
    max-width: calc(100vw - 48px);
    max-height: calc(100vh - 96px);
    overflow: visible;
  }

  .placeholder {
    font-size: var(--size-sm);
    color: var(--text-muted);
    font-style: italic;
    padding: 40px;
  }

  .placeholder.error {
    color: var(--ui-danger);
  }
</style>
