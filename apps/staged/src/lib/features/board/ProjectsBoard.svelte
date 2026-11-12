<!--
  ProjectsBoard.svelte — the home page's Board view: a column per tag in one
  group ("No ‹group›" first), each holding the projects with that tag, then a
  + column that adds a tag to the group.

  It shows the grid's projects (the filter chips, tag chips included, narrow
  them) in the grid's newest-first order, and a search hides cards rather
  than unmounting them, as in the grid. Moving a card between columns changes
  the project's tag in the group: by drag and drop with a fine pointer on
  desktop, else by "Move to" in the card's context menu. Both are optimistic,
  so the card moves at once, and focus follows it to its new column. A move
  the tag chips rule out (onto Done with only In progress's chip on) takes
  the card off the board, and focus goes to its neighbour instead. Collapsed
  and hidden columns are remembered per group (boardViewPrefs). When every
  showing card is in a hidden column (a tag chip for a hidden column's tag,
  say), the board says so over the columns, with a way to show them. When
  the columns run past the window, a fade and chevron mark each side with
  more (BoardScrollEdges).

  On a phone the columns are full-width pages swiped sideways, under
  BoardPhonePager. Mounted inside the home layer like the grid, or inside
  the project layer by a pinned board's view (BoardView), never as a layer
  of its own.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { Project, TagGroup } from '../../types';
  import { tagsStore } from '../../stores/tags.svelte';
  import { viewport } from '../../shared/viewport.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { projectActions } from '../projects/projectActions.svelte';
  import { focusIfDropped, neighbourAfterRemove } from '../projects/focusAfterRemove';
  import type { BoardCardHandlers } from './BoardCard.svelte';
  import BoardAddColumn from './BoardAddColumn.svelte';
  import BoardColumn from './BoardColumn.svelte';
  import BoardEmptyState from './BoardEmptyState.svelte';
  import BoardPhonePager from './BoardPhonePager.svelte';
  import BoardScrollEdges from './BoardScrollEdges.svelte';
  import { buildBoardColumns } from './boardColumns';
  import {
    PROJECT_DRAG_TYPE,
    readProjectDrag,
    resolveBoardDrop,
    writeProjectDrag,
  } from './boardDrop';
  import { columnPrefsFor } from './boardPrefs';
  import { boardViewPrefs, setBoardColumnFlag } from './boardViewPrefs.svelte';

  interface Props {
    /** The group the board is by; null when there are no groups. */
    group: TagGroup | null;
    /** The projects to show, in card order. */
    projects: Project[];
    /** The projects a search leaves showing (its rank), or null when not searching. */
    searchRank: { has(projectId: string): boolean } | null;
    onOpen: (projectId: string) => void;
    /** Whether the phone pager's toolbar offers "Board by"; off when the route fixes the group. */
    groupPicker?: boolean;
  }

  let { group, projects, searchRank, onOpen, groupPicker = true }: Props = $props();

  let boardEl = $state<HTMLElement | null>(null);
  let columnsEl = $state<HTMLElement | null>(null);
  let draggingId = $state<string | null>(null);
  let dropKey = $state<string | null>(null);
  let activeIndex = $state(0);

  let paged = $derived(viewport.isMobile);
  let dragEnabled = $derived(!paged && !viewport.hasCoarsePointer);
  let projectsById = $derived(new Map(projects.map((project) => [project.id, project])));

  function isVisible(projectId: string): boolean {
    return searchRank === null || searchRank.has(projectId);
  }

  let layout = $derived.by(() => {
    if (!group) return null;
    const groupId = group.id;
    return buildBoardColumns({
      group,
      tags: tagsStore.tagsByGroup.get(groupId) ?? [],
      projects,
      groupTagIdOf: (projectId) => tagsStore.groupTagFor(projectId, groupId)?.id ?? null,
      isVisible,
      prefs: columnPrefsFor(boardViewPrefs.columns, groupId, tagsStore.catalog),
    });
  });
  let activeKey = $derived(layout?.columns[activeIndex]?.key ?? null);
  // Nothing on the board, but matches behind hidden columns.
  let allMatchesHidden = $derived(
    !!layout && layout.hiddenCount > 0 && layout.columns.every((column) => column.count === 0)
  );

  function cardElement(projectId: string): HTMLElement | null {
    for (const el of boardEl?.querySelectorAll<HTMLElement>('[data-board-card]') ?? []) {
      if (el.dataset.boardCard === projectId) return el;
    }
    return null;
  }

  /**
   * Put focus on a card that just changed column. WebKit drops focus when a
   * keyed list moves the focused node, and here the card is a new node in
   * another column; the closing menu may also hand focus back to the old,
   * now detached, card a frame later. Called as the move is made, so the
   * card's neighbours can still be read: when the move took it off the
   * board (filtered out, or into a collapsed column), they take focus.
   */
  async function focusCard(projectId: string) {
    const neighbour = neighbourAfterRemove(renderedCardIds, projectId, cardElement);
    await tick();
    const card = cardElement(projectId);
    if (card) {
      card.focus({ preventScroll: true });
      card.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    } else {
      neighbour(null)?.focus({ preventScroll: true });
    }
    requestAnimationFrame(() => focusIfDropped(() => cardElement(projectId) ?? neighbour(null)));
  }

  /** The cards on screen, column by column: the order focus moves through after a remove. */
  function shownCardIds(): string[] {
    if (!layout) return [];
    return layout.columns.flatMap((column) =>
      column.collapsed && !paged
        ? []
        : column.projects.filter((project) => isVisible(project.id)).map((project) => project.id)
    );
  }

  // The cards on screen as last rendered. A move takes its fallback focus
  // target from these rather than from `layout`: the write is optimistic, so
  // by the time "Move to" reports it, `layout` may already have dropped the
  // card for the filter chips, and with it the card's place.
  let renderedCardIds: string[] = [];
  $effect(() => {
    renderedCardIds = shownCardIds();
  });

  const handlers: BoardCardHandlers = {
    get dragEnabled() {
      return dragEnabled;
    },
    get draggingId() {
      return draggingId;
    },
    open: (projectId) => onOpen(projectId),
    startDrag(projectId, event) {
      if (!dragEnabled || !event.dataTransfer) return;
      writeProjectDrag(event.dataTransfer, projectId);
      draggingId = projectId;
    },
    endDrag() {
      draggingId = null;
      dropKey = null;
    },
    remove(project) {
      const focusAfterRemove = neighbourAfterRemove(shownCardIds(), project.id, cardElement);
      void projectActions.requestRemoveProject(project, focusAfterRemove);
    },
    moved: (projectId) => void focusCard(projectId),
  };

  /** Only card drags count: files and text dragged over the board pass through. */
  function isCardDrag(event: DragEvent): boolean {
    return draggingId !== null || !!event.dataTransfer?.types.includes(PROJECT_DRAG_TYPE);
  }

  function handleDragOver(key: string, event: DragEvent) {
    if (!dragEnabled || !isCardDrag(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
    dropKey = key;
  }

  function handleDragLeave(key: string, event: DragEvent) {
    const next = event.relatedTarget;
    if (next instanceof Node && (event.currentTarget as Node).contains(next)) return;
    if (dropKey === key) dropKey = null;
  }

  function handleDrop(key: string, event: DragEvent) {
    if (!dragEnabled || !isCardDrag(event)) return;
    event.preventDefault();
    const projectId = readProjectDrag(event.dataTransfer, draggingId, new Set(projectsById.keys()));
    handlers.endDrag();
    if (!projectId || !group) return;
    const write = resolveBoardDrop(key, group.id, tagsStore.catalog);
    const project = projectsById.get(projectId);
    if (!write || !project) return;
    void tagActions.setProjectGroupTag(project, write.groupId, write.tagId);
    void focusCard(projectId);
  }

  function setCollapsed(key: string, collapsed: boolean) {
    if (!group) return;
    setBoardColumnFlag(group.id, key, 'collapsed', collapsed, tagsStore.catalog);
  }

  function hideColumn(key: string) {
    if (!group) return;
    setBoardColumnFlag(group.id, key, 'hidden', true, tagsStore.catalog);
  }

  function showHiddenColumns() {
    if (!group || !layout) return;
    for (const column of layout.hidden) {
      setBoardColumnFlag(group.id, column.key, 'hidden', false, tagsStore.catalog);
    }
  }

  function handleColumnsScroll() {
    if (!paged || !columnsEl || columnsEl.clientWidth === 0) return;
    activeIndex = Math.round(columnsEl.scrollLeft / columnsEl.clientWidth);
  }

  function pageTo(key: string) {
    const column = [
      ...(columnsEl?.querySelectorAll<HTMLElement>('[data-board-column]') ?? []),
    ].find((el) => el.dataset.boardColumn === key);
    column?.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'start' });
  }

  /** The card element for a project, for the home page's scroll restore. */
  export function cardFor(projectId: string): HTMLElement | null {
    return cardElement(projectId);
  }
</script>

<div class="projects-board" class:paged bind:this={boardEl}>
  {#if !tagsStore.loaded}
    <div class="board-state">Loading tags…</div>
  {:else if !group || !layout}
    <BoardEmptyState />
  {:else}
    {#if paged}
      <BoardPhonePager
        {group}
        {groupPicker}
        columns={layout.columns}
        {activeKey}
        onSelect={pageTo}
      />
    {/if}
    {#if allMatchesHidden}
      <p class="board-hint" role="status">
        {layout.hiddenCount === 1
          ? 'The matching project is in a hidden column.'
          : `All ${layout.hiddenCount} matching projects are in hidden columns.`}
        <button type="button" class="board-hint-action" onclick={showHiddenColumns}>
          Show hidden columns
        </button>
      </p>
    {/if}
    <div class="board-columns-frame">
      <div
        class="board-columns"
        class:paged
        bind:this={columnsEl}
        onscroll={handleColumnsScroll}
        aria-label={`Projects by ${group.name}`}
        role="group"
      >
        {#each layout.columns as column (column.key)}
          <BoardColumn
            {column}
            {group}
            {handlers}
            {isVisible}
            {paged}
            dropTarget={draggingId !== null && dropKey === column.key}
            onDragOver={(event) => handleDragOver(column.key, event)}
            onDragLeave={(event) => handleDragLeave(column.key, event)}
            onDrop={(event) => handleDrop(column.key, event)}
            onCollapse={(collapsed) => setCollapsed(column.key, collapsed)}
            onHide={() => hideColumn(column.key)}
          />
        {/each}
        <BoardAddColumn {group} {paged} />
      </div>
      <BoardScrollEdges scroller={columnsEl} enabled={!paged} />
    </div>
  {/if}
</div>

<style>
  .projects-board {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }

  .board-state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .board-hint {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 10px;
    margin: 0;
    padding: 8px 12px;
    border: 1px dashed var(--border-muted);
    border-radius: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .board-hint-action {
    padding: 0;
    border: none;
    background: none;
    color: var(--text-accent);
    font: inherit;
    cursor: pointer;
  }

  .board-hint-action:hover {
    text-decoration: underline;
  }

  /* Positions BoardScrollEdges' fades over the scroller's sides. */
  .board-columns-frame {
    position: relative;
    min-width: 0;
  }

  .board-columns {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    min-width: 0;
    overflow-x: auto;
    padding-bottom: 8px;
    scrollbar-width: thin;
    scrollbar-color: var(--border-muted) transparent;
  }

  /* One column per page, swiped sideways. */
  .board-columns.paged {
    gap: 0;
    scroll-snap-type: x mandatory;
    overscroll-behavior-x: contain;
    scrollbar-width: none;
  }

  .board-columns.paged::-webkit-scrollbar {
    display: none;
  }
</style>
