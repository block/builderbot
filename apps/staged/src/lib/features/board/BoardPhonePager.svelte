<!--
  BoardPhonePager.svelte — the board's header on a phone, where columns are
  full-width pages swiped sideways: the board's toolbar, then a strip of the
  columns' names and counts. Tapping a name pages to that column; the strip
  marks the column on screen.
-->
<script lang="ts">
  import type { TagGroup } from '../../types';
  import BoardToolbar from './BoardToolbar.svelte';
  import type { BoardColumn } from './boardColumns';

  interface Props {
    group: TagGroup;
    /** Passed to the toolbar: off where the route fixes the group. */
    groupPicker?: boolean;
    columns: BoardColumn[];
    /** The column on screen. */
    activeKey: string | null;
    onSelect: (key: string) => void;
  }

  let { group, groupPicker = true, columns, activeKey, onSelect }: Props = $props();
</script>

<div class="board-pager">
  <BoardToolbar {group} {groupPicker} />
  <div class="pager-strip" aria-label={`${group.name} columns`}>
    {#each columns as column (column.key)}
      <button
        type="button"
        class="pager-tab touch-hit"
        class:active={column.key === activeKey}
        aria-current={column.key === activeKey ? 'true' : undefined}
        onclick={() => onSelect(column.key)}
      >
        <span class="pager-name">{column.name}</span>
        <span class="pager-count">{column.count}</span>
      </button>
    {/each}
  </div>
</div>

<style>
  .board-pager {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .pager-strip {
    display: flex;
    gap: 4px;
    overflow-x: auto;
    scrollbar-width: none;
  }

  .pager-strip::-webkit-scrollbar {
    display: none;
  }

  .pager-tab {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    max-width: 180px;
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    background: transparent;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
  }

  .pager-tab.active {
    border-color: var(--border-emphasis);
    background: var(--bg-primary);
    color: var(--text-primary);
  }

  .pager-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pager-count {
    flex-shrink: 0;
    font-size: var(--size-xs);
    font-variant-numeric: tabular-nums;
  }
</style>
