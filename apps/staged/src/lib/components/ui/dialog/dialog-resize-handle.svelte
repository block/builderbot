<!--
  dialog-resize-handle.svelte — drag the right edge of a dialog to resize it

  Render as the last child of a `Dialog.Content` whose width comes from
  `createDialogWidth`. The dialog stays centred on `left-1/2`, so widening by W
  moves each edge by W/2 — the handle therefore applies twice the pointer delta
  to keep the right edge under the cursor.

  Pointer capture on the separator keeps move/up events coming to us even when the
  cursor leaves the dialog, and gives us `pointercancel` for free. Dragging out
  over the overlay is safe: bits-ui only dismisses on a `pointerdown` that
  starts outside the content.
-->
<script lang="ts">
  import { flushSync, untrack } from 'svelte';
  import { viewport } from '$lib/shared/viewport.svelte';
  import { DIALOG_VIEWPORT_GUTTER } from './dialogWidth.svelte.js';
  import { createDialogResize, resizeBounds } from './dialogResize';

  interface Props {
    /** Narrowest the dialog may get, in px. */
    minWidth: number;
    /**
     * Called with the requested total width. `commit` is false while dragging
     * and true only for a changed final width, so callers persist only the result.
     */
    onWidthChange: (width: number, commit: boolean) => void;
    /** Always called after a gesture or direct command, including cancellation and no-ops. */
    onResizeEnd: () => void;
    /** Disable width transitions before measuring a gesture or direct command. */
    onResizeStart?: () => void;
    /** Double-click handler; restores the default width. */
    onReset: () => void;
    label?: string;
  }

  let {
    minWidth,
    onWidthChange,
    onResizeEnd,
    onResizeStart,
    onReset,
    label = 'Resize dialog',
  }: Props = $props();

  let resizing = $state(false);
  let viewportWidth = $state(window.innerWidth);
  let renderedWidth = $state(0);
  let controls = $state('');
  let dialog: HTMLElement | null = null;
  let captured: { handle: HTMLElement; pointerId: number } | null = null;
  let bodyStyles: { cursor: string; userSelect: string } | null = null;
  const bounds = $derived(resizeBounds(minWidth, viewportWidth - DIALOG_VIEWPORT_GUTTER * 2));
  const accessibleWidth = $derived(Math.max(bounds.min, Math.min(bounds.max, renderedWidth)));

  const resize = createDialogResize({
    geometry: () => ({
      ...resizeBounds(minWidth, window.innerWidth - DIALOG_VIEWPORT_GUTTER * 2),
      // Keep the entrance scale animation out of pointer and keyboard resize math.
      width: dialog?.offsetWidth ?? minWidth,
    }),
    preview: (next) => onWidthChange(next, false),
    commit: (next) => onWidthChange(next, true),
    end: () => {
      resizing = false;
      releaseCapture();
      if (bodyStyles) {
        document.body.style.cursor = bodyStyles.cursor;
        document.body.style.userSelect = bodyStyles.userSelect;
        bodyStyles = null;
      }
      onResizeEnd();
    },
  });

  function measure() {
    // Ignore the entrance scale animation, which does not trigger ResizeObserver.
    renderedWidth = dialog?.offsetWidth ?? minWidth;
  }

  function observeDialog(handle: HTMLElement) {
    dialog = handle.closest<HTMLElement>('[data-slot="dialog-content"]');
    controls = dialog?.id ?? '';
    const observer = new ResizeObserver(measure);
    if (dialog) observer.observe(dialog);
    measure();
    return {
      destroy() {
        resize.cancel();
        observer.disconnect();
        dialog = null;
      },
    };
  }

  // Changing the column layout invalidates the gesture's geometry and projection.
  $effect(() => {
    minWidth;
    viewport.isMobile;
    viewport.canSplit;
    untrack(() => resize.cancel());
  });

  function handleWindowResize() {
    resize.cancel();
    measure();
  }

  function startResize(event: PointerEvent) {
    if (event.button !== 0 || captured) return;
    // Also stops `Dialog.Content`'s full-screen window drag from claiming this.
    event.preventDefault();

    const handle = event.currentTarget as HTMLElement;
    handle.focus({ preventScroll: true });
    handle.setPointerCapture(event.pointerId);
    captured = { handle, pointerId: event.pointerId };

    resizing = true;
    bodyStyles = {
      cursor: document.body.style.cursor,
      userSelect: document.body.style.userSelect,
    };
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    flushSync(() => onResizeStart?.());
    resize.start(event);
  }

  function releaseCapture() {
    if (!captured) return;
    const { handle, pointerId } = captured;
    captured = null;
    if (handle.hasPointerCapture(pointerId)) {
      handle.releasePointerCapture(pointerId);
    }
  }

  function runImmediateResize(action: () => void) {
    flushSync(() => onResizeStart?.());
    try {
      flushSync(action);
      // Lay out the new width before the caller restores its chat transition.
      measure();
    } finally {
      onResizeEnd();
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    if (!resizing) runImmediateResize(() => resize.key(event.key));
  }
</script>

<svelte:window bind:innerWidth={viewportWidth} onresize={handleWindowResize} />

{#if !viewport.isMobile}
  <!-- A focusable ARIA separator is the interactive window-splitter pattern. -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    use:observeDialog
    role="separator"
    tabindex="0"
    class="dialog-resize-handle"
    class:active={resizing}
    data-slot="dialog-resize-handle"
    data-dialog-no-drag
    aria-label={label}
    aria-orientation="vertical"
    aria-controls={controls || undefined}
    aria-valuemin={bounds.min}
    aria-valuemax={bounds.max}
    aria-valuenow={accessibleWidth}
    aria-valuetext={`${accessibleWidth} pixels`}
    aria-disabled={bounds.min === bounds.max}
    onpointerdown={startResize}
    onpointermove={resize.move}
    onpointerup={resize.release}
    onpointercancel={(event) => resize.cancel(event.pointerId)}
    onlostpointercapture={(event) => resize.cancel(event.pointerId)}
    onkeydown={handleKeydown}
    ondblclick={() => {
      resize.cancel();
      runImmediateResize(onReset);
    }}
  ></div>
{/if}

<style>
  @media (min-width: 769px) {
    :global([data-slot='dialog-content'].dialog-resize-gutter) {
      padding-right: 8px;
    }
  }

  .dialog-resize-handle {
    position: absolute;
    top: 0;
    right: 0;
    width: 8px;
    height: 100%;
    cursor: col-resize;
    z-index: 5;
    border: none;
    background: transparent;
    padding: 0;
    touch-action: none;
  }

  .dialog-resize-handle::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    right: 2px;
    width: 2px;
    background-color: transparent;
    transition: background-color 0.15s ease;
  }

  .dialog-resize-handle:hover::after,
  .dialog-resize-handle:focus-visible::after,
  .dialog-resize-handle.active::after {
    background-color: var(--border-emphasis);
  }
</style>
