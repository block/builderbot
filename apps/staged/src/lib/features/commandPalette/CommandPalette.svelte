<!--
  CommandPalette.svelte — the ⌘K project palette: a Spotlight-style dialog
  that finds any project and opens its detail view.

  Mounted once in App.svelte beside the other shared dialogs and driven by
  commandPalette.open (the shortcut itself is registered with the rest of
  the app's shortcuts in App.svelte).

  What it searches is every project the app knows about —
  projectsDataStore.projects, the unfiltered list — and never the window's
  filtered views: the chip filters and the sidebar's search narrow the grid
  and the sidebar, not this. A project those hide is listed with a "Filtered
  out" badge, and opening it goes through navigation's selectProject, which
  shows the detail view whatever the filters say and leaves them untouched
  (the sidebar keeps the selected project's row visible on its own).

  Matching and ranking live in paletteSearch.ts. The dialog is the design
  system's (focus trap, Escape, backdrop dismiss, focus restore, the
  open/close animation); inside it the input is a combobox that keeps focus
  and points at the highlighted option with aria-activedescendant.
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import Search from '@lucide/svelte/icons/search';
  import * as Dialog from '$lib/components/ui/dialog';
  import { isImeKeydown } from '../../shared/imeKeydown';
  import { activeDescendantId, listboxOptionId } from '../../shared/listboxIds';
  import { revealWithin } from '../../shared/revealWithin';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { navigation, selectProject } from '../layout/navigation.svelte';
  import { activeLayerHost } from '../layout/workspaceLayer';
  import { projectFiltersStore } from '../projects/projectFilters.svelte';
  import { searchProjects } from '../projects/projectSearch';
  import { projectSearchSnapshots } from '../projects/projectSearchContext';
  import { projectSearchIndex } from '../projects/projectSearchIndex.svelte';
  import { projectsListViewState } from '../projects/projectsListViewState.svelte';
  import CommandPaletteRow from './CommandPaletteRow.svelte';
  import { closeCommandPalette, commandPalette } from './paletteState.svelte';
  import { moveSelection, paletteResults, type PaletteResults } from './paletteSearch';

  const uid = $props.id();
  const listboxId = `command-palette-${uid}-results`;

  const NO_RESULTS: PaletteResults = { rows: [], total: 0, terms: [] };
  const NO_IDS: ReadonlySet<string> = new Set();

  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);
  let selectedIndex = $state(0);
  // Set when a result was opened, so closing sends focus to the view that
  // came up rather than back to a control in the one it replaced.
  let navigated = false;

  let query = $derived(commandPalette.query);
  let searching = $derived(query.trim().length > 0);
  let sidebarQuery = $derived(projectsListViewState.searchQuery);

  // Prepared search data for every project (branches, PRs, repo labels,
  // note and review titles). Built only while something reads it: the
  // palette is open and a query — its own, or the sidebar's for the
  // "Filtered out" badge — is active.
  const searchSnapshot = projectSearchSnapshots();
  let snapshot = $derived(
    commandPalette.open && (searching || sidebarQuery.trim().length > 0) ? searchSnapshot() : null
  );

  let results = $derived(
    commandPalette.open
      ? paletteResults({
          projects: projectsDataStore.projects,
          query,
          snapshot,
          reposByProject: projectsDataStore.reposByProject,
          branchesByProject: projectsDataStore.branchesByProject,
          currentProjectId: navigation.selectedProjectId,
        })
      : NO_RESULTS
  );
  let rows = $derived(results.rows);

  // The projects this window's chip filters or sidebar search hide. Only
  // for the badge: the palette lists and opens them all the same.
  let filteredOutIds = $derived.by(() => {
    if (!commandPalette.open) return NO_IDS;
    if (!projectFiltersStore.hasActiveFilters && !sidebarQuery.trim()) return NO_IDS;
    const shown = new Set(
      searchProjects(projectFiltersStore.filteredProjects, sidebarQuery, snapshot).projects.map(
        (project) => project.id
      )
    );
    return new Set(
      projectsDataStore.projects.filter((p) => !shown.has(p.id)).map((project) => project.id)
    );
  });

  // The highlighted row. A list that shrank under it (data changing while
  // open) clamps rather than pointing past the end.
  let activeIndex = $derived(rows.length === 0 ? -1 : Math.min(selectedIndex, rows.length - 1));
  let activeDescendant = $derived(
    activeDescendantId(listboxId, {
      open: rows.length > 0,
      count: rows.length,
      selectedIndex: activeIndex,
    })
  );

  // Each open starts from the top, with what a query will search on its way:
  // every project's branches and repos (the idle drip may not have reached
  // them all) and the note/review title index.
  $effect.pre(() => {
    if (!commandPalette.open) return;
    untrack(() => {
      selectedIndex = 0;
      navigated = false;
      projectSearchIndex.ensureLoaded();
      void projectsDataStore.ensureProjectsHydrated();
    });
  });

  function handleInput() {
    selectedIndex = 0;
    if (listEl) listEl.scrollTop = 0;
    // A stale title index reloads on the keystroke, as in the search fields.
    projectSearchIndex.ensureLoaded();
  }

  /** Highlight a row; from the keyboard, also scroll it into view. */
  function highlight(index: number, reveal: boolean) {
    if (index < 0 || index === activeIndex) return;
    selectedIndex = index;
    if (!reveal) return;
    void tick().then(() => {
      const row = document.getElementById(listboxOptionId(listboxId, index));
      if (row && listEl) revealWithin(row, listEl, { block: 'nearest' });
    });
  }

  function openRow(index: number) {
    const project = rows[index]?.project;
    // A project being removed is listed, but there is nothing left to open.
    if (!project || projectsDataStore.isProjectDeleting(project.id)) return;
    // Already the view on screen: closing is the whole of it, and focus goes
    // back where it was.
    const onScreen =
      navigation.currentRoute.kind === 'project' &&
      navigation.currentRoute.projectId === project.id;
    navigated = !onScreen;
    closeCommandPalette();
    if (!onScreen) selectProject(project.id);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (isImeKeydown(e) || e.metaKey || e.ctrlKey || e.altKey) return;
    const delta = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
    if (delta === 0 && e.key !== 'Enter') return;
    // These keys are the palette's. Stopped as well as prevented: views
    // under the dialog listen on the window (the splash form submits on
    // Enter, the diff view steps through hunks on the arrows).
    e.preventDefault();
    e.stopPropagation();
    if (delta !== 0) highlight(moveSelection(activeIndex, delta, rows.length), true);
    else openRow(activeIndex);
  }

  function handleOpenAutoFocus(e: Event) {
    e.preventDefault();
    inputEl?.focus();
  }

  // Closed without opening anything (Escape, the backdrop, ⌘K again): the
  // dialog returns focus to what had it. After opening a project that
  // control belongs to a view that is now covered or gone, so focus goes to
  // the layer that came up instead.
  function handleCloseAutoFocus(e: Event) {
    if (!navigated) return;
    navigated = false;
    e.preventDefault();
    activeLayerHost()?.focus({ preventScroll: true });
  }
