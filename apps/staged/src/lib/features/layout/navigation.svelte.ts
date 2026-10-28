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
import { useWorkspaceLayer } from './workspaceLayer';

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

/**
 * The non-home view the workspace's project layer holds: the last project or
 * repos base route. It outlives the base route so the layer can stay mounted
 * (covered or clipped) while home is shown.
 */
export type ProjectLayer = { kind: 'project'; projectId: string } | { kind: 'repos' };

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
  // The workspace is two stacked layers (App.svelte): the home grid under the
  // project view. These say which one the base route — the workspace route
  // under any settings / diff on top — shows, and what each layer holds.
  //
  // homeActive: the base route is the projects grid.
  homeActive: !initialProject,
  // homeLayerRetained: sticky once home has been the base route of a rendered
  // workspace; the grid stays mounted (covered) under later projects. Seeded
  // false even when the stack starts at the root: under Tauri it sits there
  // until restoreInitialRoute pushes the persisted project, and counting that
  // would mount the grid under a project it was never shown before.
  homeLayerRetained: false,
  // projectLayer: the last project / repos base route, kept while home is
  // active so the project layer can be retained rather than rebuilt.
  projectLayer: (initialProject
    ? { kind: 'project', projectId: initialProject }
    : null) as ProjectLayer | null,
});

/**
 * Run `close` when the view calling this leaves the screen while it stays
 * mounted: its workspace layer goes inactive (home shown over a retained
 * project, or a project over the retained grid) or the workspace is parked
 * under settings / the diff view. A dialog, alert or popover the view owns
 * would otherwise stay open above what replaced it — before views were kept
 * mounted, unmounting closed them. Call from component setup with whatever
 * unmounting used to close.
 */
export function onViewHidden(close: () => void): void {
  const layer = useWorkspaceLayer();
  $effect(() => {
    if (layer.visible) return;
    untrack(close);
  });
}

/**
 * Run `refresh` each time the calling view's layer comes back on screen
 * (inactive → active), for the work a remount used to redo per visit. Not run
 * for the mount itself, even when the view mounts into a layer that has been
 * re-activated before: the mount path already does its own loading.
 */
export function onLayerActivated(refresh: () => void): void {
  const layer = useWorkspaceLayer();
  let seen = untrack(() => layer.activations);
  $effect(() => {
    const activations = layer.activations;
    if (activations === seen) return;
    seen = activations;
    untrack(refresh);
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

  navigation.homeActive = base.kind === 'projects';
  retainHomeLayerIfActive();
  // Replace only when the view differs so the object's identity is stable:
  // the layer's consumers key on it, and a settings / diff push over a project
  // re-syncs without changing the base.
  const layer = navigation.projectLayer;
  if (base.kind === 'project') {
    if (!(layer?.kind === 'project' && layer.projectId === base.projectId))
      navigation.projectLayer = { kind: 'project', projectId: base.projectId };
  } else if (base.kind === 'repos') {
    if (layer?.kind !== 'repos') navigation.projectLayer = { kind: 'repos' };
  }
}

/**
 * Home becomes retained the first time it is the base route of a workspace
 * that renders — i.e. once the initial route has settled (App.svelte gates
 * <main> on that), so a root stack waiting on restoreInitialRoute doesn't count.
 */
function retainHomeLayerIfActive(): void {
  if (navigation.homeActive && navigation.initialRouteSettled) navigation.homeLayerRetained = true;
}

/**
 * Mark the initial route settled and retain the home layer if the grid is the
 * base route at that point. Every settle path must go through here: a window
 * that settles at the root without retaining the grid — e.g. a secondary
 * window whose seed never lands — renders two empty layers and stays blank,
 * with no Back affordance at the root to trigger the goHome that would.
 */
function settleInitialRoute(): void {
  navigation.initialRouteSettled = true;
  retainHomeLayerIfActive();
}

/**
 * Drop the retained project layer for `projectId` — because the project was
 * deleted, say — so the layer unmounts rather than showing a missing project.
 * Only while home is active: on a project the layer is the view on screen, and
 * navigation owns what replaces it.
 */
export function releaseProjectLayer(projectId: string): void {
  const layer = navigation.projectLayer;
  if (!navigation.homeActive || layer?.kind !== 'project' || layer.projectId !== projectId) return;
  navigation.projectLayer = null;
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
    settleInitialRoute();
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
    settleInitialRoute();
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
