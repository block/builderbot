<!--
  ProjectTransferBanner.svelte — a project's transfer in progress, in
  ProjectHome: the direction, the phase (and the repo it is about), the
  backend's latest detail line, and Cancel while a cancel can still be
  honoured (before the database switch).
-->
<script lang="ts">
  import Spinner from '../../shared/Spinner.svelte';
  import { Button } from '$lib/components/ui/button';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectActions } from './projectActions.svelte';
  import {
    isTransferCancellable,
    transferDestinationLabel,
    transferPhaseLabel,
  } from './projectTransfer';

  interface Props {
    projectId: string;
  }

  let { projectId }: Props = $props();

  let transfer = $derived(projectsDataStore.getProjectTransfer(projectId));
  let repoName = $derived.by(() => {
    const repoId = transfer?.projectRepoId;
    if (!repoId) return null;
    const repo = projectsDataStore.reposByProject.get(projectId)?.find((r) => r.id === repoId);
    if (!repo) return null;
    return repo.subpath ? `${repo.githubRepo}/${repo.subpath}` : repo.githubRepo;
  });

  // Local: the banner unmounts with the transfer, so a new one starts fresh.
  let cancelling = $state(false);

  async function cancel() {
    cancelling = true;
    const wasRunning = await projectActions.cancelTransfer(projectId);
    if (!wasRunning) cancelling = false;
  }
</script>

{#if transfer}
  <div class="transfer-banner" role="status" aria-live="polite">
    <Spinner size={14} />
    <div class="transfer-text">
      <span class="transfer-title">
        Transferring to {transferDestinationLabel(transfer.direction)}
      </span>
      <span class="transfer-phase">
        {transferPhaseLabel(transfer.phase, transfer.direction, repoName, transfer.bytes)}
      </span>
      {#if transfer.detail}
        <span class="transfer-detail" title={transfer.detail}>{transfer.detail}</span>
      {/if}
    </div>
    {#if isTransferCancellable(transfer.phase)}
      <Button variant="outline" size="sm" onclick={cancel} disabled={cancelling}>
        {cancelling ? 'Cancelling…' : 'Cancel'}
      </Button>
    {/if}
  </div>
{/if}

<style>
  .transfer-banner {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    border: 1px solid var(--ui-accent);
    border-radius: 8px;
    background: var(--bg-primary);
    color: var(--text-primary);
    /* Sits in .projects-list's 32px gap; close the gap to its section. */
    margin-bottom: -20px;
  }

  .transfer-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .transfer-title {
    font-size: var(--size-sm);
    font-weight: 600;
  }

  .transfer-phase {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .transfer-detail {
    font-size: var(--size-xs);
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
