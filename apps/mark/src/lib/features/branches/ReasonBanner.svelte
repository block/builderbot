<script lang="ts">
  import { Info, X } from 'lucide-svelte';

  interface Props {
    reason: string | null | undefined;
    onDismiss: () => void;
  }

  let { reason, onDismiss }: Props = $props();

  let dismissed = $state(false);

  function handleDismiss() {
    dismissed = true;
    onDismiss();
  }
</script>

{#if reason && !dismissed}
  <div class="reason-banner">
    <Info size={13} class="reason-icon" />
    <span class="reason-text">{reason}</span>
    <button class="reason-dismiss" onclick={handleDismiss} title="Dismiss">
      <X size={12} />
    </button>
  </div>
{/if}

<style>
  .reason-banner {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 8px 10px;
    margin-bottom: 10px;
    border-radius: 6px;
    background-color: color-mix(in srgb, var(--ui-info, #3b82f6) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--ui-info, #3b82f6) 25%, transparent);
  }

  .reason-banner :global(.reason-icon) {
    color: var(--ui-info, #3b82f6);
    flex-shrink: 0;
    margin-top: 1px;
  }

  .reason-text {
    flex: 1;
    font-size: var(--size-xs);
    color: var(--text-primary);
    line-height: 1.4;
    min-width: 0;
  }

  .reason-dismiss {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    padding: 0;
    flex-shrink: 0;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      color 0.1s,
      background-color 0.1s;
  }

  .reason-banner:hover .reason-dismiss {
    opacity: 1;
  }

  .reason-dismiss:hover {
    color: var(--text-primary);
    background-color: color-mix(in srgb, var(--ui-info, #3b82f6) 15%, transparent);
  }
</style>
