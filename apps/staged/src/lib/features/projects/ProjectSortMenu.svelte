<!--
  ProjectSortMenu.svelte — the control for the window's project sort: the
  ordered keys the grid, the sidebar and every board are in (projectOrder).

  One menu in three places. In a search row (`variant="toolbar"`: the home
  grid and board, and a pinned board) the trigger reads "Sort" and the keys'
  names, or "Newest" with none; on a phone it is the icon alone. In the
  sidebar's projects header it is an icon. Wherever the name can't show, a
  dot says the order isn't the default.

  The menu lists the keys in order, numbered, the first the most significant.
  Each is a submenu: its direction, worded for its kind, then Move up, Move
  down and Remove. "Add sort key" offers created, last updated and each tag
  group not in use yet, and "Reset to default" shows while there is a key to
  drop. On a board, the key for the group its columns come from does nothing
  (every card in a column ties on it), so it reads "Used for columns here";
  it stays editable, since it still orders the grid and the sidebar.

  The menu stays open through an edit, so a sort is built in one visit. A
  direction change leaves everything where it is. An edit that changes the
  list (add, move, remove) closes the submenu it was picked from: the rows
  shift, and a submenu left open would slide a different item under the
  pointer, where a second click would undo a move or add a second key. Focus
  then goes to the row the edit was about, which also keeps the arrow keys
  and Escape working after the focused item's submenu is gone.

  Edits resolve against the catalog with its pending reorders, as the sorted
  list does, and prune keys for deleted groups on the way (projectSort). The
  trigger waits for a tag snapshot from the backend: before that no group is
  known, and an edit would drop every group key as deleted. A first read
  that fails settles the tags as loaded with the catalog still empty, so
  `tagsStore.loaded` is not the test; the trigger stays off until a read
  lands.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import ArrowDown from '@lucide/svelte/icons/arrow-down';
  import ArrowUp from '@lucide/svelte/icons/arrow-up';
  import ArrowUpDown from '@lucide/svelte/icons/arrow-up-down';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Plus from '@lucide/svelte/icons/plus';
  import X from '@lucide/svelte/icons/x';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { viewport } from '../../shared/viewport.svelte';
  import { tagsStore } from '../../stores/tags.svelte';
  import { onViewHidden } from '../layout/navigation.svelte';
  import { pendingCatalogMoves } from '../tags/pendingCatalogMoves.svelte';
  import {
    addableSortKeys,
    isInertOnBoard,
    resolveProjectSort,
    sortDirectionLabel,
    sortDirections,
    sortKeyId,
    sortKeyLabel,
    sortSummary,
    withSortKeyAdded,
    withSortKeyDirection,
    withSortKeyMoved,
    withoutSortKey,
    type ProjectSort,
    type ProjectSortKey,
  } from './projectSort';
  import { projectSortPrefs, setProjectSort } from './projectSortPrefs.svelte';

  interface Props {
    /** A search row's labelled button, or the sidebar header's icon. */
    variant?: 'toolbar' | 'sidebar';
    /** On a board: the group its columns come from, whose key does nothing there. */
    boardGroupId?: string | null;
  }

  let { variant = 'toolbar', boardGroupId = null }: Props = $props();

  /** The "Add sort key" submenu's name among the open submenus; no key id is this. */
  const ADD = 'add';

  let open = $state(false);
  /** The submenu that is open: a key's id, or ADD. */
  let openSub = $state<string | null>(null);
  let content = $state<HTMLElement | null>(null);

  // Off screen (home shown over the project layer, or parked under settings /
  // the diff view), the menu would stay open on top.
  onViewHidden(() => {
    open = false;
  });

  $effect(() => {
    if (!open) openSub = null;
  });

  let catalog = $derived(pendingCatalogMoves.catalog);
  let sort = $derived(resolveProjectSort(projectSortPrefs.sort, catalog));
  let addable = $derived(addableSortKeys(sort, catalog));
  let summary = $derived(sortSummary(sort, catalog));
  let sorting = $derived(sort.length > 0);

  function setSubOpen(id: string, next: boolean) {
    if (next) openSub = id;
    else if (openSub === id) openSub = null;
  }

  function setDirection(id: string, value: string) {
    if (value !== 'asc' && value !== 'desc') return;
    setProjectSort(withSortKeyDirection(projectSortPrefs.sort, id, value, catalog));
  }

  function addKey(key: ProjectSortKey) {
    editList(withSortKeyAdded(projectSortPrefs.sort, key, catalog), sortKeyId(key));
  }

  function moveKey(id: string, step: -1 | 1) {
    editList(withSortKeyMoved(projectSortPrefs.sort, id, step, catalog), id);
  }

  function removeKey(id: string) {
    const index = sort.findIndex((key) => sortKeyId(key) === id);
    const next = withoutSortKey(projectSortPrefs.sort, id, catalog);
    // The key that takes the removed one's place, else the one before it.
    const neighbour = next[index] ?? next[index - 1];
    editList(next, neighbour ? sortKeyId(neighbour) : null);
  }

  function rowFor(id: string | null): HTMLElement | null {
    if (id === null) return null;
    for (const row of content?.querySelectorAll<HTMLElement>('[data-sort-key]') ?? []) {
      if (row.dataset.sortKey === id) return row;
    }
    return null;
  }

  /**
   * Apply an edit that changes the list of keys, then close the submenu it
   * was picked from and put focus on the row `rowId` names, or on "Add sort
   * key" when no such row is left.
   *
   * A tap in a phone's sheet focuses the menu itself instead: a focused row
   * reads as selected there, with no hover to move it on, and from the menu
   * the arrow keys still reach the first item (see returnFocusFromMenuSub).
   */
  function editList(next: ProjectSort, rowId: string | null) {
    const keyboard = content?.ownerDocument.activeElement?.matches(':focus-visible') ?? false;
    setProjectSort(next);
    openSub = null;
    void tick().then(() => {
      const menu = content;
      if (!open || !menu) return;
      const row =
        viewport.sheetMenus && !keyboard
          ? null
          : (rowFor(rowId) ?? menu.querySelector<HTMLElement>('[data-sort-add]'));
      (row ?? menu).focus({ preventScroll: true });
    });
  }
