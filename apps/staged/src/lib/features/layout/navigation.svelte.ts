/**
 * Lightweight client-side navigation state.
 *
 * Detail views are modeled as an app-owned stack. Compatibility fields are
 * still exposed for existing callers, but they are synchronized from the
 * current route instead of owning navigation.
 *
 * The last viewed project is persisted so the user returns to it on relaunch.
 * Only the first window (label `main`) and web mode persist it; secondary
 * windows are seeded with their opener's selected project and persist nothing,
 * so concurrent windows can't fight over the one key.
 */

import { untrack } from 'svelte';
import { getStoreValue, initPersistentStore, setStoreValue } from '../../shared/persistentStore';
import { getWindowLabel } from '../../transport';
import { takeWindowSeed } from '../../commands';
import {
  readSnapshot,
  writeSnapshot,
  clearSnapshot,
  SNAPSHOT_KEYS,
} from '../../shared/webSnapshot';
import type { DiffScope } from '../../commands';
import type { CommitTimelineItem } from '../../types';
import { projectStateStore } from '../../stores/projectState.svelte';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { requestProjectsListRestore } from '../projects/projectsListViewState.svelte';

const LAST_PROJECT_STORE_KEY = 'last-viewed-project';

/**
 * How long the first workspace render waits for the persisted route. The
 * store read runs alongside initPreferences' own sequential store round trips,
 * so it normally settles first; the bound only matters if the store hangs.
 */
export const INITIAL_ROUTE_TIMEOUT_MS = 300;

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

function initialProjectId(): string | null {
  const label = getWindowLabel();
  // localStorage is shared by Tauri webviews. Only `main` owns the persisted
  // snapshot; a secondary window must start empty until it consumes its
  // explicit backend seed. In web mode there is no label and the synchronous
  // snapshot remains the cold-boot accelerator.
  if (label && label !== 'main') return null;
  return readSnapshot<string>(SNAPSHOT_KEYS.lastProject);
}

const initialProject = initialProjectId();
const initialDetailStack: DetailRoute[] = initialProject
  ? [rootRoute(), { kind: 'project', projectId: initialProject }]
  : [rootRoute()];

export const navigation = $state({
  activeView: 'workspace' as 'workspace' | 'settings',
  // Web mode seeds synchronously from the localStorage mirror so <ProjectHome>
  // and the Back affordance can paint on the first frame. The snapshot is a
  // no-op under Tauri (webSnapshot.ts), where the stack starts at the root and
  // restoreInitialRoute pushes the persisted project before the workspace
  // renders; `listProjects()` validation follows in initNavigation.
  selectedProjectId: initialProject,
  showReposList: false,
  settingsSection: 'general' as SettingsSection,
  detailStack: initialDetailStack,
  currentRoute: initialDetailStack[initialDetailStack.length - 1] ?? rootRoute(),
  canGoBack: initialDetailStack.length > 1,
  // Settings and the diff route render over the workspace, which stays mounted
  // underneath (App.svelte). The one predicate for "covered", so the gates on
  // window-level requests the parked views would otherwise still answer can't
  // drift apart.
  workspaceParked: false,
  // Flips once restoreInitialRoute has either pushed the persisted project or
  // decided there is none. App.svelte holds the workspace back until then, so
  // a cold start into a project never mounts (and fetches for) the home grid
  // only to replace it a moment later.
  initialRouteSettled: false,
});

/**
 * Run `close` when the workspace gets parked under settings or the diff view.
 * Its components stay mounted then (App.svelte), so a dialog, alert or
 * popover one of them owns would stay open above the view on top — before the
 * workspace was kept mounted, unmounting closed them. Call from component
 * setup with whatever unmounting used to close.
 */
export function onWorkspaceParked(close: () => void): void {
  $effect(() => {
    if (!navigation.workspaceParked) return;
    untrack(close);
  });
}

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

/**
 * The route the parked workspace is showing: the nearest one below any
 * settings / diff routes on top. Those render over the workspace rather than
 * replacing it, so the workspace's inputs must come from here — deriving them
 * from the top route remounted the view underneath (repos → settings swapped
 * ReposListView for ProjectsList under the cover, and its top bar with it).
 */
