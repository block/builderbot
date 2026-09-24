<!--
  dialog-resize-handle.svelte — drag the right edge of a dialog to resize it

  Render as the last child of a `Dialog.Content` whose width comes from
  `createDialogWidth`. The dialog stays centred on `left-1/2`, so widening by W
  moves each edge by W/2 — the handle therefore applies twice the pointer delta
  to keep the right edge under the cursor.

  Pointer capture on the button keeps move/up events coming to us even when the
  cursor leaves the dialog, and gives us `pointercancel` for free. Dragging out
  over the overlay is safe: bits-ui only dismisses on a `pointerdown` that
  starts outside the content.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { viewport } from '$lib/shared/viewport.svelte';
  import { dialogMaxWidth } from './dialogWidth.svelte.js';

  const KEYBOARD_STEP = 16;

  interface Props {
    /** Current total width of the dialog in px. */
    width: number;
    /** Narrowest the dialog may get, in px. */
    minWidth: number;
    /**
     * Called with the requested total width. `commit` is false while dragging
     * and true once the gesture ends, so callers can persist only the result.
     */
    onWidthChange: (width: number, commit: boolean) => void;
    /** Double-click handler; restores the default width. */
    onReset: () => void;
    label?: string;
  }

  let { width, minWidth, onWidthChange, onReset, label = 'Resize dialog' }: Props = $props();

  let resizing = $state(false);
  let startX = 0;
  let startWidth = 0;
  let captured: { handle: HTMLButtonElement; pointerId: number } | null = null;

  function startResize(event: PointerEvent) {
    if (event.button !== 0) return;
    // Also stops `Dialog.Content`'s full-screen window drag from claiming this.
    event.preventDefault();

    const handle = event.currentTarget as HTMLButtonElement;
    handle.setPointerCapture(event.pointerId);
    captured = { handle, pointerId: event.pointerId };

    resizing = true;
    startX = event.clientX;
    startWidth = width;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  /**
   * Clamp against the total the dialog may reach before handing the width on.
   * Callers that split the total across columns (NoteModal's chat pane) store
   * less than they are given, so they cannot do this clamp themselves.
   */
  function requestWidth(next: number, commit: boolean) {
    const max = dialogMaxWidth(minWidth);
    onWidthChange(Math.max(minWidth, Math.min(max, next)), commit);
  }

  function handleMove(event: PointerEvent) {
    if (!resizing) return;
    requestWidth(startWidth + (event.clientX - startX) * 2, false);
  }

  function stopResize() {
    if (!resizing) return;
    resizing = false;
    releaseCapture();
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    requestWidth(width, true);
  }

  function releaseCapture() {
    if (!captured) return;
    const { handle, pointerId } = captured;
    captured = null;
    if (handle.hasPointerCapture(pointerId)) {
      handle.releasePointerCapture(pointerId);
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      requestWidth(width - KEYBOARD_STEP, true);
    } else if (event.key === 'ArrowRight') {
      event.preventDefault();
      requestWidth(width + KEYBOARD_STEP, true);
    } else if (event.key === 'Home') {
      event.preventDefault();
      requestWidth(minWidth, true);
    } else if (event.key === 'End') {
      event.preventDefault();
      requestWidth(dialogMaxWidth(minWidth), true);
    }
  }

  onDestroy(() => {
    if (!resizing) return;
    resizing = false;
    releaseCapture();
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
  });
</script>

{#if !viewport.isMobile}
  <button
    type="button"
    class="dialog-resize-handle"
    class:active={resizing}
    data-slot="dialog-resize-handle"
    aria-label={label}
    onpointerdown={startResize}
    onpointermove={handleMove}
    onpointerup={stopResize}
    onpointercancel={stopResize}
    onkeydown={handleKeydown}
    ondblclick={onReset}
  ></button>
{/if}

<style>
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
