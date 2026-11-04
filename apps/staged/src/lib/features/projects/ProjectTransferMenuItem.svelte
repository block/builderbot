<!--
  ProjectTransferMenuItem.svelte — the "Transfer to Blox" / "Transfer to
  local" context-menu item shared by the landing grid and the sidebar.
  Whichever location the project is not in; hidden for a project with no
  repos, disabled while the project is deleting or already transferring.
-->
<script lang="ts">
  import CloudUpload from '@lucide/svelte/icons/cloud-upload';
  import Laptop from '@lucide/svelte/icons/laptop';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import type { Project } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectActions } from './projectActions.svelte';
  import { transferActionLabel, transferDirectionFrom } from './projectTransfer';

  interface Props {
    project: Project;
    disabled?: boolean;
  }

  let { project, disabled = false }: Props = $props();

  let direction = $derived(transferDirectionFrom(project.location));
  let hasRepos = $derived((projectsDataStore.repoCountsByProject.get(project.id) ?? 0) > 0);
</script>

{#if hasRepos}
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
