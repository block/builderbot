<!--
  CommandPalette.svelte — the ⌘K project palette: a Spotlight-style dialog
  that finds any project and opens its detail view.

  Mounted once in App.svelte beside the other shared dialogs and driven by
  commandPalette.open (the shortcut is defined in paletteState.svelte.ts and
  registered with the rest of the app's shortcuts in App.svelte).

  What it searches is every project the app knows about —
  projectsDataStore.projects, the unfiltered list — and never the window's
  filtered views: the chip filters and the search field narrow the grid and
  the sidebar, not this. A project those hide is listed with a "Filtered
  out" badge, and opening it goes through navigation's selectProject, which
  shows the detail view whatever the filters say and leaves them untouched
  (the sidebar keeps the selected project's row visible on its own).

  It opens over Settings and the diff view too. A project opened from there
  replaces the workspace route underneath (selectProject), and the diff view
  first saves what was typed into it (diffLeaveCheck) while the row being
  opened shows the wait and the palette takes nothing but a dismissal.

  What the panel shows is one value, `view`, which stops changing when the
  palette closes: the dialog stays mounted for its exit animation, and it
  fades out showing what it last showed.

  Matching and ranking live in paletteSearch.ts. The dialog is the design
  system's (focus trap, Escape, backdrop dismiss, focus restore, the
  open/close animation); inside it the input is a combobox that keeps focus
  and points at the highlighted option with aria-activedescendant.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { toast } from 'svelte-sonner';
  import Search from '@lucide/svelte/icons/search';
  import * as Dialog from '$lib/components/ui/dialog';
  import { isImeKeydown } from '../../shared/imeKeydown';
  import { activeDescendantId, listboxOptionId } from '../../shared/listboxIds';
  import { revealWithin } from '../../shared/revealWithin';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { diffLeaveCheck, navigation, selectProject } from '../layout/navigation.svelte';
  import { activeLayerHost } from '../layout/workspaceLayer';
  import { projectFiltersStore } from '../projects/projectFilters.svelte';
  import { projectSearchSnapshots, repoBadgeShortName } from '../projects/projectSearchContext';
  import { projectSearchIndex } from '../projects/projectSearchIndex.svelte';
  import {
    gridStickyProjectId,
    projectsListViewState,
  } from '../projects/projectsListViewState.svelte';
  import CommandPaletteRow from './CommandPaletteRow.svelte';
  import { closeCommandPalette, commandPalette } from './paletteState.svelte';
  import {
    hiddenProjectIds,
    moveSelection,
    paletteResults,
    type PaletteResults,
  } from './paletteSearch';

  const uid = $props.id();
  const listboxId = `command-palette-${uid}-results`;

  /** What the panel shows of the stores: the results, and what the rows'
   *  badges and the wait come from. */
  interface PaletteView {
    results: PaletteResults;
    filteredOutIds: ReadonlySet<string>;
    /** The project the window is showing. */
    currentId: string | null;
    /** The project the palette is waiting to open. */
    busyId: string | null;
  }

  const NO_IDS: ReadonlySet<string> = new Set();
  const NO_VIEW: PaletteView = {
    results: { rows: [], total: 0, terms: [] },
    filteredOutIds: NO_IDS,
    currentId: null,
    busyId: null,
  };

  let inputEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);
  // The highlight starts on the top row and stays there, whichever project
  // that becomes, until the arrow keys or the pointer move it. From then on
  // it is a project, not a position: `selectedId` names it, and
  // `selectedIndex` is the row it was last seen at, used only once that
  // project has left the list. Null means not moved yet (a fresh open, an
  // edited query).
  let selectedId = $state<string | null>(null);
  let selectedIndex = $state(0);
  // Set when a result was opened, so closing sends focus to the view that
  // came up rather than back to a control in the one it replaced.
  let navigated = false;
  // Which open this is, and the project it is waiting on the diff view to
  // save before opening (its row shows the wait).
  let openSession = 0;
  let openingId = $state<string | null>(null);
  // The highlight was last moved by the pointer (see the reveal effect).
  let movedByPointer = false;
  // Where the pointer was last seen over the list (see handleListPointerMove).
  let pointerAt: { x: number; y: number } | null = null;

  let query = $derived(commandPalette.query);
  let searching = $derived(query.trim().length > 0);
  let listQuery = $derived(projectsListViewState.searchQuery);

  // Prepared search data for every project (branches, PRs, repo labels,
  // note and review titles). Built only while something reads it: the
  // palette is open (`view`, below, is the one reader of these three) and a
  // query — its own, or the window's search field's for the "Filtered out"
  // badge — is active.
  const searchSnapshot = projectSearchSnapshots();
  let snapshot = $derived(searching || listQuery.trim().length > 0 ? searchSnapshot() : null);

  let liveResults = $derived(
    paletteResults({
      projects: projectsDataStore.projects,
      query,
      snapshot,
      reposByProject: projectsDataStore.reposByProject,
      branchesByProject: projectsDataStore.branchesByProject,
      repoShortName: repoBadgeShortName,
      currentProjectId: navigation.selectedProjectId,
    })
  );

  // The projects this window's own list is hiding, for the badge only: the
  // palette lists and opens them all the same. "Own list" is the surface the
  // workspace shows (under Settings or the diff view too): the grid on home,
  // the sidebar beside a project or the repos list. Each keeps one project
  // its chips exclude, and the badge goes by the same one: the grid's sticky
  // return card, the sidebar's selected project. A project being removed is
  // off both lists, and its row says "Removing…" instead.
  let listStickyId = $derived(
    navigation.homeActive
      ? gridStickyProjectId(projectFiltersStore.activeFilters)
      : navigation.projectLayer?.kind === 'project'
        ? navigation.projectLayer.projectId
        : null
  );
  let liveFilteredOutIds = $derived.by(() => {
    if (!projectFiltersStore.hasActiveFilters && !listQuery.trim()) return NO_IDS;
    return hiddenProjectIds({
      projects: projectsDataStore.projects,
      chipFiltered: projectFiltersStore.filteredProjects,
      stickyId: listStickyId,
      query: listQuery,
      snapshot,
    });
  });

  // What the panel shows. It follows the stores while the palette is open
  // and holds what it last showed once it is closed: the dialog stays
  // mounted while its exit animation plays, and closing changes what the
  // stores say (opening a project makes it the current one, which reorders
  // the recents and moves the badges). A panel that kept following would
  // fade out showing something else, and one that emptied would show "No
  // projects" and announce it. The next open computes it afresh.
  let heldView = NO_VIEW;
  let view = $derived.by(() => {
    if (!commandPalette.open) return heldView;
    heldView = {
      results: liveResults,
      filteredOutIds: liveFilteredOutIds,
      currentId: navigation.selectedProjectId,
      busyId: openingId,
    };
    return heldView;
  });
  let results = $derived(view.results);
  let rows = $derived(results.rows);

  // Each open starts from the top, with what a query will search on its way:
  // every project's branches and repos (the idle drip may not have reached
  // them all) and the note/review title index.
  //
  // The top of the list too. An open usually mounts a new list, but one that
  // comes while the last close is still fading out (⌘K straight after
  // Escape) takes over the dialog as it stands, with the list scrolled to
  // wherever the arrow keys or the wheel left it. The highlight is back on
  // the top row, and nothing else would bring that row into view: an
  // unmoved highlight is never revealed (its index hasn't changed), and a
  // reveal measures a panel the animation is still scaling, so it stops
  // short.
  $effect.pre(() => {
    if (!commandPalette.open) return;
    untrack(() => {
      openSession += 1;
      selectedId = null;
      selectedIndex = 0;
      navigated = false;
      openingId = null;
      movedByPointer = false;
      pointerAt = null;
      if (listEl) listEl.scrollTop = 0;
      projectSearchIndex.ensureLoaded();
      void projectsDataStore.ensureProjectsHydrated();
    });
  });

  // The highlighted row: the top one until the highlight is moved, then
  // wherever the highlighted project is now. The list changes while the
  // palette is open (opening it fetches every project's branches, and each
  // one landing can reorder the recents; projects are created, renamed and
  // removed), and Enter must open the project that was highlighted, not
  // whichever one has taken its row. Only when that project is no longer
  // listed does the highlight fall back to its old position, clamped to the
  // list.
  let selectedAt = $derived(
    selectedId === null ? -1 : rows.findIndex((row) => row.project.id === selectedId)
  );
  let activeIndex = $derived(
    rows.length === 0 ? -1 : selectedAt >= 0 ? selectedAt : Math.min(selectedIndex, rows.length - 1)
  );
  let activeId = $derived(activeIndex < 0 ? null : rows[activeIndex].project.id);
  let activeDescendant = $derived(
    activeDescendantId(listboxId, {
      open: rows.length > 0,
      count: rows.length,
      selectedIndex: activeIndex,
    })
  );

  // Follow a highlight the user has moved: remember the row its project sits
  // at, and once it has fallen to a position (a clamp, after that project
  // left the list), pin it to the project found there, so the next change to
  // the list doesn't move it again. One that was never moved is not pinned:
  // results reorder as they load (branches land, the title index brings a
  // better match), and a pinned default would follow the project that
  // happened to be on top first, taking the list's scroll down with it.
  $effect.pre(() => {
    const id = activeId;
    const index = activeIndex;
    if (id === null) return;
    untrack(() => {
      if (selectedId === null) return;
      selectedId = id;
      selectedIndex = index;
    });
  });

  // Keep the highlighted row in view when it moves: the arrow keys moved the
  // highlight, or the list reordered and took the row with it. Not when the
  // pointer moved the highlight, which would scroll the list out from under
  // it.
  $effect(() => {
    const index = activeIndex;
    if (index < 0) return;
    if (movedByPointer) {
      movedByPointer = false;
      return;
    }
    untrack(() => {
      const row = document.getElementById(listboxOptionId(listboxId, index));
      if (row && listEl) revealWithin(row, listEl, { block: 'nearest' });
    });
  });

  // One line for screen readers, in a live region that stays mounted (below):
  // how many projects are listed, or why none are.
  let status = $derived.by(() => {
    if (view.busyId !== null) return 'Saving comment…';
    if (rows.length > 0) {
      if (results.total > rows.length) return `Top ${rows.length} of ${results.total} projects`;
      return `${results.total} ${results.total === 1 ? 'project' : 'projects'}`;
    }
    if (searching) return `No projects match “${query.trim()}”`;
    return projectsDataStore.loaded ? 'No projects yet' : 'Loading projects…';
  });

  function handleInput() {
    selectedId = null;
    selectedIndex = 0;
    if (listEl) listEl.scrollTop = 0;
    // A stale title index reloads on the keystroke, as in the search fields.
    projectSearchIndex.ensureLoaded();
  }

  /** Highlight the project at a row, from the keyboard or the pointer. Not
   *  while a project is waiting to open (see openRow), nor once closed. */
  function highlight(index: number, fromPointer: boolean) {
    if (!commandPalette.open || openingId !== null) return;
    const id = rows[index]?.project.id;
    if (!id || index === activeIndex) return;
    movedByPointer = fromPointer;
    selectedId = id;
    selectedIndex = index;
  }

  // The pointer highlights the row it moves over, and only when it moves.
  // WebKit sends a pointermove, at the pointer's unchanged position, to
  // whatever slides under a resting pointer when the list scrolls, which is
  // what the arrow keys do to keep the highlight in view: taken as a hover,
  // each one would snap the highlight back to the pointer's row. So an event
  // counts when its position differs from the last one seen over the list,
  // gaps and padding included (a row can slide under a pointer resting
  // there). The first one after an open only says where the pointer is: it
  // may be that same resting pointer, and a real movement sends another
  // straight after.
  function handleListPointerMove(e: PointerEvent) {
    const last = pointerAt;
    pointerAt = { x: e.clientX, y: e.clientY };
    if (last === null || (last.x === pointerAt.x && last.y === pointerAt.y)) return;
    const option = e.target instanceof Element ? e.target.closest('[role="option"]') : null;
    if (!(option instanceof HTMLElement)) return;
    const index = rows.findIndex((row) => row.project.id === option.dataset.projectId);
    if (index >= 0) highlight(index, true);
  }

  /** Whether the project can be opened: listed, and not being removed (such
   *  a project is listed, but there is nothing left to open). */
  function openable(projectId: string): boolean {
    return (
      projectsDataStore.projects.some((project) => project.id === projectId) &&
      !projectsDataStore.isProjectDeleting(projectId)
    );
  }

  async function openRow(index: number) {
    // Not from a panel that is only fading out: its rows are still there.
    if (!commandPalette.open) return;
    const project = rows[index]?.project;
    if (!project || openingId !== null || !openable(project.id)) return;

    // Over the diff view, opening a project leaves it. Comments typed there
    // save as they are written; like the diff view's own close, wait for the
    // last save and stay when it failed, so the text is still there to retry.
    // The row says it is waiting, so a slow save doesn't read as a palette
    // that ignored the key. Until the save lands the palette holds still:
    // the field is read-only (typing, pasting, an IME) and the keys and the
    // pointer leave the highlight where it is, as Enter and clicks already
    // did nothing. Otherwise a query typed meanwhile could filter the
    // waiting row out, or the arrows scroll it away, with the open it stands
    // for still on its way. Escape still closes, which calls the open off.
    const diffSaved = diffLeaveCheck();
    if (diffSaved) {
      const session = openSession;
      openingId = project.id;
      let saved = false;
      try {
        saved = await diffSaved();
      } catch (e) {
        console.error('[CommandPalette] Could not save the diff comment before leaving:', e);
      } finally {
        // Unless the palette was reopened meanwhile: `openingId` is that
        // open's now.
        if (session === openSession) openingId = null;
      }
      // Dismissed while the save ran: the user called it off.
      if (!commandPalette.open || session !== openSession) return;
      if (!saved) {
        // Closing returns focus to the comment that needs another try.
        closeCommandPalette();
        toast.error('Comment not saved', {
          description: 'Still on the diff, so nothing typed there is lost. Try again.',
        });
        return;
      }
      if (!openable(project.id)) return;
    }

    // Already the view on screen: closing is the whole of it, and focus goes
    // back where it was.
    const onScreen =
      navigation.currentRoute.kind === 'project' &&
      navigation.currentRoute.projectId === project.id;
    navigated = !onScreen;
    closeCommandPalette();
    if (!onScreen) selectProject(project.id);
  }

  // On the dialog, not the input: the keys are the palette's wherever focus
  // sits inside it.
  function handleKeydown(e: KeyboardEvent) {
    if (isImeKeydown(e) || e.metaKey || e.ctrlKey || e.altKey) return;
    // Focus that ended up on the dialog itself (it is focusable, for the
    // focus trap) goes back to the field, where this key's text belongs.
    if (document.activeElement !== inputEl && e.key !== 'Tab' && e.key !== 'Escape') {
      inputEl?.focus();
    }
    // Waiting to open a project (openRow): the palette holds still, and
    // only the keys that leave it get through. The field is read-only
    // meanwhile, and a key a read-only field has no use for falls to the
    // browser (WebKit takes Backspace there as "back" in the page history),
    // so the rest are taken here.
    if (openingId !== null) {
      if (e.key !== 'Escape' && e.key !== 'Tab') {
        e.preventDefault();
        e.stopPropagation();
      }
      return;
    }
    const delta = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
    if (delta === 0 && e.key !== 'Enter') return;
    // These keys are the palette's. Stopped as well as prevented: views
    // under the dialog listen on the window (the splash form submits on
    // Enter, the diff view steps through hunks on the arrows).
    e.preventDefault();
    e.stopPropagation();
    if (delta !== 0) highlight(moveSelection(activeIndex, delta, rows.length), false);
    else void openRow(activeIndex);
  }

  // A press anywhere in the panel but the field itself must not take focus
  // from it: on a row the click still opens it, and on the footer, the icon
  // or the empty state, typing and the arrow keys carry on. Without this the
  // press focuses the dialog and the field goes dead until clicked again.
  function handleMousedown(e: MouseEvent) {
    if (e.target === inputEl) return;
    e.preventDefault();
    if (document.activeElement !== inputEl) inputEl?.focus();
  }

  // The dialog closes on any Escape keydown. One that cancels an IME
  // composition is the IME's: closing would throw away the query with it.
  function handleEscapeKeydown(e: KeyboardEvent) {
    if (isImeKeydown(e)) e.preventDefault();
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
  <!-- The shadow is a utility, not a rule in the style block below: Tailwind
       draws the primitive's `ring-1` edge as a box-shadow layer and composes
       its shadow utilities with it, where a `box-shadow` declaration replaces
       the lot and the panel loses the 1px edge every other dialog has. -->
  <Dialog.Content
    data-command-palette=""
    class="flex flex-col gap-0 overflow-hidden p-0 shadow-(--shadow-elevated) sm:max-w-[620px]"
    showCloseButton={false}
    fullScreenOnMobile={false}
    onOpenAutoFocus={handleOpenAutoFocus}
    onCloseAutoFocus={handleCloseAutoFocus}
    onEscapeKeydown={handleEscapeKeydown}
    onkeydown={handleKeydown}
    onmousedown={handleMousedown}
  >
    <Dialog.Title class="sr-only">Search projects</Dialog.Title>
    <Dialog.Description class="sr-only">
      Type to find a project by name, repo, branch or pull request. Use the arrow keys to choose a
      result and Enter to open it.
    </Dialog.Description>
    <!-- The one live region, mounted for as long as the dialog is: only its
         text changes. A region inserted already holding its text (the count
         in the footer, the empty state) is often not announced at all. -->
    <div class="sr-only" role="status" aria-live="polite" aria-atomic="true">{status}</div>

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
        readonly={openingId !== null}
        oninput={handleInput}
      />
    </div>

    {#if rows.length > 0}
      <div
        class="palette-list"
        role="listbox"
        id={listboxId}
        aria-label="Projects"
        tabindex="-1"
        bind:this={listEl}
        onpointermove={handleListPointerMove}
      >
        {#each rows as row, i (row.project.id)}
          <CommandPaletteRow
            id={listboxOptionId(listboxId, i)}
            {row}
            terms={results.terms}
            selected={i === activeIndex}
            current={row.project.id === view.currentId}
            filteredOut={view.filteredOutIds.has(row.project.id)}
            removing={projectsDataStore.isProjectDeleting(row.project.id)}
            busy={row.project.id === view.busyId}
            onopen={() => void openRow(i)}
          />
        {/each}
      </div>
    {:else}
      <div class="palette-empty">
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
        <span class="count">
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
