export const projectsListViewState = $state({
  scrollTop: 0,
  returnTargetProjectId: null as string | null,
  restorePending: false,
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
