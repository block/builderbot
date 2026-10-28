<!--
  PinnedRepoCards.svelte - The sidebar's stack of RepoCards with drag-to-reorder.

  Renders the given repos as full RepoCards (Add project, action pills, the ⋮
  menu with pin/unpin). While `reorderable`, cards are drag handles and a drop
  reorders optimistically before the persisted pin order round-trips; the
  sidebar turns this off while a search is showing only some of the pinned
  repos, where a drop would persist the subset as the whole pin order.
-->
<script lang="ts">
  import type { RepoHomeItem } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import * as commands from '../../api/commands';
  import { moveRepo, repoKey } from './repoReorder';
  import RepoCard from './RepoCard.svelte';

  interface Props {
    repos: RepoHomeItem[];
    reorderable?: boolean;
  }

  let { repos, reorderable = false }: Props = $props();

  // Tracks the input list, but a drop reassigns it so the reorder applies
  // optimistically (derived override); the next `repos` change re-syncs.
  let ordered = $derived(repos);
  // The dragged card by key, not index: `repos` can change between dragstart
  // and drop, so both ends are resolved against `ordered` at drop time.
  let dragSourceKey = $state<string | null>(null);

  function handleDragStart(repo: RepoHomeItem) {
    return (e: DragEvent) => {
      dragSourceKey = repoKey(repo);
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

  function handleDrop(target: RepoHomeItem) {
    return async (_e: DragEvent) => {
      // Only a pinned-only list may be persisted as the pin order; the parent
      // turns reorderable off while a search shows a filtered subset.
      if (!reorderable) return;
      const sourceKey = dragSourceKey;
      dragSourceKey = null;
      if (sourceKey === null) return;

      // Null when the source or target left the list mid-drag, or they are
      // the same card: nothing to apply, nothing to persist.
      const items = moveRepo(ordered, sourceKey, repoKey(target));
      if (!items) return;
      ordered = items;

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
    dragSourceKey = null;
  }
</script>

<!-- Unlabelled on purpose: the SidebarSectionHead directly above already
     announces "Pinned repos" or "Repos · N results" for the current contents. -->
<div class="pinned-repos-list" role="list">
  {#each ordered as repo (repoKey(repo))}
    <RepoCard
      {repo}
      hidePinButton
      {reorderable}
      onReorderStart={handleDragStart(repo)}
      onReorderOver={handleDragOver}
      onReorderDrop={handleDrop(repo)}
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
