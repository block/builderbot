<script lang="ts">
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import CirclePause from '@lucide/svelte/icons/circle-pause';
  import Spinner from '../../shared/Spinner.svelte';
  import type { WorkspaceStatus } from '../../types';

  interface Props {
    status: WorkspaceStatus;
    workspaceError?: string;
    fallbackError?: string | null;
  }

  let { status, workspaceError, fallbackError = null }: Props = $props();
</script>

{#if status === 'starting'}
  <div class="status-view starting-view">
    <Spinner size={20} />
    <span class="status-text">Provisioning workspace…</span>
    <span class="status-hint">Cloning repo and creating branch…</span>
  </div>
{:else if status === 'stopped'}
  <div class="status-view stopped-view">
    <CirclePause size={20} />
    <span class="status-text">Workspace stopped</span>
    <span class="status-hint">Start a new branch to reprovision this workspace.</span>
  </div>
{:else if status === 'suspended'}
  <div class="status-view suspended-view">
    <CirclePause size={20} />
    <span class="status-text">Workspace suspended</span>
    <span class="status-hint">Resume this workspace from the project header.</span>
  </div>
{:else if status === 'error'}
  <div class="status-view error-view">
    <AlertCircle size={20} />
    <span class="status-text">Workspace error</span>
    <span class="status-hint">{workspaceError ?? fallbackError ?? 'Something went wrong.'}</span>
  </div>
{/if}

<style>
  .status-view {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-height: 100px;
    text-align: center;
    color: var(--text-muted);
  }

  .status-view .status-text {
    font-size: var(--size-sm);
    color: var(--text-primary);
    font-weight: 600;
  }

  .status-view .status-hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .starting-view :global(svg) {
    color: var(--ui-info);
  }

  .stopped-view :global(svg) {
    color: var(--text-muted);
  }

  .suspended-view :global(svg) {
    color: var(--ui-warning);
  }

  .error-view :global(svg) {
    color: var(--ui-danger);
  }
</style>
