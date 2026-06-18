/**
 * Lightweight client-side navigation state.
 *
 * Detail views are modeled as an app-owned stack. Compatibility fields are
 * still exposed for existing callers, but they are synchronized from the
 * current route instead of owning navigation.
 *
 * The last viewed project is persisted so the user returns to it on relaunch.
 */

import { getStoreValue, setStoreValue } from '../../shared/persistentStore';
import * as commands from '../../api/commands';
import type { DiffScope } from '../../commands';
import type { CommitTimelineItem } from '../../types';
import { projectStateStore } from '../../stores/projectState.svelte';
import { projectsList } from '../projects/projectsSidebarState.svelte';
import { requestProjectsListRestore } from '../projects/projectsListViewState.svelte';
import { reposUiEnabled } from '../../featureFlags';

const LAST_PROJECT_STORE_KEY = 'last-viewed-project';

export type SettingsSection = 'general' | 'repo' | 'keyboard' | 'doctor';

export type DiffDetailRoute = {
  kind: 'diff';
  branchId: string;
  projectId?: string | null;
  commitSha?: string;
  scope?: DiffScope;
  reviewId?: string;
  beforeLabel?: string;
  afterLabel?: string;
  readonly?: boolean;
  commits?: CommitTimelineItem[];
  baseBranchLabel?: string;
  branchLabel?: string;
  projectName?: string;
  githubRepo?: string;
  subpath?: string | null;
};

export type DetailRoute =
  | { kind: 'projects' }
  | { kind: 'repos' }
  | { kind: 'project'; projectId: string }
  | { kind: 'settings'; section: SettingsSection }
  | DiffDetailRoute;

function rootRoute(): DetailRoute {
  return { kind: 'projects' };
}

export const navigation = $state({
  activeView: 'workspace' as 'workspace' | 'settings',
  selectedProjectId: null as string | null,
  showReposList: false,
  settingsSection: 'general' as SettingsSection,
  detailStack: [rootRoute()] as DetailRoute[],
  currentRoute: rootRoute() as DetailRoute,
  canGoBack: false,
});

function currentRoute(): DetailRoute {
  return navigation.detailStack[navigation.detailStack.length - 1] ?? rootRoute();
}

function lastProjectRouteProjectId(stack = navigation.detailStack): string | null {
  for (let i = stack.length - 1; i >= 0; i--) {
    const route = stack[i];
    if (route.kind === 'project') return route.projectId;
    if (route.kind === 'diff' && route.projectId) return route.projectId;
  }
  return null;
}

function syncCompatibilityState(): void {
  const route = currentRoute();
  navigation.currentRoute = route;
  navigation.canGoBack = navigation.detailStack.length > 1;

  if (route.kind === 'settings') {
    navigation.activeView = 'settings';
    navigation.selectedProjectId = lastProjectRouteProjectId();
    navigation.showReposList = false;
    navigation.settingsSection = route.section;
    return;
  }

  navigation.activeView = 'workspace';
  navigation.showReposList = route.kind === 'repos';
  navigation.selectedProjectId =
    route.kind === 'project'
      ? route.projectId
      : route.kind === 'diff'
        ? (route.projectId ?? lastProjectRouteProjectId())
        : null;
}

function setDetailStack(routes: DetailRoute[]): void {
  navigation.detailStack = routes.length > 0 ? routes : [rootRoute()];
  syncCompatibilityState();
}

function replaceCurrentRoute(route: DetailRoute): void {
  setDetailStack([...navigation.detailStack.slice(0, -1), route]);
}

function pushOrReplaceRoute(route: DetailRoute): void {
  const current = currentRoute();
  if (current.kind === route.kind) {
    replaceCurrentRoute(route);
  } else {
    setDetailStack([...navigation.detailStack, route]);
  }
}

/**
 * Persist the current navigation target.
 * Saves `null` for the home screen or the project ID.
 */
function persistLastProject(projectId: string | null): void {
  setStoreValue(LAST_PROJECT_STORE_KEY, projectId);
}

/**
 * Restore the last viewed project on app launch.
 *
 * Must be called after `initPersistentStore()` (which is done inside
 * `initPreferences()`). If the stored project no longer exists the
 * user is sent to the home screen instead.
 */
export async function initNavigation(): Promise<void> {
  const lastProjectId = await getStoreValue<string | null>(LAST_PROJECT_STORE_KEY);
  if (!lastProjectId) return;

  // Validate the project still exists before navigating to it
  try {
    const projects = await commands.listProjects();
    projectsList.current = projects;
    const existingIds = new Set(projects.map((p) => p.id));
    if (existingIds.has(lastProjectId)) {
      setDetailStack([rootRoute(), { kind: 'project', projectId: lastProjectId }]);
    } else {
      // Project was deleted — clear the stale value
      await setStoreValue(LAST_PROJECT_STORE_KEY, null);
    }
    // Remove unread entries for projects that no longer exist
    await projectStateStore.pruneDeletedProjects(existingIds);
  } catch {
    // If we can't list projects (e.g. store error), stay on home
    console.warn('[Navigation] Could not verify last project, falling back to home');
  }
}

