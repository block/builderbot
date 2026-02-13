/**
 * Lightweight client-side navigation state.
 *
 * Controls which view is shown in the main content area:
 * - `selectedProjectId === null` → ProjectsList (landing page)
 * - `selectedProjectId === <id>` → ProjectHome filtered to that project
 */

export const navigation = $state({
  selectedProjectId: null as string | null,
});

/** Navigate to a specific project's detail view. */
export function selectProject(projectId: string): void {
  navigation.selectedProjectId = projectId;
}

/** Navigate back to the projects list (landing page). */
export function goHome(): void {
  navigation.selectedProjectId = null;
}
