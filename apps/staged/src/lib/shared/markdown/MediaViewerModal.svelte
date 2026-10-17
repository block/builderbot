<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import type { MediaKind } from './mediaUrl';
  import { createDiagramZoomController } from './diagramZoom';

  interface Props {
    open: boolean;
    src: string;
    kind: MediaKind;
    title: string;
    onClose: () => void;
  }

  let { open, src, kind, title, onClose }: Props = $props();
  let failed = $state(false);

  // Images get the diagram viewer's zoom controller (pinch, drag, wheel,
  // double-tap). The app's viewport meta disables page zoom, so without it a
  // pinch on the full-screen image does nothing in mobile Safari. The image is
  // laid out at its natural size and the controller scales it to fit.
  const MIN_SCALE = 0.05;
  const MAX_SCALE = 8;
  const FIT_PADDING = 16;
  const DOUBLE_TAP_SCALE = 2;

  let stageEl = $state<HTMLDivElement | null>(null);
  let surfaceEl = $state<HTMLDivElement | null>(null);
  let imageSize = $state<{ width: number; height: number } | null>(null);
  let scale = $state(1);
  let offsetX = $state(0);
  let offsetY = $state(0);
  let dragging = $state(false);

  let isImage = $derived(kind === 'image');
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
    // Fit on open and again whenever the stage changes size: the dialog
    // settling into its full-screen layout, rotation, window resizes.
    const observer = new ResizeObserver(() => controller.reset());
    observer.observe(stageEl);
    controller.reset();

    return () => {
      observer.disconnect();
      controller.destroy();
    };
  });

  function handleImageLoad(event: Event) {
    const img = event.currentTarget as HTMLImageElement;
    imageSize = { width: img.naturalWidth, height: img.naturalHeight };
  }
</script>

<Dialog.Root {open} onOpenChange={(value) => !value && onClose()}>
  <Dialog.Content
    class="max-w-[90vw] sm:max-w-[90vw] max-h-[90vh] bg-background p-0 gap-0 overflow-hidden border border-[var(--border-subtle)] flex flex-col {isImage
      ? 'w-[90vw] h-[90vh]'
      : 'w-auto'}"
    showCloseButton={false}
  >
    <Dialog.Header
      class="flex-row items-center justify-between gap-3 px-4 py-3 border-b border-[var(--border-subtle)]"
    >
      <Dialog.Title class="min-w-0 text-[var(--size-sm)] font-medium text-foreground truncate"
        >{title || (kind === 'video' ? 'Video' : 'Image')}</Dialog.Title
      >
      <Button variant="ghost" size="sm" onclick={onClose}>Close</Button>
    </Dialog.Header>
    <div class="media-viewer-body" class:image={isImage}>
      {#if open}
        {#key src}
          {#if kind === 'video'}
            <!-- Agent recordings need not contain speech; no caption track is available. -->
            <!-- svelte-ignore a11y_media_has_caption -->
            <video
              {src}
              controls
              autoplay
              playsinline
              aria-label={title || 'Video'}
              onerror={() => (failed = true)}
            ></video>
          {:else}
            <div class="viewer-stage" class:dragging bind:this={stageEl}>
              <div
                class="image-surface"
                class:sized={imageSize !== null}
                style={surfaceStyle}
                bind:this={surfaceEl}
              >
                <img {src} alt={title} onload={handleImageLoad} onerror={() => (failed = true)} />
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
    min-height: 120px;
    padding: 16px;
    overflow: auto;
  }
  .media-viewer-body.image {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    padding: 0;
    overflow: hidden;
  }
  .media-viewer-body video {
    display: block;
    max-width: 100%;
    max-height: calc(90vh - 100px);
    margin: auto;
    object-fit: contain;
    border-radius: 4px;
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
    pointer-events: none;
  }
  .media-viewer-body p {
    color: var(--text-muted);
    font-size: var(--size-sm);
    padding: 16px;
  }
</style>
