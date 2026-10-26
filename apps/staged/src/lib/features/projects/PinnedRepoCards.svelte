<!--
  PinnedRepoCards.svelte - The sidebar's stack of RepoCards with drag-to-reorder.

  Renders the given repos as full RepoCards (Add project, action pills, the ⋮
  menu with pin/unpin). While `reorderable`, cards are drag handles and a drop
  reorders optimistically before the persisted pin order round-trips; the
  sidebar turns this off while a search is showing a mixed pinned/unpinned
  subset, where "reorder" has no meaning.
-->
<script lang="ts">
  import type { RepoHomeItem } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import * as commands from '../../api/commands';
  import RepoCard from './RepoCard.svelte';

  interface Props {
    repos: RepoHomeItem[];
    reorderable?: boolean;
  }

  let { repos, reorderable = false }: Props = $props();

  // Local copy so a drag reorder applies optimistically; re-synced whenever
  // the input list changes.
  let ordered = $state<RepoHomeItem[]>([]);
  let dragSourceIndex = $state<number | null>(null);

  $effect(() => {
    ordered = repos;
  });

  function handleDragStart(index: number) {
    return (e: DragEvent) => {
      dragSourceIndex = index;
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = 'move';
      }
    };
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = 'move';
    }
  }

  function handleDrop(index: number) {
    return async (_e: DragEvent) => {
      if (dragSourceIndex === null || dragSourceIndex === index) {
        dragSourceIndex = null;
        return;
      }

      const items = [...ordered];
      const [moved] = items.splice(dragSourceIndex, 1);
      items.splice(index, 0, moved);
      ordered = items;
      dragSourceIndex = null;

      const orderedKeys: [string, string][] = items.map((r) => [r.githubRepo, r.subpath]);
      try {
        await commands.reorderPinnedRepos(orderedKeys);
      } catch (e) {
        console.error('[PinnedRepoCards] Failed to reorder pinned repos:', e);
        // Reload to get the correct order
        await projectsDataStore.refreshHomeRepos();
      }
    };
  }

  function handleDragEnd() {
    dragSourceIndex = null;
  }
</script>

<div class="pinned-repos-list" role="list" aria-label="Pinned repos">
  {#each ordered as repo, index (repo.githubRepo + '\t' + repo.subpath)}
    <RepoCard
      {repo}
      hidePinButton
      {reorderable}
      onReorderStart={handleDragStart(index)}
      onReorderOver={handleDragOver}
      onReorderDrop={handleDrop(index)}
      onReorderEnd={handleDragEnd}
      onChange={() => projectsDataStore.refreshHomeRepos()}
    />
  {/each}
</div>

<style>
  .pinned-repos-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 2px 0;
  }
</style>
