<!--
  HoldToDiscard.svelte - Hold-to-confirm discard button
  
  A safety mechanism for destructive actions. User must hold the button
  for a short duration to confirm, preventing accidental discards.
  Shows visual progress feedback during the hold.
-->
<script lang="ts">
  import { X } from 'lucide-svelte';

  interface Props {
    onDiscard: () => void;
    title?: string;
    holdDuration?: number;
  }

  let { onDiscard, title = 'Hold to discard', holdDuration = 700 }: Props = $props();

  let isHolding = $state(false);
  let progress = $state(0);
  let startTime: number | null = null;
  let animationFrame: number | null = null;

  function updateProgress() {
    if (!startTime) return;

    const elapsed = Date.now() - startTime;
    progress = Math.min(elapsed / holdDuration, 1);

    if (progress >= 1) {
      cancelHold();
      onDiscard();
    } else {
      animationFrame = requestAnimationFrame(updateProgress);
    }
  }

  function startHold(event: MouseEvent) {
    event.stopPropagation();
    event.preventDefault();

    isHolding = true;
    progress = 0;
    startTime = Date.now();
    animationFrame = requestAnimationFrame(updateProgress);
  }

  function cancelHold() {
    isHolding = false;
    progress = 0;
    startTime = null;

    if (animationFrame) {
      cancelAnimationFrame(animationFrame);
      animationFrame = null;
    }
  }

  function handleMouseUp() {
    cancelHold();
  }

  function handleMouseLeave() {
    cancelHold();
  }
</script>

<button
  class="hold-to-discard"
  class:holding={isHolding}
  onmousedown={startHold}
  onmouseup={handleMouseUp}
  onmouseleave={handleMouseLeave}
  {title}
>
  <!-- Progress fill (behind icon) -->
  <div class="progress-fill" style="width: {progress * 100}%"></div>

  <!-- Icon (hidden when holding) -->
  {#if !isHolding}
    <X size={12} />
  {/if}
</button>

<style>
  .hold-to-discard {
    position: relative;
    height: 100%;
    width: 28px;
    background: var(--bg-secondary);
    border: 1px solid var(--border-primary);
    border-right: none;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    /* Shift icon slightly left for visual centering */
    padding-left: 4px;
    box-sizing: border-box;
    color: var(--text-muted);
    transition:
      color 0.1s ease,
      background-color 0.1s ease;
    overflow: hidden;
  }

  .hold-to-discard:hover {
    background-color: var(--bg-input);
    color: var(--status-deleted);
  }

  .progress-fill {
    position: absolute;
    top: 0;
    left: 0;
    height: 100%;
    background-color: var(--status-deleted);
    opacity: 0.4;
    pointer-events: none;
  }
</style>
