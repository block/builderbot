<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { untrack } from 'svelte';
  import { createDiagramZoomController, type DiagramZoomController } from './diagramZoom';
  import {
    pinchOpenProgress,
    pinchTransform,
    transformOverRect,
    visibleImageRect,
    type MediaPinchGesture,
  } from './mediaPinch';

  interface Props {
    open: boolean;
    src: string;
    title: string;
    onClose: () => void;
    /** The inline image this was opened from; a pinch close returns the picture to it. */
    originImage?: HTMLImageElement | null;
    /** A pinch on the inline image that is opening the viewer (see mediaPinch.ts). */
    gesture?: MediaPinchGesture | null;
  }

  let { open, src, title, onClose, originImage = null, gesture = null }: Props = $props();
  let failed = $state(false);

  // Pinch transitions. The picture follows the fingers between its place in
  // the note and the fit, and the viewer's own surface (backdrop, header,
  // overlay) fades with it, so the note shows through underneath:
  //   opening     a pinch-out on the inline image is driving the picture
  //   dismissing  a pinch-in from the fit has taken it below the fit
  //   settling    the fingers lifted and it is animating to where it rests
  type Phase = 'idle' | 'opening' | 'dismissing' | 'settling';
  const SETTLE_MS = 220;
  let phase = $state<Phase>(untrack(() => (gesture ? 'opening' : 'idle')));
  // How much of the viewer's surface shows: 0 is the note, 1 the viewer.
  let chrome = $state(untrack(() => (gesture ? 0 : 1)));
  // Only for a close with nowhere to return to: the picture fades out instead.
  let surfaceOpacity = $state(1);
  let settleTimer: ReturnType<typeof setTimeout> | null = null;
  let transitioning = $derived(phase !== 'idle');
  // Once a pinch transition has run, the dialog's background stays with the
  // backdrop element that fades: handing it back to the dialog at the end of
  // a transition would show the note through for a frame. The slide (or
  // zoom) the dialog opens with is off as well; it would fight the tracked
  // motion.
  let layered = $state(untrack(() => gesture !== null));
  let contentStyle = $derived(
    layered
      ? `animation:none;transition:none;background-color:transparent;border-color:transparent;box-shadow:none${transitioning ? ';overflow:visible' : ''}`
      : undefined
  );
  let fadeStyle = $derived(
    transitioning
      ? `opacity:${chrome};${phase === 'settling' ? `transition:opacity ${SETTLE_MS}ms ease-out;` : ''}`
      : undefined
  );

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
      ? `width: ${imageSize.width}px; height: ${imageSize.height}px; transform: matrix(${scale}, 0, 0, ${scale}, ${offsetX}, ${offsetY});${surfaceOpacity < 1 ? ` opacity: ${surfaceOpacity};` : ''}`
      : ''
  );

  // While the picture is in flight it stands in for the inline image, which
  // would otherwise show a second copy through the fading backdrop.
  $effect(() => {
    const image = originImage;
    if (!image || !transitioning || !imageSize) return;
    image.style.visibility = 'hidden';
    return () => {
      image.style.visibility = '';
    };
  });

  $effect(() => () => {
    if (settleTimer) clearTimeout(settleTimer);
  });

  // Until the picture has loaded there is nothing to move, and the viewer is
  // still see-through. A pinch that ends in that time is settled here: let go
  // early it closes at once, and otherwise the viewer simply appears. Once
  // the picture is in, the controller's effect below follows the pinch.
  $effect(() => {
    if (!gesture || imageSize || untrack(() => phase) !== 'opening') return;
    return gesture.subscribe({
      frame: () => {},
      end: (stayOpen) => {
        if (stayOpen) showPlainly();
        else onClose();
      },
    });
  });

  function showPlainly() {
    phase = 'idle';
    chrome = 1;
  }

  function handleImageError() {
    failed = true;
    showPlainly();
  }

  $effect(() => {
    if (open && src) {
      failed = false;
      imageSize = null;
    }
  });

  $effect(() => {
    if (!stageEl || !surfaceEl || !imageSize) return;
    const stage = stageEl;
    const size = imageSize;

    const controller = createDiagramZoomController(stage, surfaceEl, {
      minScale: MIN_SCALE,
      maxScale: MAX_SCALE,
      fitPadding: FIT_PADDING,
      doubleClickScale: DOUBLE_TAP_SCALE,
      doubleClickTogglesFit: true,
      constrainToContent: true,
      pinchDismiss: {
        onProgress: (progress) => {
          if (phase !== 'idle' && phase !== 'dismissing') return;
          phase = progress > 0 ? 'dismissing' : 'idle';
          if (progress > 0) layered = true;
          chrome = 1 - progress;
        },
        onRelease: (close) => {
          if (close) closeToOrigin(controller, stage, size);
          else settleAtFit(controller);
        },
      },
      // Not while a pinch on the inline image is driving, or while settling.
      isEnabled: () => imageSize !== null && (phase === 'idle' || phase === 'dismissing'),
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
    observer.observe(stage);

    // Opened by a pinch that is still going (or ended before the image
    // loaded): start over the inline image and follow the fingers from there.
    // Untracked throughout: subscribing replays the latest frame at once, and
    // its read of `phase` must not make a phase change rebuild the controller.
    const opening = untrack(() => (phase === 'opening' ? gesture : null));
    const unsubscribe = untrack(() =>
      opening?.subscribe({
        frame: (frame) => {
          if (phase !== 'opening') return;
          const stageRect = stage.getBoundingClientRect();
          const origin = transformOverRect(opening.startRect, stageRect, size);
          controller.setTransform(pinchTransform(origin, opening.start, frame, stageRect));
          chrome = pinchOpenProgress(frame.scale);
        },
        end: (stayOpen) => {
          if (phase !== 'opening') return;
          if (stayOpen) settleAtFit(controller);
          else closeToOrigin(controller, stage, size);
        },
      })
    );
    if (!opening) controller.reset();

    return () => {
      unsubscribe?.();
      observer.disconnect();
      controller.destroy();
    };
  });

  function settle(done: () => void) {
    phase = 'settling';
    if (settleTimer) clearTimeout(settleTimer);
    settleTimer = setTimeout(() => {
      settleTimer = null;
      done();
    }, SETTLE_MS + 30);
  }

  function settleAtFit(controller: DiagramZoomController) {
    settle(() => {
      phase = 'idle';
    });
    chrome = 1;
    controller.reset();
  }

  /**
   * Close by returning the picture to the inline image. With nowhere to
   * return to (the image scrolled out of view, or the note re-rendered) it
   * shrinks about its own centre and fades instead.
   */
  function closeToOrigin(
    controller: DiagramZoomController,
    stage: HTMLElement,
    size: { width: number; height: number }
  ) {
    settle(onClose);
    chrome = 0;
    const rect = visibleImageRect(originImage);
    if (rect) {
      controller.setTransform(transformOverRect(rect, stage.getBoundingClientRect(), size));
      return;
    }
    const shrunk = scale * 0.6;
    controller.setTransform({
      scale: shrunk,
      offsetX: offsetX + ((scale - shrunk) * size.width) / 2,
      offsetY: offsetY + ((scale - shrunk) * size.height) / 2,
    });
    surfaceOpacity = 0;
  }

  // The fingers of an opening pinch lift on the page behind the viewer; that
  // must not count as a press outside it.
  function handleInteractOutside(event: PointerEvent) {
    if (transitioning) event.preventDefault();
  }

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
    style={contentStyle}
    overlayProps={{ style: layered ? `animation:none;${fadeStyle ?? ''}` : undefined }}
    onInteractOutside={handleInteractOutside}
  >
    {#if layered}
      <div class="viewer-backdrop" style={fadeStyle}></div>
    {/if}
    <Dialog.Header
      style={fadeStyle}
      class="relative flex-row items-center justify-between gap-3 pt-[calc(--spacing(3)+var(--dialog-safe-top,0px))] pb-3 pl-[calc(--spacing(4)+var(--dialog-safe-left,0px))] pr-[calc(--spacing(4)+var(--dialog-safe-right,0px))] border-b border-[var(--border-subtle)]"
    >
      <Dialog.Title class="min-w-0 text-[var(--size-sm)] font-medium text-foreground truncate"
        >{title || 'Image'}</Dialog.Title
      >
      <Button variant="ghost" size="sm" onclick={onClose}>Close</Button>
    </Dialog.Header>
    <div class="media-viewer-body" class:transitioning>
      {#if open}
        {#key src}
          {#if !failed}
            <div class="viewer-stage" class:dragging bind:this={stageEl}>
              <div
                class="image-surface"
                class:sized={imageSize !== null}
                class:settling={phase === 'settling'}
                style={surfaceStyle}
                bind:this={surfaceEl}
              >
                <img
                  {src}
                  alt={title}
                  draggable="false"
                  onload={handleImageLoad}
                  onerror={handleImageError}
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
  /* Stands in for the dialog's own background and border, which are cleared
     for pinch transitions so that they can fade separately from the picture. */
  .viewer-backdrop {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background: var(--background);
    box-shadow: 0 0 0 1px var(--border-subtle);
    pointer-events: none;
  }
  /* In flight the picture crosses the header and the stage's edges. */
  .media-viewer-body.transitioning,
  .media-viewer-body.transitioning .viewer-stage {
    overflow: visible;
  }
  .image-surface.settling {
    transition:
      transform 220ms cubic-bezier(0.2, 0.8, 0.2, 1),
      opacity 220ms ease-out;
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
