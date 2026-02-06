<!--
  HorizontalScrollbar.svelte - Custom horizontal scrollbar

  A fully custom horizontal scrollbar for transform-based scrolling.
  Handles drag, wheel, and track click interactions.
  Fades out when inactive.
-->
<script lang="ts">
  import { onMount } from 'svelte';

  interface Props {
    /** Current scroll position in pixels */
    scrollX: number;
    /** Total content width in pixels */
    contentWidth: number;
    /** Visible viewport width in pixels */
    viewportWidth: number;
    /** Callback when user scrolls via this scrollbar */
    onScroll: (deltaX: number) => void;
  }

  let { scrollX, contentWidth, viewportWidth, onScroll }: Props = $props();

  // ==========================================================================
  // State
  // ==========================================================================

  let trackEl: HTMLDivElement | null = $state(null);
  let isDragging = $state(false);
  let isHovered = $state(false);
  let isActive = $state(false);
  let fadeTimeout: ReturnType<typeof setTimeout> | null = null;

  // ==========================================================================
  // Derived values
  // ==========================================================================

  /** Maximum scroll position */
  let maxScroll = $derived(Math.max(0, contentWidth - viewportWidth));

  /** Thumb width as percentage of track */
  let thumbWidthPercent = $derived(
    contentWidth > 0 ? Math.max(10, (viewportWidth / contentWidth) * 100) : 100
  );

  /** Thumb position as percentage of track */
  let thumbLeftPercent = $derived(
    maxScroll > 0 ? (scrollX / maxScroll) * (100 - thumbWidthPercent) : 0
  );

  /** Whether scrollbar should be visible (content overflows) */
  let canScroll = $derived(contentWidth > viewportWidth);

  /** Whether scrollbar is visible (can scroll and is active/hovered) */
  let isVisible = $derived(canScroll && (isActive || isHovered || isDragging));

  // ==========================================================================
  // Activity tracking (for fade in/out)
  // ==========================================================================

  function showScrollbar() {
    isActive = true;
    if (fadeTimeout) clearTimeout(fadeTimeout);
    fadeTimeout = setTimeout(() => {
      isActive = false;
    }, 1500);
  }

  // Show scrollbar when scroll position changes
  $effect(() => {
    const _ = scrollX;
    if (canScroll) {
      showScrollbar();
    }
  });

  // ==========================================================================
  // Drag handling
  // ==========================================================================

  let dragStartX = 0;
  let dragStartScrollX = 0;

  function handleThumbMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();

    isDragging = true;
    dragStartX = e.clientX;
    dragStartScrollX = scrollX;

    document.addEventListener('mousemove', handleDragMove);
    document.addEventListener('mouseup', handleDragEnd);
  }

  function handleDragMove(e: MouseEvent) {
    if (!isDragging || !trackEl) return;

    const trackWidth = trackEl.clientWidth;
    const thumbWidth = (thumbWidthPercent / 100) * trackWidth;
    const availableTrack = trackWidth - thumbWidth;

    if (availableTrack <= 0) return;

    const deltaX = e.clientX - dragStartX;
    const scrollDelta = (deltaX / availableTrack) * maxScroll;
    const newScrollX = Math.max(0, Math.min(maxScroll, dragStartScrollX + scrollDelta));

    onScroll(newScrollX - scrollX);
  }

  function handleDragEnd() {
    isDragging = false;
    document.removeEventListener('mousemove', handleDragMove);
    document.removeEventListener('mouseup', handleDragEnd);
  }

  // ==========================================================================
  // Track click handling
  // ==========================================================================

  function handleTrackClick(e: MouseEvent) {
    if (!trackEl || isDragging) return;

    const target = e.target as HTMLElement;
    if (target.classList.contains('thumb')) return;

    const trackRect = trackEl.getBoundingClientRect();
    const clickX = e.clientX - trackRect.left;
    const trackWidth = trackRect.width;

    // Jump so the thumb center is at the click position
    const thumbWidthPx = (thumbWidthPercent / 100) * trackWidth;
    const targetThumbLeft = clickX - thumbWidthPx / 2;
    const availableTrack = trackWidth - thumbWidthPx;

    if (availableTrack <= 0) return;

    const targetScrollX = (targetThumbLeft / availableTrack) * maxScroll;
    const clampedScrollX = Math.max(0, Math.min(maxScroll, targetScrollX));

    onScroll(clampedScrollX - scrollX);
  }

  // ==========================================================================
  // Wheel handling (when hovering the scrollbar itself)
  // ==========================================================================

  function handleWheel(e: WheelEvent) {
    if (!canScroll) return;
    e.preventDefault();
    // Use deltaX if available, otherwise use deltaY (for shift+wheel)
    const delta = e.deltaX !== 0 ? e.deltaX : e.deltaY;
    onScroll(delta);
  }

  // ==========================================================================
  // Cleanup
  // ==========================================================================

  onMount(() => {
    return () => {
      if (fadeTimeout) clearTimeout(fadeTimeout);
      document.removeEventListener('mousemove', handleDragMove);
      document.removeEventListener('mouseup', handleDragEnd);
    };
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="scrollbar-h"
  class:visible={isVisible}
  class:dragging={isDragging}
  onmouseenter={() => (isHovered = true)}
  onmouseleave={() => (isHovered = false)}
  onwheel={handleWheel}
>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="track" bind:this={trackEl} onclick={handleTrackClick}>
    <!-- Thumb -->
    {#if canScroll}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="thumb"
        style="left: {thumbLeftPercent}%; width: {thumbWidthPercent}%;"
        onmousedown={handleThumbMouseDown}
      ></div>
    {/if}
  </div>
</div>

<style>
  .scrollbar-h {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 12px;
    padding: 2px;
    opacity: 0;
    transition: opacity 0.2s ease;
    z-index: 10;
  }

  .scrollbar-h.visible {
    opacity: 1;
  }

  .track {
    position: relative;
    width: 100%;
    height: 100%;
    border-radius: 4px;
    background: transparent;
  }

  .scrollbar-h:hover .track {
    background: var(--scrollbar-track, rgba(128, 128, 128, 0.1));
  }

  .thumb {
    position: absolute;
    top: 0;
    bottom: 0;
    min-width: 20px;
    border-radius: 4px;
    background: var(--scrollbar-thumb-transparent);
    cursor: pointer;
    transition: background 0.1s ease;
  }

  .thumb:hover,
  .scrollbar-h.dragging .thumb {
    background: var(--scrollbar-thumb-hover-transparent);
  }
</style>
