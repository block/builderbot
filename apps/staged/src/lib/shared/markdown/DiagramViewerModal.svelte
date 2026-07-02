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

  let viewportEl: HTMLDivElement;
  let zoomController: DiagramZoomController | null = null;
  let scale = $state(1);
  let offsetX = $state(0);
  let offsetY = $state(0);
  let dragging = $state(false);

  let transformStyle = $derived(
    `transform: matrix(${scale}, 0, 0, ${scale}, ${offsetX}, ${offsetY});`
  );
  let resetKey = $derived(`${open}\0${svgMarkup ?? ''}`);
  let hasCustomTransform = $derived(scale !== 1 || offsetX !== 0 || offsetY !== 0);

  $effect(() => {
    if (!viewportEl) return;

    zoomController = createDiagramZoomController(viewportEl, {
      minScale: MIN_SCALE,
      maxScale: MAX_SCALE,
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

    return () => {
      zoomController?.destroy();
      zoomController = null;
    };
  });

  $effect(() => {
    resetKey;
    resetView();
  });

  function resetView() {
    if (zoomController) {
      zoomController.reset();
    } else {
      scale = 1;
      offsetX = 0;
      offsetY = 0;
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
</script>

<Dialog.Root {open} onOpenChange={(v) => !v && closeViewer()}>
  <Dialog.Content
    class="h-screen max-h-none w-screen max-w-none sm:max-w-none rounded-none border-0 bg-background p-0 gap-0 overflow-hidden flex flex-col"
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
