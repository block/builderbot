<!--
  ProjectCardStatusIndicator.svelte — what a project card shows in its top
  right corner while something is happening: a wave while a run action runs
  (with a spinner when sessions run too), a spinner while sessions run or a
  workspace provisions, a cloud for a lost Blox connection, or the unread dot.
  Nothing when idle. Shared by the home grid's cards and the board's; the
  card must be positioned.
-->
<script lang="ts">
  import { fade } from 'svelte/transition';
  import CloudOff from '@lucide/svelte/icons/cloud-off';
  import { disconnectedSessionsStore } from '../../stores/disconnectedSessions.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { disconnectedLabel } from '../../shared/disconnectedSession';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import type { ProjectStatus } from './projectStatus';
  import { transferringTitle } from './projectTransfer';

  interface Props {
    projectId: string;
    status: ProjectStatus;
  }

  let { projectId, status }: Props = $props();

  let busy = $derived(
    status.kind === 'running' || status.kind === 'backgroundHold' || status.kind === 'transferring'
  );
  let busyTitle = $derived(
    status.kind === 'backgroundHold'
      ? 'Waiting on background work'
      : status.kind === 'transferring'
        ? transferringTitle(projectsDataStore.transferringProjects.get(projectId))
        : undefined
  );
</script>

{#if status.runActionPhase === 'running' && busy}
  <div
    class="status-indicator wave-spinner"
    title={busyTitle}
    in:fade={{ duration: 300, delay: 150 }}
    out:fade={{ duration: 150 }}
  >
    <SineWave size={14} />
    <Spinner size={14} />
  </div>
{:else if status.runActionPhase === 'running'}
  <div
    class="status-indicator wave"
    in:fade={{ duration: 300, delay: 150 }}
    out:fade={{ duration: 150 }}
  >
    <SineWave size={14} />
  </div>
{:else if status.kind === 'runAction' || busy}
  <div
    class="status-indicator spinner"
    title={busyTitle}
    in:fade={{ duration: 300, delay: 150 }}
    out:fade={{ duration: 150 }}
  >
    <Spinner size={14} />
  </div>
{:else if status.kind === 'disconnected'}
  {@const label = disconnectedLabel(disconnectedSessionsStore.projectReconcileState(projectId))}
  <div
    class="status-indicator disconnected"
    title={label}
    aria-label={label}
    in:fade={{ duration: 300, delay: 150 }}
    out:fade={{ duration: 150 }}
  >
    <CloudOff size={14} />
  </div>
{:else if status.kind === 'unread'}
  <div
    class="status-indicator unread-dot"
    in:fade={{ duration: 300, delay: 150 }}
    out:fade={{ duration: 150 }}
  ></div>
{/if}

<style>
  .status-indicator {
    position: absolute;
    top: 10px;
    right: 10px;
    z-index: 5;
  }

  .status-indicator.spinner {
    color: var(--ui-accent);
  }

  .status-indicator.wave {
    color: var(--ui-accent);
  }

  .status-indicator.wave-spinner {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--ui-accent);
  }

  .status-indicator.disconnected {
    display: flex;
    color: var(--ui-warning);
  }

  .status-indicator.unread-dot {
    width: 8px;
    height: 8px;
    background-color: var(--ui-accent);
    border-radius: 50%;
  }
</style>
