<!--
  ProjectsBoard.svelte — the home page's Board view: a column per tag in one
  group ("No ‹group›" first), each holding the projects with that tag, then a
  + column that adds a tag to the group.

  It shows the grid's projects (the filter chips, tag chips included, narrow
  them) in the grid's order, which is the window's sort (projectOrder) and
  newest first by default, and a search hides cards rather than unmounting
  them, as in the grid. A sort key for this board's own group ties inside
  every column, so the next key orders the cards. Moving a card between columns changes
  the project's tag in the group: by dragging it with a fine pointer on
  desktop, else by "Move to" in the card's context menu. Both are the same
  optimistic write (tagActions.setProjectGroupTag), so the card moves at
  once, and goes back with a toast if the write fails; focus follows it to
  its new column. A move the tag chips rule out (onto Done with only In
  progress's chip on) takes the card off the board, and focus goes to its
  neighbour instead.

  The columns are all as tall as the board, which is at least as tall as the
  tallest of them and reaches the bottom of the page it is on: a column with
  one card is as much to aim at as one with twenty.

  While a card is dragged (boardCardDrag) a copy of it follows the pointer
  and the column it is in line with is marked as the drop target, anywhere
  down the column's height; the gap between two columns is the nearer one's,
  and the page's margin under the board is the board. "No ‹group›" clears
  the tag and a collapsed column takes the card like any other; the card's
  own column changes nothing, and the + column refuses it, with a no-entry
  cursor. Nothing on the board answers the pointer while a card is in hand.
  A card held near either side scrolls the columns that way.

  Collapsed and hidden columns are remembered per group (boardViewPrefs).
  When every showing card is in a hidden column (a tag chip for a hidden
  column's tag, say), the board says so over the columns, with a way to show
  them. When the columns run past the window, a fade and chevron mark each
  side with more (BoardScrollEdges).

  A tag column moves left or right from its ⋯ menu, which reorders the
  group's tags. The move shows before its write lands (the column order is
  read through pendingCatalogMoves), steps over hidden columns, and leaves
  focus on the column's menu, with the column kept on screen, so it can be
  repeated. "No ‹group›" is not a tag and stays first.

  Where cards drag, a tag column drags too, by its header or its collapsed
  strip, and lands on the side of the column it is dropped on: the left half
  before it, the right half after, and after "No ‹group›" on either. Only
  how far across the row the pointer is decides which column that is: a
  header is at the row's top edge, and a pointer that drifts above it is
  still dragging. A line in the gap shows where the column would land, and
  a pointer in a gap lands the column there. Past the last column it lands
  last. Clearly away from the row (see `DRAG_REACH`) there is no line and a
  release does nothing. A drop that would leave the columns in their order
  shows no line and writes nothing, and the cursor says it is refused. Near
  either end of the row, or just past it, the drag scrolls it sideways. A
  chip with the column's icon and name follows the pointer.

  Both drags are made of pointer events (boardCardDrag for a card,
  shared/pointerDrag for a column), because the desktop build's native
  file-drop handler answers every HTML5 drag itself and the page never sees
  one as a drop. So neither carries data: the board reads what a card is
  over from the element under the pointer, and resolves each point a column
  reaches from the columns' rects.

  On a phone the columns are full-width pages swiped sideways, under
  BoardPhonePager. Mounted inside the home layer like the grid, or inside
  the project layer by a pinned board's view (BoardView), never as a layer
  of its own.
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import type { Project, TagGroup } from '../../types';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { tagsStore } from '../../stores/tags.svelte';
  import type { PointerDragPoint } from '../../shared/pointerDrag';
  import { viewport } from '../../shared/viewport.svelte';
  import { pendingCatalogMoves } from '../tags/pendingCatalogMoves.svelte';
  import { tagActions } from '../tags/tagActions.svelte';
  import { resolveTagDrop } from '../tags/tagReorder';
  import { projectActions } from '../projects/projectActions.svelte';
  import { focusIfDropped, neighbourAfterRemove } from '../projects/focusAfterRemove';
  import type { BoardCardHandlers } from './BoardCard.svelte';
  import BoardAddColumn from './BoardAddColumn.svelte';
  import BoardColumn, { type BoardColumnDragHandlers } from './BoardColumn.svelte';
  import BoardDragGhost from './BoardDragGhost.svelte';
  import BoardEmptyState from './BoardEmptyState.svelte';
  import BoardPhonePager from './BoardPhonePager.svelte';
  import BoardScrollEdges from './BoardScrollEdges.svelte';
  import {
    createCardDrag,
    createEdgeScroll,
    dropTargetAt,
    type BoardDropTarget,
    type DragPoint,
  } from './boardCardDrag';
  import { columnDropTarget, columnStepTarget, type ColumnDrop } from './boardColumnReorder';
  import { buildBoardColumns } from './boardColumns';
  import { columnAtPoint, judgeBoardDrop, type BoardDropVerdict } from './boardDrop';
  import { columnPrefsFor } from './boardPrefs';
  import { revealOffset, SCROLL_FADE_WIDTH } from './boardScroll';
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
  /** The columns' scroller. */
  let columnsEl = $state<HTMLElement | null>(null);
  /** The column being dragged, if any. */
  let draggingColumnKey = $state<string | null>(null);
  /** Where the dragged column would land: beside a column, on one side of it. */
  let columnDrop = $state<ColumnDrop | null>(null);
  /** What holds the scroller and lays the scroll edges over it. */
  let frameEl = $state<HTMLElement | null>(null);
  let activeIndex = $state(0);

  /** A card in hand. */
  interface CardInHand {
    projectId: string;
    /** The card's element, copied for the ghost. */
    card: HTMLElement;
    width: number;
    /** From the card's top-left corner to where it was grabbed. */
    grab: DragPoint;
    point: DragPoint;
    /** What the pointer is over. */
    target: BoardDropTarget | null;
  }
  let drag = $state.raw<CardInHand | null>(null);
  let draggingId = $derived(drag?.projectId ?? null);

  let paged = $derived(viewport.isMobile);
  let dragEnabled = $derived(!paged && !viewport.hasCoarsePointer);
  let projectsById = $derived(new Map(projects.map((project) => [project.id, project])));

  function isVisible(projectId: string): boolean {
    return searchRank === null || searchRank.has(projectId);
  }

  // The columns come from the catalog with the reorders in flight applied,
  // so a moved column shows at once and the next move steps from there.
  let columnPrefs = $derived(
    group ? columnPrefsFor(boardViewPrefs.columns, group.id, pendingCatalogMoves.catalog) : null
  );
  let layout = $derived.by(() => {
    if (!group || !columnPrefs) return null;
    const groupId = group.id;
    return buildBoardColumns({
      group,
      tags: pendingCatalogMoves.catalog.tagsByGroup.get(groupId) ?? [],
      projects,
      groupTagIdOf: (projectId) => tagsStore.groupTagFor(projectId, groupId)?.id ?? null,
      isVisible,
      prefs: columnPrefs,
    });
  });
  let activeKey = $derived(layout?.columns[activeIndex]?.key ?? null);

  // The column the dragged card is in now, and what letting go of it would
  // do: both read live, so a tag deleted or a card moved mid-drag (another
  // window, an agent) changes the answer before the drop, not after.
  let homeKey = $derived(
    draggingId === null
      ? null
      : (layout?.columns.find((column) => column.projects.some(({ id }) => id === draggingId))
          ?.key ?? null)
  );
  let verdict = $derived<BoardDropVerdict>(
    drag && group
      ? judgeBoardDrop(drag.target, homeKey, group.id, tagsStore.catalog)
      : { kind: 'none' }
  );
  let dropKey = $derived(verdict.kind === 'move' ? verdict.key : null);
  let refused = $derived(verdict.kind === 'refused');
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

  function targetAt(point: DragPoint): BoardDropTarget | null {
    return columnsEl ? dropTargetAt(point, columnsEl) : null;
  }

  const cardDrag = createCardDrag({
    begin(projectId, card, press) {
      // A project on its way out keeps its tags: its card stays put.
      if (!dragEnabled || !group || !projectsById.has(projectId)) return false;
      if (projectsDataStore.isProjectDeleting(projectId)) return false;
      const box = card.getBoundingClientRect();
      drag = {
        projectId,
        card,
        width: box.width,
        grab: { x: press.x - box.left, y: press.y - box.top },
        point: press,
        target: null,
      };
      return true;
    },
    move(point) {
      if (!drag) return;
      drag = { ...drag, point, target: targetAt(point) };
      edgeScroll.follow();
    },
    end(point) {
      if (!drag) return;
      const { projectId } = drag;
      // Judged where it was let go, against the board as it is now.
      const outcome =
        point && group
          ? judgeBoardDrop(targetAt(point), homeKey, group.id, tagsStore.catalog)
          : null;
      drag = null;
      const project = projectsById.get(projectId);
      // Anywhere but another column, its own included, the card goes back:
      // nothing is written and focus stays where it was.
      if (outcome?.kind !== 'move' || !project) return;
      void tagActions.setProjectGroupTag(project, outcome.write.groupId, outcome.write.tagId);
      void focusCard(projectId);
    },
  });

  // A card held near a side of the board scrolls the columns that way. The
  // drag hears the scroll, and reads again what is under the still pointer.
  const edgeScroll = createEdgeScroll({
    scroller: () => columnsEl,
    point: () => drag?.point ?? null,
  });

  // The drag owns the cursor and keeps the pointer from selecting text on
  // its way across the page.
  $effect(() => {
    if (draggingId === null) return;
    const style = document.body.style;
    const before = {
      cursor: style.cursor,
      userSelect: style.getPropertyValue('user-select'),
      webkitUserSelect: style.getPropertyValue('-webkit-user-select'),
    };
    style.cursor = refused ? 'not-allowed' : 'grabbing';
    style.setProperty('user-select', 'none');
    style.setProperty('-webkit-user-select', 'none');
    return () => {
      style.cursor = before.cursor;
      style.setProperty('user-select', before.userSelect);
      style.setProperty('-webkit-user-select', before.webkitUserSelect);
    };
  });

  // The card in hand left the board (deleted, filtered out), or the board
  // stopped taking drags (the window narrowed to the phone layout).
  $effect(() => {
    if (draggingId === null) return;
    if (!dragEnabled || !group || !projectsById.has(draggingId)) untrack(() => cardDrag.cancel());
  });

  $effect(() => () => {
    cardDrag.cancel();
    edgeScroll.stop();
  });

  const handlers: BoardCardHandlers = {
    get draggingId() {
      return draggingId;
    },
    open: (projectId) => onOpen(projectId),
    press(projectId, event) {
      if (dragEnabled) cardDrag.press(projectId, event);
    },
    remove(project) {
      const focusAfterRemove = neighbourAfterRemove(shownCardIds(), project.id, cardElement);
      void projectActions.requestRemoveProject(project, focusAfterRemove);
    },
    moved: (projectId) => void focusCard(projectId),
  };

  /**
   * Whether the dragged column would move if dropped where it is. The line
   * shows only then.
   */
  let columnDropWouldMove = $derived(
    draggingColumnKey !== null &&
      columnDrop !== null &&
      columnMove(draggingColumnKey, columnDrop) !== null
  );

  /**
   * Where a column held at `point` would land (see `columnAtPoint`), from
   * the row and the columns as they are laid out now. The drag scrolls the
   * row, so the rects are read afresh each time.
   */
  function columnDropAt(point: PointerDragPoint): ColumnDrop | null {
    if (!columnsEl) return null;
    const columns = [...columnsEl.querySelectorAll<HTMLElement>('[data-board-column]')].map(
      (el) => ({ key: el.dataset.boardColumn ?? '', rect: el.getBoundingClientRect() })
    );
    return columnAtPoint(point, columnsEl.getBoundingClientRect(), columns, columnDrop);
  }

  function setColumnDrop(drop: ColumnDrop | null) {
    if (columnDrop?.columnKey === drop?.columnKey && columnDrop?.place === drop?.place) return;
    columnDrop = drop;
  }

  function endColumnDrag() {
    draggingColumnKey = null;
    columnDrop = null;
  }

  // A column's drag, as the pointerDrag action on its header or strip
  // reports it. The action follows every `start` with a `drop` or a
  // `cancel`, so no column drag is ever left over.
  const columnDragHandlers: BoardColumnDragHandlers = {
    get enabled() {
      return dragEnabled;
    },
    get draggingKey() {
      return draggingColumnKey;
    },
    placeFor(columnKey) {
      if (!columnDropWouldMove || columnDrop?.columnKey !== columnKey) return null;
      return columnDrop.place;
    },
    scroll: { container: () => columnsEl, axis: 'x' },
    start(columnKey) {
      // One drag at a time: a card still in hand is put back.
      cardDrag.cancel();
      draggingColumnKey = columnKey;
      columnDrop = null;
    },
    /**
     * The cursor says a drop is refused wherever it would leave the columns
     * in their order: level with the column itself, with the half where it
     * already is, in a gap with no line, and clearly away from the row.
     */
    move(point) {
      setColumnDrop(columnDropAt(point));
      return columnDropWouldMove;
    },
    /**
     * Drop the dragged column where the pointer was released, which is
     * resolved as any other point. The pointer put the column there, so
     * focus stays where it is.
     */
    drop(point) {
      const key = draggingColumnKey;
      const drop = columnDropAt(point);
      endColumnDrag();
      const move = key !== null && drop !== null ? columnMove(key, drop) : null;
      if (!move) return;
      void pendingCatalogMoves.moveTag({
        tagId: move.tagId,
        groupId: move.groupId,
        index: move.index,
      });
    },
    cancel: endColumnDrag,
  };

  function setCollapsed(key: string, collapsed: boolean) {
    if (!group) return;
    setBoardColumnFlag(group.id, key, 'collapsed', collapsed, tagsStore.catalog);
  }

  function hideColumn(key: string) {
    if (!group) return;
    setBoardColumnFlag(group.id, key, 'hidden', true, tagsStore.catalog);
  }

  /**
   * The tag move that drops column `key` on `drop`, read against the order
   * the board shows, or null when the columns would stay as they are.
   */
  function columnMove(key: string, drop: ColumnDrop) {
    if (!group || !columnPrefs) return null;
    const catalog = pendingCatalogMoves.catalog;
    const target = columnDropTarget(catalog, group.id, columnPrefs.hidden, key, drop);
    return target && resolveTagDrop(catalog, key, target);
  }

  /** Where a step left (-1) or right (1) would put a column, or null at that end. */
  function columnStep(key: string, step: -1 | 1) {
    if (!group || !columnPrefs) return null;
    return columnStepTarget(pendingCatalogMoves.catalog, group.id, columnPrefs.hidden, key, step);
  }

  /** Move a tag column past the nearest showing one on that side. */
  function moveColumn(key: string, step: -1 | 1) {
    const target = columnStep(key, step);
    const move = target && resolveTagDrop(pendingCatalogMoves.catalog, key, target);
    if (!move) return;
    void focusColumnMenu(key);
    void pendingCatalogMoves.moveTag({
      tagId: move.tagId,
      groupId: move.groupId,
      index: move.index,
    });
  }

  function columnElement(key: string): HTMLElement | null {
    for (const el of columnsEl?.querySelectorAll<HTMLElement>('[data-board-column]') ?? []) {
      if (el.dataset.boardColumn === key) return el;
    }
    return null;
  }

  function columnMenuTrigger(key: string): HTMLElement | null {
    for (const el of columnsEl?.querySelectorAll<HTMLElement>('[data-board-column-menu]') ?? []) {
      if (el.dataset.boardColumnMenu === key) return el;
    }
    return null;
  }

  /**
   * Put focus back on the ⋯ menu of a column that just moved, so the move
   * can be repeated from the keyboard, and keep the column on screen. The
   * closing menu hands focus to its trigger, but the keyed list then moves
   * the column and WebKit drops focus from a node that moves. The focus
   * waits for a render, by which time the move is showing, and is checked
   * once more on the next frame in case the menu closed after it.
   */
  async function focusColumnMenu(key: string) {
    await tick();
    columnMenuTrigger(key)?.focus({ preventScroll: true });
    revealColumn(key);
    requestAnimationFrame(() => focusIfDropped(() => columnMenuTrigger(key)));
  }

  /**
   * Scroll the columns sideways, and only them, until a column is clear of
   * the edge fades. On the phone pager that is the column's own page: the
   * move changed which column sits at the scroll position.
   */
  function revealColumn(key: string) {
    const column = columnElement(key);
    if (!columnsEl || !column) return;
    const box = columnsEl.getBoundingClientRect();
    const fade = paged ? 0 : SCROLL_FADE_WIDTH;
    const view = { left: box.left + fade, right: box.right - fade };
    const offset = revealOffset(view, column.getBoundingClientRect());
    if (offset !== 0) columnsEl.scrollLeft += offset;
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
    columnElement(key)?.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'start' });
  }

  /** The card element for a project, for the home page's scroll restore. */
  export function cardFor(projectId: string): HTMLElement | null {
    return cardElement(projectId);
  }