</script>

<DropdownMenu.Root bind:open>
  {#if variant === 'sidebar'}
    <DropdownMenu.Trigger
      class="sidebar-sort-trigger {sorting ? 'sorting' : ''}"
      title={sorting ? `Sort: ${summary}` : 'Sort projects'}
      aria-label={`Sort projects: ${summary}`}
      disabled={!tagsStore.hasSnapshot}
    >
      <ArrowUpDown size={12} />
      {#if sorting}<span class="sort-dot"></span>{/if}
    </DropdownMenu.Trigger>
  {:else}
    <DropdownMenu.Trigger
      class="board-toolbar-trigger project-sort-trigger touch-hit"
      title="Sort projects"
      aria-label={`Sort projects: ${summary}`}
      disabled={!tagsStore.hasSnapshot}
    >
      <ArrowUpDown size={13} class="board-toolbar-chevron" />
      <span class="trigger-label">Sort</span>
      <span class="trigger-value">{summary}</span>
      <ChevronDown size={13} class="board-toolbar-chevron sort-trigger-chevron" />
      {#if sorting}<span class="sort-dot"></span>{/if}
    </DropdownMenu.Trigger>
  {/if}
  <DropdownMenu.Content bind:ref={content} align="end" sideOffset={4} class="min-w-[240px]">
    <DropdownMenu.Label>Sort by</DropdownMenu.Label>
    {#if !sorting}
      <div class="px-2 pb-1.5 text-sm text-muted-foreground" data-sort-default>
        Newest first (default)
      </div>
    {/if}
    {#each sort as key, index (sortKeyId(key))}
      {@const id = sortKeyId(key)}
      {@const inert = isInertOnBoard(key, boardGroupId)}
      <DropdownMenu.Sub bind:open={() => openSub === id, (next) => setSubOpen(id, next)}>
        <DropdownMenu.SubTrigger data-sort-key={id} data-sort-inert={inert ? '' : undefined}>
          <span class="w-3.5 shrink-0 text-center text-xs text-muted-foreground tabular-nums">
            {index + 1}
          </span>
          <span class="min-w-0 flex-1 truncate {inert ? 'text-muted-foreground' : ''}">
            {sortKeyLabel(key, catalog)}
          </span>
          <span class="max-w-[160px] truncate pl-3 text-xs text-muted-foreground">
            {inert ? 'Used for columns here' : sortDirectionLabel(key.kind, key.direction)}
          </span>
        </DropdownMenu.SubTrigger>
        <DropdownMenu.SubContent class="min-w-[190px]">
          <DropdownMenu.RadioGroup
            value={key.direction}
            onValueChange={(value) => setDirection(id, value)}
          >
            {#each sortDirections(key.kind) as direction (direction)}
              <DropdownMenu.RadioItem value={direction} closeOnSelect={false}>
                {sortDirectionLabel(key.kind, direction)}
              </DropdownMenu.RadioItem>
            {/each}
          </DropdownMenu.RadioGroup>
          <DropdownMenu.Separator />
          <DropdownMenu.Item
            closeOnSelect={false}
            disabled={index === 0}
            onSelect={() => moveKey(id, -1)}
          >
            <ArrowUp size={14} />
            Move up
          </DropdownMenu.Item>
          <DropdownMenu.Item
            closeOnSelect={false}
            disabled={index === sort.length - 1}
            onSelect={() => moveKey(id, 1)}
          >
            <ArrowDown size={14} />
            Move down
          </DropdownMenu.Item>
          <DropdownMenu.Separator />
          <DropdownMenu.Item closeOnSelect={false} onSelect={() => removeKey(id)}>
            <X size={14} />
            Remove
          </DropdownMenu.Item>
        </DropdownMenu.SubContent>
      </DropdownMenu.Sub>
    {/each}
    {#if addable.length > 0}
      <DropdownMenu.Separator />
      <DropdownMenu.Sub bind:open={() => openSub === ADD, (next) => setSubOpen(ADD, next)}>
        <DropdownMenu.SubTrigger data-sort-add>
          <Plus size={14} />
          Add sort key
        </DropdownMenu.SubTrigger>
        <DropdownMenu.SubContent class="min-w-[170px]">
          {#each addable as key (sortKeyId(key))}
            <DropdownMenu.Item
              closeOnSelect={false}
              data-sort-add-key={sortKeyId(key)}
              onSelect={() => addKey(key)}
            >
              <span class="truncate">{sortKeyLabel(key, catalog)}</span>
            </DropdownMenu.Item>
          {/each}
        </DropdownMenu.SubContent>
      </DropdownMenu.Sub>
    {/if}
    {#if sorting}
      <DropdownMenu.Separator />
      <DropdownMenu.Item onSelect={() => setProjectSort([])}>Reset to default</DropdownMenu.Item>
    {/if}
  </DropdownMenu.Content>
</DropdownMenu.Root>

<style>
  /* DropdownMenu.Trigger renders in a child component, so its classes escape
     this component's scoping; the dot below is authored here and stays
     scoped. The toolbar trigger's look is the shared `board-toolbar-trigger`
     (app.css). */
  :global(.project-sort-trigger),
  :global(.sidebar-sort-trigger) {
    position: relative;
  }

  :global(.project-sort-trigger:disabled),
  :global(.sidebar-sort-trigger:disabled) {
    opacity: 0.6;
    cursor: default;
  }

  :global(.sidebar-sort-trigger) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  :global(.sidebar-sort-trigger[data-state='open']) {
    background: var(--projects-sidebar-hover-bg, var(--bg-hover));
    color: var(--text-primary);
  }

  @media (hover: hover) {
    :global(.sidebar-sort-trigger:not(:disabled):hover) {
      background: var(--projects-sidebar-hover-bg, var(--bg-hover));
      color: var(--text-primary);
    }
  }

  :global(.sidebar-sort-trigger.sorting) {
    color: var(--ui-accent);
  }

  /* The order isn't the default. In a search row the trigger names the sort,
     so the dot only shows once the phone layout drops the name. */
  .sort-dot {
    position: absolute;
    top: 2px;
    right: 2px;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--ui-accent);
    pointer-events: none;
  }

  :global(.project-sort-trigger) .sort-dot {
    display: none;
    top: 5px;
    right: 5px;
  }

  /* Icon-only on a phone, where the search field needs the room, as the view
     toggle is. */
  @media (max-width: 640px) {
    :global(.project-sort-trigger) {
      padding: 0 9px;
    }

    :global(.project-sort-trigger) .trigger-label,
    :global(.project-sort-trigger) .trigger-value,
    :global(.project-sort-trigger .sort-trigger-chevron) {
      display: none;
    }

    :global(.project-sort-trigger) .sort-dot {
      display: block;
    }
  }
</style>
