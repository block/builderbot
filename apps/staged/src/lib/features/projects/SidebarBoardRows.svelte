<!--
  SidebarBoardRows.svelte - The pinned boards in the projects sidebar.

  One row per tag group whose board is pinned (Settings › Tags › "Show board
  in sidebar"), in group order, under the Projects heading and above the
  project rows. A row opens that group's board as the detail view and is
  marked active while the project layer shows it. The rows follow the tag
  catalog, so pinning or unpinning in another window adds or drops a row
  through the store's `tags-changed` refetch.
-->
<script lang="ts">
  import SquareKanban from '@lucide/svelte/icons/square-kanban';
  import { tagsStore } from '../../stores/tags.svelte';

  interface Props {
    /** The group whose board the project layer shows, if any. */
    activeGroupId: string | null;
    onSelect: (groupId: string) => void;
  }

  let { activeGroupId, onSelect }: Props = $props();

  let groups = $derived(tagsStore.sidebarGroups);
</script>

{#if groups.length > 0}
  <div class="board-rows" aria-label="Boards">
    {#each groups as group (group.id)}
      <button
        type="button"
        class="board-row"
        class:active={activeGroupId === group.id}
        aria-current={activeGroupId === group.id ? 'page' : undefined}
        data-board-group={group.id}
        title={`${group.name} board`}
        onclick={() => onSelect(group.id)}
      >
        <SquareKanban size={14} />
        <span class="board-name">{group.name}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .board-rows {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 4px;
  }

  /* Reads like a project row (ProjectsSidebar): the same bleed, hover and
     active surfaces, one line tall. */
  .board-row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: calc(100% + (2 * var(--project-row-bleed, 8px)));
    margin: 0 calc(-1 * var(--project-row-bleed, 8px));
    min-height: 32px;
    border: none;
    border-radius: 0;
    background-color: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 6px 10px;
    text-align: left;
    font-size: var(--size-sm);
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .board-row :global(svg) {
    flex-shrink: 0;
    stroke: var(--text-muted);
  }

  .board-row:hover {
    color: var(--text-primary);
    background-color: var(--projects-sidebar-hover-bg, var(--bg-hover));
  }

  .board-row.active {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .board-row:hover :global(svg),
  .board-row.active :global(svg) {
    stroke: var(--text-primary);
  }

  .board-row:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: -1px;
  }

  .board-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