</script>

<div
  class="projects-board"
  class:paged
  class:dragging={draggingId !== null}
  class:drag-refused={refused}
  bind:this={boardEl}
>
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
        {#each layout.columns as column, i (column.key)}
          <BoardColumn
            {column}
            {group}
            {handlers}
            columnDrag={columnDragHandlers}
            {isVisible}
            {paged}
            first={i === 0}
            dropTarget={dropKey === column.key}
            canMoveLeft={columnStep(column.key, -1) !== null}
            canMoveRight={columnStep(column.key, 1) !== null}
            onCollapse={(collapsed) => setCollapsed(column.key, collapsed)}
            onMove={(step) => moveColumn(column.key, step)}
            onHide={() => hideColumn(column.key)}
          />
        {/each}
        <BoardAddColumn {group} {paged} refusing={refused && drag?.target?.kind === 'add-column'} />
      </div>
      <BoardScrollEdges scroller={columnsEl} enabled={!paged} />
    </div>
  {/if}
</div>

{#if drag}
  <BoardDragGhost
    source={drag.card}
    width={drag.width}
    x={drag.point.x - drag.grab.x}
    y={drag.point.y - drag.grab.y}
    {refused}
  />
{/if}

<style>
  /* Takes what is left of the page under it (the page's content is a
     column of at least the window's height), and never less than its
     columns need. */
  .projects-board {
    flex: 1 0 auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }

  /* A drag owns the cursor over everything on the board, the cards and
     buttons with one of their own included. Nothing on the board answers the
     pointer meanwhile, so a card carried over it sets off no hover styles
     and no tooltips: only the columns' scroller is left to be under the
     pointer, and the drop target is read off where the columns lie in it
     (dropTargetAt). */
  .projects-board.dragging :global(*) {
    cursor: grabbing !important;
    pointer-events: none !important;
  }

  .projects-board.dragging .board-columns {
    pointer-events: auto !important;
  }

  .projects-board.drag-refused :global(*) {
    cursor: not-allowed !important;
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

  /* Positions BoardScrollEdges' fades over the scroller's sides, and hands
     the board's height on to the scroller. */
  .board-columns-frame {
    position: relative;
    flex: 1 0 auto;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  /* Every column is as tall as the scroller, which is as tall as the board:
     a column's box is where a dragged card can be dropped on it, and what
     lights up when one is over it. The scroller scrolls sideways only, so
     it never shrinks under its columns' height to scroll them down: the
     page does that. */
  .board-columns {
    flex: 1 0 auto;
    display: flex;
    align-items: stretch;
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
