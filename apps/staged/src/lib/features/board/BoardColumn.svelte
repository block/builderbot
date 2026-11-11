<!--
  BoardColumn.svelte — one board column: its tag's icon, name and card count,
  a ⋯ menu, and the cards. A drop zone while a card is dragged.

  Collapsed, it is a narrow strip with the name running down it and the
  count; clicking it expands the column, and a card dragged over it tints
  the strip itself (the column's thin accent border is too little to read
  on a 40px strip). "Rename" in the menu edits the
  name in the header: Enter or leaving the field saves, Escape puts it back.
-->
<script lang="ts">
  import CircleDashed from '@lucide/svelte/icons/circle-dashed';
  import { tick } from 'svelte';
  import type { TagGroup } from '../../types';
  import TagIcon from '../tags/TagIcon.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { MAX_TAG_NAME_CHARS } from '../tags/tagState';
  import BoardCard, { type BoardCardHandlers } from './BoardCard.svelte';
  import BoardColumnMenu from './BoardColumnMenu.svelte';
  import type { BoardColumn } from './boardColumns';

  interface Props {
    column: BoardColumn;
    group: TagGroup;
    handlers: BoardCardHandlers;
    isVisible: (projectId: string) => boolean;
    /** A dragged card is over this column. */
    dropTarget: boolean;
    /** On the phone pager: full width, never collapsed. */
    paged: boolean;
    onDragOver: (event: DragEvent) => void;
    onDragLeave: (event: DragEvent) => void;
    onDrop: (event: DragEvent) => void;
    onCollapse: (collapsed: boolean) => void;
    onHide: () => void;
  }

  let {
    column,
    group,
    handlers,
    isVisible,
    dropTarget,
    paged,
    onDragOver,
    onDragLeave,
    onDrop,
    onCollapse,
    onHide,
  }: Props = $props();

  let collapsed = $derived(column.collapsed && !paged);
  let renaming = $state(false);
  let nameInput = $state<HTMLInputElement | null>(null);

  async function startRename() {
    if (!column.tag) return;
    renaming = true;
    await tick();
    nameInput?.focus();
    nameInput?.select();
  }

  async function commitName(input: HTMLInputElement) {
    const tag = column.tag;
    renaming = false;
    const name = input.value.trim();
    if (!tag || !name || name === tag.name) return;
    await tagActions.editTag(tag.id, { name });
  }

  function handleNameKeydown(event: KeyboardEvent & { currentTarget: HTMLInputElement }) {
    if (event.key === 'Enter') {
      event.preventDefault();
      event.currentTarget.blur();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.currentTarget.value = column.tag?.name ?? '';
      event.currentTarget.blur();
    }
  }
</script>

{#snippet columnIcon()}
  {#if column.tag}
    <TagIcon tag={column.tag} size={14} />
  {:else}
    <CircleDashed size={14} class="text-muted-foreground" />
  {/if}
{/snippet}

<section
  class="board-column"
  class:collapsed
  class:paged
  class:drop-target={dropTarget}
  data-board-column={column.key}
  aria-label={`${column.name}, ${column.count} ${column.count === 1 ? 'project' : 'projects'}`}
  ondragover={onDragOver}
  ondragleave={onDragLeave}
  ondrop={onDrop}
>
  {#if collapsed}
    <button
      type="button"
      class="collapsed-strip"
      class:drop-target={dropTarget}
      title={`Expand ${column.name}`}
      aria-label={`Expand ${column.name}`}
      onclick={() => onCollapse(false)}
    >
      {@render columnIcon()}
      <span class="collapsed-name">{column.name}</span>
      <span class="column-count">{column.count}</span>
    </button>
  {:else}
    <header class="board-column-head">
      {@render columnIcon()}
      {#if renaming && column.tag}
        <input
          bind:this={nameInput}
          class="column-name-input"
          type="text"
          aria-label="Tag name"
          maxlength={MAX_TAG_NAME_CHARS}
          value={column.tag.name}
          onblur={(event) => void commitName(event.currentTarget)}
          onkeydown={handleNameKeydown}
        />
      {:else}
        <h3 class="column-name" title={column.name}>{column.name}</h3>
      {/if}
      <span class="column-count">{column.count}</span>
      <BoardColumnMenu
        {column}
        canCollapse={!paged}
        onCollapse={() => onCollapse(true)}
        {onHide}
        onRename={() => void startRename()}
      />
    </header>
    <div class="board-column-cards">
      {#each column.projects as project (project.id)}
        <BoardCard {project} {group} visible={isVisible(project.id)} {handlers} />
      {/each}
      {#if column.count === 0}
        <p class="column-empty">No projects</p>
      {/if}
    </div>
  {/if}
</section>

<style>
  .board-column {
    flex: 0 0 272px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
    padding: 8px;
    border: 1px solid transparent;
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-primary) 45%, transparent);
    transition:
      border-color 0.1s ease,
      background-color 0.1s ease;
  }

  .board-column.drop-target {
    border-color: var(--ui-accent);
    background: color-mix(in srgb, var(--ui-accent) 8%, transparent);
  }

  .board-column.paged {
    flex-basis: 100%;
    scroll-snap-align: start;
  }

  .board-column.collapsed {
    flex-basis: 40px;
    padding: 4px;
    align-self: stretch;
  }

  .collapsed-strip {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-height: 160px;
    padding: 8px 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
  }

  .collapsed-strip:hover {
    background: var(--bg-hover);
  }

  /* The drop feedback a card gets from the column's border, made visible on
     the strip: :hover doesn't apply during a drag, and the strip is the
     whole column. */
  .collapsed-strip.drop-target {
    background: color-mix(in srgb, var(--ui-accent) 14%, transparent);
    color: var(--ui-accent);
    box-shadow: inset 0 0 0 1px var(--ui-accent);
  }

  .collapsed-name {
    writing-mode: vertical-rl;
    font-size: var(--size-sm);
    font-weight: 600;
    white-space: nowrap;
  }

  .board-column-head {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    min-height: 26px;
    padding: 0 2px 0 4px;
  }

  .board-column-head :global(svg) {
    flex-shrink: 0;
  }

  .column-name {
    flex: 1;
    min-width: 0;
    margin: 0;
    overflow: hidden;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .column-name-input {
    flex: 1;
    min-width: 0;
    height: 24px;
    padding: 0 6px;
    border: 1px solid var(--border-emphasis);
    border-radius: 4px;
    background: var(--bg-input);
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 600;
    outline: none;
  }

  .column-name-input:focus {
    border-color: var(--ui-accent);
  }

  .column-count {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-variant-numeric: tabular-nums;
  }

  /* Tall enough to drop on when empty. */
  .board-column-cards {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-height: 48px;
  }

  .column-empty {
    margin: 0;
    padding: 12px 4px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    text-align: center;
  }
</style>
