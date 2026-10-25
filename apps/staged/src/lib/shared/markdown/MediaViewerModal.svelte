<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { createDiagramZoomController } from './diagramZoom';

  interface Props {
    open: boolean;
    src: string;
    title: string;
    onClose: () => void;
  }

  let { open, src, title, onClose }: Props = $props();
  let failed = $state(false);

  // The image gets the diagram viewer's zoom controller (pinch, drag, wheel,
  // double-tap). The app's viewport meta disables page zoom, so without it a
  // pinch on the full-screen image does nothing in mobile Safari. The image is
  // laid out at its natural size and the controller scales it to fit.
  const MIN_SCALE = 0.05;
  const MAX_SCALE = 8;
  const FIT_PADDING = 16;
  const DOUBLE_TAP_SCALE = 2;
  // An <img> without intrinsic dimensions (an SVG with no width/height or
  // viewBox) reports 0×0; lay it out at the replaced-element default instead.
  const DEFAULT_IMAGE_SIZE = { width: 300, height: 150 };

  let stageEl = $state<HTMLDivElement | null>(null);
  let surfaceEl = $state<HTMLDivElement | null>(null);
  let imageSize = $state<{ width: number; height: number } | null>(null);
  let scale = $state(1);
  let offsetX = $state(0);
  let offsetY = $state(0);
  let dragging = $state(false);

  let surfaceStyle = $derived(
    imageSize
      ? `width: ${imageSize.width}px; height: ${imageSize.height}px; transform: matrix(${scale}, 0, 0, ${scale}, ${offsetX}, ${offsetY});`
      : ''
  );

  $effect(() => {
    if (open && src) {
      failed = false;
      imageSize = null;
    }
  });

  $effect(() => {
    if (!stageEl || !surfaceEl || !imageSize) return;

    const controller = createDiagramZoomController(stageEl, surfaceEl, {
      minScale: MIN_SCALE,
      maxScale: MAX_SCALE,
      fitPadding: FIT_PADDING,
      doubleClickScale: DOUBLE_TAP_SCALE,
      doubleClickTogglesFit: true,
      constrainToContent: true,
      isEnabled: () => imageSize !== null,
      onTransform: (transform) => {
        scale = transform.scale;
        offsetX = transform.offsetX;
        offsetY = transform.offsetY;
      },
      onDraggingChange: (isDragging) => {
        dragging = isDragging;
      },
    });
    // Fit on open and again whenever the stage changes size (the dialog
    // settling into its full-screen layout, rotation, window resizes) for as
    // long as the view is still at the fit; a zoomed or panned view is kept.
    const observer = new ResizeObserver(() => controller.refit());
    observer.observe(stageEl);
    controller.reset();

    return () => {
      observer.disconnect();
      controller.destroy();
    };
  });

  function handleImageLoad(event: Event) {
    const img = event.currentTarget as HTMLImageElement;
    imageSize =
      img.naturalWidth > 0 && img.naturalHeight > 0
        ? { width: img.naturalWidth, height: img.naturalHeight }
        : DEFAULT_IMAGE_SIZE;
  }
</script>

<Dialog.Root {open} onOpenChange={(value) => !value && onClose()}>
  <Dialog.Content
    class="w-[90vw] h-[90vh] max-w-[90vw] sm:max-w-[90vw] max-h-[90vh] bg-background p-0 gap-0 overflow-hidden border border-[var(--border-subtle)] flex flex-col"
    showCloseButton={false}
  >
    <Dialog.Header
      class="flex-row items-center justify-between gap-3 px-4 py-3 border-b border-[var(--border-subtle)]"
    >
      <Dialog.Title class="min-w-0 text-[var(--size-sm)] font-medium text-foreground truncate"
        >{title || 'Image'}</Dialog.Title
      >
      <Button variant="ghost" size="sm" onclick={onClose}>Close</Button>
    </Dialog.Header>
    <div class="media-viewer-body">
      {#if open}
        {#key src}
          {#if !failed}
            <div class="viewer-stage" class:dragging bind:this={stageEl}>
              <div
                class="image-surface"
                class:sized={imageSize !== null}
                style={surfaceStyle}
                bind:this={surfaceEl}
              >
                <img
                  {src}
                  alt={title}
                  draggable="false"
                  onload={handleImageLoad}
                  onerror={() => (failed = true)}
                />
              </div>
            </div>
          {/if}
        {/key}
        {#if failed}<p role="status">
            Unable to display this media. The file may be missing or its codec unsupported.
          </p>{/if}
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
  .media-viewer-body {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .viewer-stage {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    cursor: grab;
    touch-action: none;
    user-select: none;
    -webkit-user-select: none;
  }
  .viewer-stage.dragging {
    cursor: grabbing;
  }
  .image-surface {
    position: absolute;
    top: 0;
    left: 0;
    transform-origin: 0 0;
    visibility: hidden;
  }
  .image-surface.sized {
    visibility: visible;
  }
  .image-surface img {
    display: block;
    width: 100%;
    height: 100%;
    max-width: none;
    max-height: none;
    border-radius: 4px;
    -webkit-touch-callout: none;
    -webkit-user-drag: none;
  }
  .media-viewer-body p {
    margin: auto;
    padding: 16px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }
</style>
