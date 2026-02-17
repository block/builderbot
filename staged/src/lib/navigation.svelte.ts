/**
 * Lightweight client-side navigation state.
 *
 * Controls which view is shown in the main content area:
 * - `selectedProjectId === null` → ProjectsList (landing page)
 * - `selectedProjectId === <id>` → ProjectHome filtered to that project
 */

import { projectStateStore } from './stores/projectState.svelte';

export const navigation = $state({
  selectedProjectId: null as string | null,
});

/** Navigate to a specific project's detail view. */
export function selectProject(projectId: string): void {
  navigation.selectedProjectId = projectId;
  // Mark the project as read when navigating to it
  projectStateStore.markAsRead(projectId);
}

/** Navigate to a project and scroll to a specific branch card. */
export function selectProjectAndBranch(projectId: string, branchId: string): void {
  const alreadyOnProject = navigation.selectedProjectId === projectId;
  navigation.selectedProjectId = projectId;
  // Mark the project as read when navigating to it
  projectStateStore.markAsRead(projectId);
  if (alreadyOnProject) {
    // Already mounted, scroll immediately.
    window.dispatchEvent(new CustomEvent('staged:scroll-to-branch', { detail: { branchId } }));
  } else {
    // Allow ProjectHome to mount and register its scroll-to-branch listener.
    setTimeout(() => {
      window.dispatchEvent(new CustomEvent('staged:scroll-to-branch', { detail: { branchId } }));
    }, 150);
  }
}

/** Navigate back to the projects list (landing page). */
export function goHome(): void {
  navigation.selectedProjectId = null;
}
