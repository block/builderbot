<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import ZoomIn from '@lucide/svelte/icons/zoom-in';
  import ZoomOut from '@lucide/svelte/icons/zoom-out';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { viewport } from '../../shared/viewport.svelte';
  import { createDiagramZoomController, type DiagramZoomController } from './diagramZoom';

  interface Props {
    open: boolean;
    svgMarkup: string | null;
    onClose: () => void;
  }

  let { open, svgMarkup, onClose }: Props = $props();

  const MIN_SCALE = 0.2;
  const MAX_SCALE = 8;
  const ZOOM_STEP = 1.25;
  const FIT_PADDING = 48;

  let viewportEl = $state<HTMLDivElement | null>(null);
  let surfaceEl = $state<HTMLDivElement | null>(null);
  let zoomController: DiagramZoomController | null = null;
  let scale = $state(1);
  let offsetX = $state(0);
  let offsetY = $state(0);
  let resetScale = $state(1);
  let resetOffsetX = $state(0);
  let resetOffsetY = $state(0);
  let dragging = $state(false);

  let transformStyle = $derived(
    `transform: matrix(${scale}, 0, 0, ${scale}, ${offsetX}, ${offsetY});`
  );
  let hasCustomTransform = $derived(
    !isClose(scale, resetScale) ||
      !isClose(offsetX, resetOffsetX) ||
      !isClose(offsetY, resetOffsetY)
  );

  $effect(() => {
    if (!viewportEl || !surfaceEl || !svgMarkup) return;

    const controller = createDiagramZoomController(viewportEl, surfaceEl, {
      minScale: MIN_SCALE,
      maxScale: MAX_SCALE,
      fitPadding: FIT_PADDING,
      isEnabled: () => svgMarkup !== null,
      onTransform: (transform) => {
        scale = transform.scale;
        offsetX = transform.offsetX;
        offsetY = transform.offsetY;
      },
      onDraggingChange: (isDragging) => {
        dragging = isDragging;
      },
    });

    zoomController = controller;
    resetView();

    return () => {
      controller.destroy();
      if (zoomController === controller) {
        zoomController = null;
      }
    };
  });

  function resetView() {
    if (zoomController) {
      const resetTransform = zoomController.getResetTransform();
      resetScale = resetTransform.scale;
      resetOffsetX = resetTransform.offsetX;
      resetOffsetY = resetTransform.offsetY;
      zoomController.reset();
    } else {
      scale = 1;
      offsetX = 0;
      offsetY = 0;
      resetScale = 1;
      resetOffsetX = 0;
      resetOffsetY = 0;
      dragging = false;
    }
  }

  function zoomBy(multiplier: number) {
    zoomController?.zoomBy(multiplier);
  }

  function closeViewer() {
    resetView();
    onClose();
  }

  function isClose(a: number, b: number): boolean {
    return Math.abs(a - b) < 0.001;
  }
</script>

<Dialog.Root {open} onOpenChange={(v) => !v && closeViewer()}>
  <Dialog.Content
    class="h-screen max-h-none w-screen max-w-none sm:max-w-none rounded-none border-0 bg-background p-0 gap-0 overflow-hidden flex flex-col"
    showCloseButton={false}
    onOpenAutoFocus={(e) => e.preventDefault()}
  >
    <Dialog.Header class="diagram-viewer-header">
      <Dialog.Title class="diagram-viewer-title">Diagram</Dialog.Title>
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
          disabled={!svgMarkup || !hasCustomTransform}
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

    <div class="viewer-stage" class:dragging bind:this={viewportEl}>
      {#if svgMarkup}
        <div class="diagram-surface" style={transformStyle} bind:this={surfaceEl}>
          {@html svgMarkup}
        </div>
      {:else}
        <div class="placeholder error">Failed to load diagram</div>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
  :global(.diagram-viewer-header) {
    position: relative;
    display: flex;
    flex-direction: row;
    align-items: center;
    min-height: 49px;
    flex-shrink: 0;
    padding: 8px 12px 8px 84px;
    border-bottom: 1px solid var(--border-subtle);
  }

  :global(.diagram-viewer-title) {
    position: absolute;
    top: 50%;
    left: 50%;
    width: min(44vw, 420px);
    min-width: 0;
    overflow: hidden;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 500;
    text-align: center;
    text-overflow: ellipsis;
    white-space: nowrap;
    transform: translate(-50%, -50%);
    pointer-events: none;
  }

  .header-actions {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 4px;
    margin-left: auto;
    flex-shrink: 0;
  }

  .viewer-stage {
    position: relative;
    display: grid;
    flex: 1;
    place-items: center;
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
    position: absolute;
    top: 0;
    left: 0;
    width: max-content;
    height: max-content;
    transform-origin: 0 0;
  }

  .diagram-surface :global(svg) {
    display: block;
    width: auto;
    height: auto;
    max-width: none;
    max-height: none;
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
