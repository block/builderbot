<!--
  BoardColumn.svelte — one board column: its tag's icon, name and card count,
  a ⋯ menu, and the cards. A drop zone while a card is dragged, and while a
  column is: the board tells the two apart.

  Collapsed, it is a narrow strip with the name running down it and the
  count; clicking it expands the column, and a card dragged over it tints
  the strip itself (the column's thin accent border is too little to read
  on a 40px strip). "Rename" in the menu edits the
  name in the header: Enter or leaving the field saves, Escape puts it back.
  "Move left" and "Move right" there are the board's to carry out.

  With drag and drop on, a tag column drags among the others by its header,
  or by its strip when collapsed. A line in the gap beside the column under
  the pointer shows where it would land. The first column in the row has no
  gap before it, so its line sits on its own edge. The header does not drag
  while its name is being edited, so the pointer can select text in the
  field, and "No ‹group›" does not drag at all.
-->
<script module lang="ts">
  /** What every column on a board shares for dragging columns, from ProjectsBoard. */
  export interface BoardColumnDragHandlers {
    /** Tag columns drag among the others. */
    readonly enabled: boolean;
    /** The column being dragged, if any. */
    readonly draggingKey: string | null;
    /** The side of a column the dragged one would land on, if it would land beside it. */
    placeFor(columnKey: string): 'before' | 'after' | null;
    start(columnKey: string, event: DragEvent): void;
    end(): void;
  }
</script>

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
    columnDrag: BoardColumnDragHandlers;
    isVisible: (projectId: string) => boolean;
    /** A dragged card is over this column. */
    dropTarget: boolean;
    /** On the phone pager: full width, never collapsed. */
    paged: boolean;
    /** First in the row of columns: nothing before it but the row's edge. */
    first: boolean;
    /** Whether the column can move a step that way (never for "No ‹group›"). */
    canMoveLeft: boolean;
    canMoveRight: boolean;
    onDragOver: (event: DragEvent) => void;
    onDragLeave: (event: DragEvent) => void;
    onDrop: (event: DragEvent) => void;
    onCollapse: (collapsed: boolean) => void;
    /** Move the column one step left (-1) or right (1). */
    onMove: (step: -1 | 1) => void;
    onHide: () => void;
  }

  let {
    column,
    group,
    handlers,
    columnDrag,
    isVisible,
    dropTarget,
    paged,
    first,
    canMoveLeft,
    canMoveRight,
    onDragOver,
    onDragLeave,
    onDrop,
    onCollapse,
    onMove,
    onHide,
  }: Props = $props();

  let collapsed = $derived(column.collapsed && !paged);
  let renaming = $state(false);
  let nameInput = $state<HTMLInputElement | null>(null);
  let draggable = $derived(columnDrag.enabled && column.tag !== null && !renaming);
  let dropPlace = $derived(columnDrag.placeFor(column.key));

  /** The header or strip itself: a selection dragged out of the name is text, not the column. */
  function handleColumnDragStart(event: DragEvent) {
    if (!draggable || event.target !== event.currentTarget) return;
    columnDrag.start(column.key, event);
  }

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
  class:first-in-row={first}
  class:drop-target={dropTarget}
  class:dragging={columnDrag.draggingKey === column.key}
  class:drop-before={dropPlace === 'before'}
  class:drop-after={dropPlace === 'after'}
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
      {draggable}
      ondragstart={handleColumnDragStart}
      ondragend={() => columnDrag.end()}
      onclick={() => onCollapse(false)}
    >
      {@render columnIcon()}
      <span class="collapsed-name">{column.name}</span>
      <span class="column-count">{column.count}</span>
    </button>
  {:else}
    <!-- Dragging is the pointer's way to move a column. The ⋯ menu's Move
         left and Move right are the keyboard's, so the header takes no role
         or tab stop for it. -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header
      class="board-column-head"
      class:draggable
      {draggable}
      ondragstart={handleColumnDragStart}
      ondragend={() => columnDrag.end()}
    >
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
        {canMoveLeft}
        {canMoveRight}
        onCollapse={() => onCollapse(true)}
        {onMove}
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
    position: relative;
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
      background-color 0.1s ease,
      opacity 0.15s ease;
  }

  .board-column.dragging {
    opacity: 0.4;
  }

  /* Where a dragged column would land: a line midway across the gap to the
     next column (the row's 12px, past this column's 1px border). The column
     after one and the column before the next draw it in the same place. */
  .board-column.drop-before::before,
  .board-column.drop-after::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    border-radius: 1px;
    background: var(--ui-accent);
    pointer-events: none;
  }

  .board-column.drop-before::before {
    left: -8px;
  }

  /* The first column has no gap before it. It starts at the row's edge, and
     the row scrolls, so it clips what is drawn past that edge: a line at
     -8px would not show. This happens once "No ‹group›" is hidden and a tag
     column is first. Its line sits on the column's own edge instead (-1px
     is its border). */
  .board-column.first-in-row.drop-before::before {
    left: -1px;
  }

  .board-column.drop-after::after {
    right: -8px;
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

  /* A press on the name drags the column, not a selection of its text (as a
     reorderable RepoCard). */
  .board-column-head.draggable {
    cursor: grab;
    user-select: none;
  }

  .board-column-head.draggable:active {
    cursor: grabbing;
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
