<script lang="ts">
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import CirclePause from '@lucide/svelte/icons/circle-pause';
  import Pause from '@lucide/svelte/icons/pause';
  import Spinner from '../../shared/Spinner.svelte';
  import type { WorkspaceStatus } from '../../types';

  interface Props {
    status: WorkspaceStatus | null;
  }

  let { status }: Props = $props();

  function label(value: WorkspaceStatus): string {
    switch (value) {
      case 'starting':
        return 'Provisioning';
      case 'running':
        return 'Running';
      case 'stopped':
        return 'Stopped';
      case 'suspended':
        return 'Suspended';
      case 'error':
        return 'Error';
    }
  }
</script>

{#if status}
  <div
    class="workspace-status-badge"
    class:starting={status === 'starting'}
    class:running={status === 'running'}
    class:stopped={status === 'stopped'}
    class:suspended={status === 'suspended'}
    class:error={status === 'error'}
  >
    {#if status === 'starting'}
      <Spinner size={12} />
    {:else if status === 'running'}
      <CheckCircle size={12} />
    {:else if status === 'stopped'}
      <CirclePause size={12} />
    {:else if status === 'suspended'}
      <Pause size={12} />
    {:else if status === 'error'}
      <AlertCircle size={12} />
    {/if}
    <span>{label(status)}</span>
  </div>
{/if}

<style>
  .workspace-status-badge {
    height: 22px;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px;
    border-radius: 999px;
    border: 1px solid var(--border-muted);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    line-height: 1;
    white-space: nowrap;
  }

  .workspace-status-badge.starting {
    border-color: var(--ui-info);
    color: var(--ui-info);
  }

  .workspace-status-badge.running {
    border-color: var(--border-muted);
    color: var(--text-primary);
  }

  .workspace-status-badge.stopped {
    border-color: var(--border-muted);
    color: var(--text-muted);
  }

  .workspace-status-badge.suspended {
    border-color: var(--border-muted);
    color: var(--text-muted);
  }

  .workspace-status-badge.error {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }
</style>