</script>

<Dialog.Root open={commandPalette.open} onOpenChange={(open) => !open && closeCommandPalette()}>
  <Dialog.Content
    data-command-palette=""
    class="flex flex-col gap-0 overflow-hidden p-0 sm:max-w-[620px]"
    showCloseButton={false}
    fullScreenOnMobile={false}
    onOpenAutoFocus={handleOpenAutoFocus}
    onCloseAutoFocus={handleCloseAutoFocus}
  >
    <Dialog.Title class="sr-only">Search projects</Dialog.Title>
    <Dialog.Description class="sr-only">
      Type to find a project by name, repo, branch or pull request. Use the arrow keys to choose a
      result and Enter to open it.
    </Dialog.Description>

    <div class="palette-search">
      <Search size={16} />
      <input
        bind:this={inputEl}
        bind:value={commandPalette.query}
        type="text"
        role="combobox"
        placeholder="Search projects…"
        aria-label="Search projects"
        aria-autocomplete="list"
        aria-expanded={rows.length > 0}
        aria-controls={rows.length > 0 ? listboxId : undefined}
        aria-activedescendant={activeDescendant}
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck="false"
        oninput={handleInput}
        onkeydown={handleKeydown}
      />
    </div>

    {#if rows.length > 0}
      <!-- A press on a row must not take focus from the input: the click
           still opens the row, and typing carries on if it doesn't. -->
      <div
        class="palette-list"
        role="listbox"
        id={listboxId}
        aria-label="Projects"
        tabindex="-1"
        bind:this={listEl}
        onmousedown={(e) => e.preventDefault()}
      >
        {#each rows as row, i (row.project.id)}
          <CommandPaletteRow
            id={listboxOptionId(listboxId, i)}
            {row}
            terms={results.terms}
            selected={i === activeIndex}
            current={row.project.id === navigation.selectedProjectId}
            filteredOut={filteredOutIds.has(row.project.id)}
            removing={projectsDataStore.isProjectDeleting(row.project.id)}
            onhighlight={() => highlight(i, false)}
            onopen={() => openRow(i)}
          />
        {/each}
      </div>
    {:else}
      <div class="palette-empty" role="status">
        {#if searching}
          <p class="empty-title">No projects match “{query.trim()}”</p>
          <p class="empty-hint">
            Search covers project names, repos, branches, pull requests and note titles.
          </p>
        {:else if projectsDataStore.loaded}
          <p class="empty-title">No projects yet</p>
        {:else}
          <p class="empty-title">Loading projects…</p>
        {/if}
      </div>
    {/if}

    <div class="palette-footer">
      <span class="hints">
        <span class="hint"><kbd>↑</kbd><kbd>↓</kbd> navigate</span>
        <span class="hint"><kbd>↵</kbd> open</span>
        <span class="hint"><kbd>esc</kbd> close</span>
      </span>
      {#if rows.length > 0}
        <span class="count" aria-live="polite">
          {#if results.total > rows.length}
            Top {rows.length} of {results.total}
          {:else}
            {results.total} {results.total === 1 ? 'project' : 'projects'}
          {/if}
        </span>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
  /* Near the top like Spotlight, not centred: the list grows and shrinks as
     the query narrows, and a centred panel would move its input with it.
     The dialog primitive centres with `inset: 0; margin: auto`; anchoring
     the top and dropping the block margins keeps its horizontal centring. */
  :global([data-command-palette]) {
    top: 12vh;
    bottom: auto;
    margin-block: 0;
    box-shadow: var(--shadow-elevated);
  }

  /* The primitive's fade and zoom, on the panel and on its backdrop, are the
     open/close animation; without motion both simply appear. */
  @media (prefers-reduced-motion: reduce) {
    :global([data-command-palette]),
    :global([data-slot='dialog-overlay']:has(+ [data-command-palette])) {
      animation: none;
    }
  }

  .palette-search {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 16px;
    height: 48px;
    flex-shrink: 0;
    border-bottom: 1px solid var(--border-muted);
    color: var(--text-muted);
  }

  .palette-search :global(svg) {
    flex-shrink: 0;
  }

  input {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text-primary);
    font: inherit;
    font-size: var(--size-base);
  }

  input::placeholder {
    color: var(--text-muted);
  }

  .palette-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px;
    /* Room for about seven rows; less in a short window, where the panel
       must still fit under its 12vh offset with the input and footer. */
    max-height: min(396px, calc(100dvh - 12vh - 140px));
    overflow-y: auto;
    overscroll-behavior: contain;
    outline: none;
  }

  .palette-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 32px 24px;
    text-align: center;
  }

  .palette-empty p {
    margin: 0;
  }

  .empty-title {
    font-size: var(--size-sm);
    color: var(--text-secondary);
    overflow-wrap: anywhere;
  }

  .empty-hint {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .palette-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-shrink: 0;
    padding: 8px 14px;
    border-top: 1px solid var(--border-muted);
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .hints {
    display: flex;
    align-items: center;
    gap: 14px;
    min-width: 0;
  }

  .hint {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    white-space: nowrap;
  }

  .count {
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }

  kbd {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1.8em;
    padding: 0.15em 0.45em;
    border: 1px solid var(--border-muted);
    border-radius: 5px;
    background: var(--bg-primary);
    color: var(--text-muted);
    font-family: inherit;
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
    line-height: 1.2;
  }
</style>
