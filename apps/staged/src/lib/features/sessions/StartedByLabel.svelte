<!--
  StartedByLabel.svelte - who started a session, when it was not the user

  A muted one-liner at the top of a session's transcript for sessions that
  stagedctl started (`Session.startedBy`): another session, or a connected
  app the user approved. Renders nothing for sessions started in the app.
-->
<script lang="ts">
  import Bot from '@lucide/svelte/icons/bot';
  import { startedByLabel } from './startedBy';

  let { startedBy }: { startedBy: string | null | undefined } = $props();

  const label = $derived(startedByLabel(startedBy));
</script>

{#if label}
  <div class="started-by" data-testid="session-started-by" title={startedBy ?? undefined}>
    <Bot size={13} />
    <span>{label}</span>
  </div>
{/if}

<style>
  .started-by {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }
</style>
