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
