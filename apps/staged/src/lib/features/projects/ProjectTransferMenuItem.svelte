<!--
  ProjectTransferMenuItem.svelte — the "Transfer to Blox" / "Transfer to
  local" context-menu item shared by the landing grid and the sidebar.
  Whichever location the project is not in; hidden when the `sq` CLI is
  unavailable (either direction needs Blox) and for a project known to have no
  repos, disabled while the project is deleting or already transferring.
-->
<script lang="ts">
  import CloudUpload from '@lucide/svelte/icons/cloud-upload';
  import Laptop from '@lucide/svelte/icons/laptop';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { sqState } from '../settings/sq.svelte';
  import { projectActions } from './projectActions.svelte';
  import { transferActionLabel, transferDirectionFrom } from './projectTransfer';

  interface Props {
    project: Project;
    disabled?: boolean;
  }

  let { project, disabled = false }: Props = $props();

  let bloxAvailable = $derived(sqState.loaded && sqState.available);
  let direction = $derived(transferDirectionFrom(project.location));
  // Until the project's repos are fetched the count isn't known (it reads 0
  // for a project without a legacy repo), so show the item and let the
  // preflight say when there is nothing to move.
  let hasRepos = $derived.by(() => {
    const repos = projectsDataStore.reposByProject.get(project.id);
    return repos === undefined || repos.length > 0;
  });
</script>

{#if bloxAvailable && hasRepos}
  <ContextMenu.Item
    disabled={disabled || projectsDataStore.isProjectTransferring(project.id)}
    onSelect={() => projectActions.requestTransferProject(project)}
  >
    {#if direction === 'toRemote'}
      <CloudUpload size={14} />
    {:else}
      <Laptop size={14} />
    {/if}
    {transferActionLabel(direction)}
  </ContextMenu.Item>
{/if}