function workspaceRoute(stack = navigation.detailStack): DetailRoute {
  for (let i = stack.length - 1; i >= 0; i--) {
    const route = stack[i];
    if (route.kind !== 'settings' && route.kind !== 'diff') return route;
  }
  return rootRoute();
}

function syncCompatibilityState(): void {
  const route = currentRoute();
  const base = workspaceRoute();
  navigation.currentRoute = route;
  navigation.canGoBack = navigation.detailStack.length > 1;
  navigation.workspaceParked = route.kind === 'settings' || route.kind === 'diff';
  navigation.activeView = route.kind === 'settings' ? 'settings' : 'workspace';
  if (route.kind === 'settings') navigation.settingsSection = route.section;

  navigation.showReposList = base.kind === 'repos';
  // A diff opened without a project route under it (an arbitrary route from a
  // reference) still names its project through lastProjectRouteProjectId.
  navigation.selectedProjectId =
    base.kind === 'project' ? base.projectId : lastProjectRouteProjectId();
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
 *
 * Only the first window (label `main`) and web mode persist. Labels are minted
 * fresh each launch, so a secondary window's key would never be read back —
 * and since a new window opens on its opener's project rather than a restored
 * one, writing at all would only accumulate orphan `…:win-N` entries in
 * `preferences.json`.
 */
function persistLastProject(projectId: string | null): void {
  // Web mode has no window label; `null` collapses to `main` and persists.
  if ((getWindowLabel() ?? 'main') !== 'main') return;
  setStoreValue(LAST_PROJECT_STORE_KEY, projectId);
  // Mirror to localStorage for the synchronous cold-boot restore (web only).
  if (projectId) writeSnapshot(SNAPSHOT_KEYS.lastProject, projectId);
  else clearSnapshot(SNAPSHOT_KEYS.lastProject);
}

/**
 * Put the persisted route in place before the first workspace render.
 *
 * Only the first window (label `main`) and web mode restore. A stack already
 * seeded from the web snapshot is final; otherwise the persisted project is
 * read from the store (initialising it if initPreferences hasn't yet — the two
 * share one init) and pushed if the user hasn't navigated meanwhile. Settles
 * `navigation.initialRouteSettled` whatever happens.
 *
 * Store init is awaited unbounded: `<main>` is also gated on
 * `preferences.loaded`, which waits on the same init promise, so settling
 * early during init would only revert to the home-grid flash without
 * rendering anything sooner. Only the key read itself is bounded by
 * INITIAL_ROUTE_TIMEOUT_MS; a read that lands later is left to
 * initNavigation, which restores a pristine stack the slow way.
 */
export async function restoreInitialRoute(): Promise<void> {
  if (navigation.initialRouteSettled) return;
  const label = getWindowLabel();
  if ((label && label !== 'main') || navigation.detailStack.length > 1) {
    navigation.initialRouteSettled = true;
    return;
  }

  let timer: ReturnType<typeof setTimeout> | undefined;
  const timedOut = Symbol('timeout');
  try {
    await initPersistentStore();
    const read = getStoreValue<string | null>(LAST_PROJECT_STORE_KEY);
    const lastProjectId = await Promise.race([
      read,
      new Promise<typeof timedOut>((resolve) => {
        timer = setTimeout(() => resolve(timedOut), INITIAL_ROUTE_TIMEOUT_MS);
      }),
    ]);
    if (lastProjectId === timedOut) {
      console.warn('[Navigation] Persisted route not read in time, rendering the first route');
      // Swallow the late result; initNavigation re-reads it.
      read.catch(() => {});
    } else if (lastProjectId && navigation.detailStack.length === 1) {
      setDetailStack([rootRoute(), { kind: 'project', projectId: lastProjectId }]);
    }
  } catch (e) {
    console.warn('[Navigation] Could not read the persisted route:', e);
  } finally {
    clearTimeout(timer);
    // Same synchronous step as the push above, so no render can observe the
    // settled flag with the stack still at the root.
    navigation.initialRouteSettled = true;
  }
}

/**
 * Restore the last viewed project on app launch.
 *
 * Must be called after `initPersistentStore()` (which is done inside
 * `initPreferences()`) and after `restoreInitialRoute()`, which normally has
 * already pushed the persisted project so this only validates it. If the
 * stored project no longer exists the user is sent to the home screen instead.
 *
 * Only the first window (label `main`) restores the persisted route on cold
 * start; a secondary window opens on the project its opener seeded it with.
 */
export async function initNavigation(): Promise<void> {
  const label = getWindowLabel();
  if (label && label !== 'main') {
    const seedProjectId = await takeWindowSeed().catch(() => null);
    if (!seedProjectId) return;
    // Validate against the project list like the restore path below — the
    // project could have been deleted between opening and initializing.
    await projectsDataStore.whenLoaded();
    if (projectsDataStore.loaded && !projectsDataStore.projects.some((p) => p.id === seedProjectId))
      return;
    setDetailStack([rootRoute(), { kind: 'project', projectId: seedProjectId }]);
    return;
  }

  const seeded = navigation.detailStack.length > 1;
  // `selectedProjectId` is normally already set, from the localStorage mirror
  // (web cold boot) or by restoreInitialRoute. Fall back to the async persistent
  // store otherwise (it is the source of truth) — e.g. the restore timed out.
  // Either way the first route has rendered and this only *corrects* it if the
  // project turns out to be gone.
  const lastProjectId =
    navigation.selectedProjectId ?? (await getStoreValue<string | null>(LAST_PROJECT_STORE_KEY));
  if (!lastProjectId) return;

  // Validate the project still exists; this runs in the background relative to
  // the first paint, which already shows the restored project.
  await projectsDataStore.whenLoaded();
  if (!projectsDataStore.loaded) {
    // If we can't list projects (e.g. store error), keep whatever we restored.
    console.warn('[Navigation] Could not verify last project, keeping restored route');
    return;
  }
  const existingIds = new Set(projectsDataStore.projects.map((p) => p.id));
  if (existingIds.has(lastProjectId)) {
    // Web: the seeded stack is already right. Tauri: push the restored route,
    // but only if the user hasn't navigated while we were validating.
    if (!seeded && navigation.detailStack.length === 1) {
      setDetailStack([rootRoute(), { kind: 'project', projectId: lastProjectId }]);
    }
  } else if (
    (!seeded && navigation.detailStack.length === 1) ||
    navigation.detailStack.some(
      (route) => route.kind === 'project' && route.projectId === lastProjectId
    )
  ) {
    // Project was deleted — back out to home and clear the persisted values.
    setDetailStack([rootRoute()]);
    await setStoreValue(LAST_PROJECT_STORE_KEY, null);
    clearSnapshot(SNAPSHOT_KEYS.lastProject);
  }
  // Remove unread entries for projects that no longer exist
  await projectStateStore.pruneDeletedProjects(existingIds);
}

/** Navigate to the repos list view. */
export function showAllRepos(): void {
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
  const projects = projectsDataStore.projects;
  const currentIndex = projects.findIndex((p) => p.id === navigation.selectedProjectId);
  if (currentIndex > 0) {
    selectProject(projects[currentIndex - 1].id);
  }
}

/** Navigate to the next project in the list. */
export function selectNextProject(): void {
  if (currentRoute().kind !== 'project' || !navigation.selectedProjectId || isModalOpen()) return;
  const projects = projectsDataStore.projects;
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

  const poppedRoute = currentRoute();
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

  if (poppedRoute.kind === 'diff') {
    window.dispatchEvent(
      new CustomEvent('staged:diff-route-popped', { detail: { route: poppedRoute } })
    );
  }
}

/** Pop the diff route only if the provided route is still the current route. */
export function closeDiffRouteIfCurrent(expectedRoute: DiffDetailRoute): void {
  if (currentRoute() !== expectedRoute) return;
  popDetailRoute();
}