/** Navigate to the repos list view. */
export function showAllRepos(): void {
  if (!reposUiEnabled) return;
  pushOrReplaceRoute({ kind: 'repos' });
}

/** Navigate to a specific project's detail view. */
export function selectProject(projectId: string): void {
  const current = currentRoute();
  if (current.kind === 'projects') {
    setDetailStack([rootRoute(), { kind: 'project', projectId }]);
  } else if (current.kind === 'project' || current.kind === 'repos') {
    replaceCurrentRoute({ kind: 'project', projectId });
  } else if (current.kind === 'settings') {
    setDetailStack([...navigation.detailStack.slice(0, -1), { kind: 'project', projectId }]);
  } else {
    setDetailStack([rootRoute(), { kind: 'project', projectId }]);
  }
  persistLastProject(projectId);
  // Mark the project as read when navigating to it, but only if it's not already read
  if (projectStateStore.isUnread(projectId)) {
    projectStateStore.markAsRead(projectId);
  }
}

/** Navigate to a project and scroll to a specific branch card. */
export function selectProjectAndBranch(projectId: string, branchId: string): void {
  const alreadyOnProject = navigation.selectedProjectId === projectId;
  selectProject(projectId);
  // Mark the project as read when navigating to it, but only if it's not already read
  if (projectStateStore.isUnread(projectId)) {
    projectStateStore.markAsRead(projectId);
  }
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

/** Returns true when a modal or diff overlay is open. */
function isModalOpen(): boolean {
  return currentRoute().kind === 'diff' || !!document.querySelector('[role="dialog"]');
}

/** Navigate to the previous project in the list. */
export function selectPreviousProject(): void {
  if (currentRoute().kind !== 'project' || !navigation.selectedProjectId || isModalOpen()) return;
  const projects = projectsList.current;
  const currentIndex = projects.findIndex((p) => p.id === navigation.selectedProjectId);
  if (currentIndex > 0) {
    selectProject(projects[currentIndex - 1].id);
  }
}

/** Navigate to the next project in the list. */
export function selectNextProject(): void {
  if (currentRoute().kind !== 'project' || !navigation.selectedProjectId || isModalOpen()) return;
  const projects = projectsList.current;
  const currentIndex = projects.findIndex((p) => p.id === navigation.selectedProjectId);
  if (currentIndex >= 0 && currentIndex < projects.length - 1) {
    selectProject(projects[currentIndex + 1].id);
  }
}

/** Navigate back to the projects list (landing page). */
export function goHome(): void {
  requestProjectsListRestore(navigation.selectedProjectId);
  setDetailStack([rootRoute()]);
  persistLastProject(null);
}

/** Show the dedicated settings view and select a settings section. */
export function openSettings(section: SettingsSection = 'general'): void {
  pushOrReplaceRoute({ kind: 'settings', section });
}

/** Return from settings to the workspace view (project or home). */
export function closeSettings(): void {
  if (currentRoute().kind === 'settings') {
    popDetailRoute();
  }
}

/** Open the diff detail route from a branch card or timeline item. */
export function openDiffRoute(route: Omit<DiffDetailRoute, 'kind'>): void {
  const diffRoute: DiffDetailRoute = { kind: 'diff', ...route };
  const current = currentRoute();

  if (current.kind === 'diff') {
    replaceCurrentRoute(diffRoute);
  } else if (current.kind === 'project' && route.projectId === current.projectId) {
    setDetailStack([...navigation.detailStack, diffRoute]);
  } else if (route.projectId) {
    setDetailStack([rootRoute(), { kind: 'project', projectId: route.projectId }, diffRoute]);
  } else {
    setDetailStack([...navigation.detailStack, diffRoute]);
  }

  if (route.projectId) {
    persistLastProject(route.projectId);
  }
}

/** Pop one app-level detail route when possible. */
export function popDetailRoute(): void {
  if (navigation.detailStack.length <= 1) return;

  const previousProjectId = navigation.selectedProjectId;
  const nextStack = navigation.detailStack.slice(0, -1);
  setDetailStack(nextStack);

  const nextRoute = currentRoute();
  if (nextRoute.kind === 'projects') {
    requestProjectsListRestore(previousProjectId);
    persistLastProject(null);
  } else if (nextRoute.kind === 'project') {
    persistLastProject(nextRoute.projectId);
  }
}
