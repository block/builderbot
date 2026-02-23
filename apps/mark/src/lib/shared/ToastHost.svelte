<script lang="ts">
  import { fly, fade } from 'svelte/transition';
  import { alerts } from './alerts.svelte';
  import AlertCard from './AlertCard.svelte';
</script>

{#if alerts.toasts.length > 0}
  <div class="toast-host" role="region" aria-label="Notifications">
    {#each alerts.toasts as toast (toast.id)}
      <div class="toast-wrap" in:fly={{ x: 16, duration: 170 }} out:fade={{ duration: 140 }}>
        <AlertCard
          tone={toast.tone}
          title={toast.title}
          message={toast.message}
          dismissible={toast.dismissible}
          onDismiss={() => alerts.dismiss(toast.id)}
        />
      </div>
    {/each}
  </div>
{/if}

<style>
  .toast-host {
    position: fixed;
    right: 20px;
    bottom: 20px;
    z-index: 1400;
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: min(380px, calc(100vw - 24px));
    pointer-events: none;
  }

  .toast-wrap {
    pointer-events: auto;
  }
</style>
