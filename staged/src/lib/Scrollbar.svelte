<!--
  Scrollbar.svelte - Custom scrollbar with markers
  
  A fully custom scrollbar that can be positioned on either side.
  Handles drag, wheel, and track click interactions.
  Shows markers for changes and comments.
  Fades out when inactive.
-->
<script lang="ts">
  import { onMount } from 'svelte';

  interface Marker {
    /** Position as percentage (0-100) */
    top: number;
    /** Height as percentage (0-100) */
    height: number;
    /** Type determines color */
    type: 'change' | 'comment' | 'annotation';
  }

  interface Props {
    /** Current scroll position in pixels */
    scrollY: number;
    /** Total content height in pixels */
    contentHeight: number;
    /** Visible viewport height in pixels */
    viewportHeight: number;
    /** Which side to render on */
    side: 'left' | 'right';
    /** Callback when user scrolls via this scrollbar */
    onScroll: (deltaY: number) => void;
    /** Markers to show (changes, comments) */
    markers?: Marker[];
  }

  let { scrollY, contentHeight, viewportHeight, side, onScroll, markers = [] }: Props = $props();

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
  let maxScroll = $derived(Math.max(0, contentHeight - viewportHeight));

  /** Thumb height as percentage of track */
  let thumbHeightPercent = $derived(
    contentHeight > 0 ? Math.max(10, (viewportHeight / contentHeight) * 100) : 100
  );

  /** Thumb position as percentage of track */
  let thumbTopPercent = $derived(
    maxScroll > 0 ? (scrollY / maxScroll) * (100 - thumbHeightPercent) : 0
  );

  /** Whether scrollbar should be visible (content overflows) */
  let canScroll = $derived(contentHeight > viewportHeight);

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
    const _ = scrollY;
    if (canScroll) {
      showScrollbar();
    }
  });

  // ==========================================================================
  // Drag handling
  // ==========================================================================

  let dragStartY = 0;
  let dragStartScrollY = 0;

  function handleThumbMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();

    isDragging = true;
    dragStartY = e.clientY;
    dragStartScrollY = scrollY;

    document.addEventListener('mousemove', handleDragMove);
    document.addEventListener('mouseup', handleDragEnd);
  }

  function handleDragMove(e: MouseEvent) {
    if (!isDragging || !trackEl) return;

    const trackHeight = trackEl.clientHeight;
    const thumbHeight = (thumbHeightPercent / 100) * trackHeight;
    const availableTrack = trackHeight - thumbHeight;

    if (availableTrack <= 0) return;

    const deltaY = e.clientY - dragStartY;
    const scrollDelta = (deltaY / availableTrack) * maxScroll;
    const newScrollY = Math.max(0, Math.min(maxScroll, dragStartScrollY + scrollDelta));

    onScroll(newScrollY - scrollY);
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
    const clickY = e.clientY - trackRect.top;
    const trackHeight = trackRect.height;

    // Calculate where the click is as a percentage
    const clickPercent = clickY / trackHeight;

    // Jump so the thumb center is at the click position
    const thumbHeightPx = (thumbHeightPercent / 100) * trackHeight;
    const targetThumbTop = clickY - thumbHeightPx / 2;
    const availableTrack = trackHeight - thumbHeightPx;

    if (availableTrack <= 0) return;

    const targetScrollY = (targetThumbTop / availableTrack) * maxScroll;
    const clampedScrollY = Math.max(0, Math.min(maxScroll, targetScrollY));

    onScroll(clampedScrollY - scrollY);
  }

  // ==========================================================================
  // Wheel handling (when hovering the scrollbar itself)
  // ==========================================================================

  function handleWheel(e: WheelEvent) {
    if (!canScroll) return;
    e.preventDefault();
    onScroll(e.deltaY);
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
  class="scrollbar"
  class:left={side === 'left'}
  class:right={side === 'right'}
  class:visible={isVisible}
  class:dragging={isDragging}
  onmouseenter={() => (isHovered = true)}
  onmouseleave={() => (isHovered = false)}
  onwheel={handleWheel}
>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="track" bind:this={trackEl} onclick={handleTrackClick}>
    <!-- Markers layer (behind thumb) -->
    {#each markers as marker}
      <div
        class="marker"
        class:change={marker.type === 'change'}
        class:comment={marker.type === 'comment'}
        class:annotation={marker.type === 'annotation'}
        style="top: {marker.top}%; height: {marker.height}%;"
      ></div>
    {/each}

    <!-- Thumb -->
    {#if canScroll}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="thumb"
        style="top: {thumbTopPercent}%; height: {thumbHeightPercent}%;"
        onmousedown={handleThumbMouseDown}
      ></div>
    {/if}
  </div>
</div>

<style>
  .scrollbar {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 12px;
    padding: 2px;
    opacity: 0;
    transition: opacity 0.2s ease;
    z-index: 10;
  }

  .scrollbar.visible {
    opacity: 1;
  }

  .scrollbar.left {
    left: 0;
  }

  .scrollbar.right {
    right: 0;
  }

  .track {
    position: relative;
    width: 100%;
    height: 100%;
    border-radius: 4px;
    background: transparent;
  }

  .scrollbar:hover .track {
    background: var(--scrollbar-track, rgba(128, 128, 128, 0.1));
  }

  .thumb {
    position: absolute;
    left: 0;
    right: 0;
    min-height: 20px;
    border-radius: 4px;
    background: var(--scrollbar-thumb-transparent);
    cursor: pointer;
    transition: background 0.1s ease;
  }

  .thumb:hover,
  .scrollbar.dragging .thumb {
    background: var(--scrollbar-thumb-hover-transparent);
  }

  .marker {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    width: 6px;
    min-height: 2px;
    border-radius: 1px;
    pointer-events: none;
  }

  .marker.change {
    background-color: var(--diff-range-border);
  }

  .marker.comment {
    background-color: var(--diff-comment-highlight);
  }

  .marker.annotation {
    background-color: var(--ui-accent);
  }
</style>
