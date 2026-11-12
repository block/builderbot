// View-local but persistent: ProjectsList is created fresh on every return to
// the landing page, so anything that should survive opening a project and
// coming back lives here rather than in the component.
export const projectsListViewState = $state({
  scrollTop: 0,
  returnTargetProjectId: null as string | null,
  restorePending: false,
  // Shared with the sidebar's search field, like the chip filters: the grid
  // and the sidebar list narrow together. It also outlives the component:
  // narrowing to a few candidates and opening them one by one would otherwise
  // reset the query on every round trip.
  searchQuery: '',
  // The grid's sticky card (see captureGridStickyProject): the project a
  // visit came back from, and the chip filter Set it was captured under.
  // Here rather than in ProjectsList so the ⌘K palette's "Filtered out"
  // badge can go by the same card the grid is keeping.
  stickyProjectId: null as string | null,
  stickyFilters: null as ReadonlySet<string> | null,
});

export function setProjectsListScrollTop(scrollTop: number): void {
  if (!Number.isFinite(scrollTop)) return;
  projectsListViewState.scrollTop = Math.max(0, scrollTop);
}

export function requestProjectsListRestore(projectId: string | null): void {
  if (!projectId) return;
  projectsListViewState.returnTargetProjectId = projectId;
  projectsListViewState.restorePending = true;
}

export function finishProjectsListRestore(): void {
  projectsListViewState.restorePending = false;
}

/**
 * Capture the grid's sticky card for a visit: the card you just came back
 * from stays in the grid even when the active filters no longer match it.
 * Visiting a project marks it read, so with Unread active the card
 * returnTargetProjectId points at — the one the scroll restore is aiming
 * for — would otherwise be gone on arrival. Same exception the sidebar makes
 * for the selected project.
 *
 * ProjectsList calls this on each visit — at mount, then before each render
 * that brings its layer back — with the active chip filter Set
 * (projectFiltersStore.activeFilters). Only a pending restore pins a card:
 * returnTargetProjectId is never cleared, so an ordinary visit — landing →
 * settings → back, where requestProjectsListRestore(null) early-returns —
 * would otherwise pin a project from a previous visit. restorePending is
 * still true at that point; the effect that finishes the restore runs after.
 */
export function captureGridStickyProject(activeFilters: ReadonlySet<string>): void {
  projectsListViewState.stickyProjectId = projectsListViewState.restorePending
    ? projectsListViewState.returnTargetProjectId
    : null;
  projectsListViewState.stickyFilters = activeFilters;
}

/**
 * The project the grid keeps showing although the chips exclude it, or null.
 * The exception lasts only until the first filter change, so a deliberate
 * re-filter hides the card. The filter store replaces its Set on every
 * change, so identity against the Set captured with the card is the signal —
 * read rather than tracked in an effect, which would paint the stale card
 * once under the new filters before removing it.
 */
export function gridStickyProjectId(activeFilters: ReadonlySet<string>): string | null {
  return projectsListViewState.stickyFilters === activeFilters
    ? projectsListViewState.stickyProjectId
    : null;
}
