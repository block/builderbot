<!--
  HoldToDiscard.svelte - Hold-to-confirm discard button
  
  A safety mechanism for destructive actions. User must hold the button
  for a short duration to confirm, preventing accidental discards.
  Shows visual progress feedback during the hold.
-->
<script lang="ts">
  interface Props {
    onDiscard: () => void;
    title?: string;
    holdDuration?: number;
  }

  let { onDiscard, title = 'Hold to discard', holdDuration = 700 }: Props = $props();

  let isHolding = $state(false);
  let progress = $state(0);
  let holdTimer: number | null = null;
  let startTime: number | null = null;
  let animationFrame: number | null = null;

  function updateProgress() {
    if (!startTime) return;

    const elapsed = Date.now() - startTime;
    progress = Math.min(elapsed / holdDuration, 1);

    if (progress >= 1) {
      // Complete - trigger discard
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
  {#if isHolding}
    <div class="progress-bar">
      <div class="progress-fill" style="width: {progress * 100}%"></div>
    </div>
  {:else}
    <span class="icon">×</span>
  {/if}
</button>

<style>
  .hold-to-discard {
    height: 100%;
    border: 1px solid var(--border-primary);
    border-right: none;
    border-radius: 3px 0 0 3px;
    background: var(--bg-secondary);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    padding: 0 calc(var(--size-base) * 0.4);
    transition:
      width 0.15s ease,
      color 0.1s ease,
      background-color 0.1s ease;
    width: auto;
  }

  .hold-to-discard:not(.holding) {
    color: var(--text-muted);
  }

  .hold-to-discard:not(.holding):hover {
    background-color: var(--bg-input);
    color: var(--status-deleted);
  }

  .hold-to-discard.holding {
    width: 30px;
    padding: 0;
    background-color: var(--bg-input);
    border: 1px solid var(--status-deleted);
  }

  .icon {
    font-size: var(--size-xl);
    line-height: 1;
  }

  .progress-bar {
    width: 100%;
    height: 100%;
    border-radius: 2px;
    overflow: hidden;
    position: relative;
  }

  .progress-fill {
    height: 100%;
    background-color: var(--status-deleted);
    transition: width 0.05s linear;
  }
</style>
