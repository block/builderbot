/**
 * Lightweight client-side navigation state.
 *
 * Navigation is a browser-style history (navigationHistory.ts), held here in
 * `navigation.history` and nowhere else. Each entry is a location: the
 * workspace route (the projects grid, a project, the repos list, a board)
 * with any settings / diff routes showing over it. Going somewhere adds an
 * entry after the current one and drops the ones forward of it; Back and
 * Forward step through them (goBack, goForward). No view sits "above"
 * another: Back from a project is wherever the user was before it, which is
 * the grid only when that is where they came from. The grid is reached by
 * going to it (goHome), like any other view.
 *
 * Back and Forward never land on something that is gone. Entries for deleted
 * projects, unpinned boards and diffs of deleted branches or reviews are
 * dropped before the step (stepHistory), and a diff entry is brought up to
 * date with its branch before it is shown again (diffRouteRefresh.ts).
 *
 * The other fields of `navigation` are still exposed for existing callers,
 * but they are synchronized from the current entry instead of owning
 * navigation.
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
import { tagsStore } from '../../stores/tags.svelte';
import { viewport } from '../../shared/viewport.svelte';
import { refreshDiffRoute } from './diffRouteRefresh';
import { requestProjectsListRestore } from '../projects/projectsListViewState.svelte';
import { useWorkspaceLayer } from './workspaceLayer';
import {
  canGoBack,
  canGoForward,
  createHistory,
  currentEntry,
  goBack as historyBack,
  goForward as historyForward,
  replaceCurrent,
  rewriteEntries,
  visit,
  type NavigationHistory,
} from './navigationHistory';

const LAST_PROJECT_STORE_KEY = 'last-viewed-project';

/**
 * How long the first workspace render waits for the persisted route. The
 * store read runs alongside initPreferences' own sequential store round trips,
 * so it normally settles first; the bound only matters if the store hangs.
 */
export const INITIAL_ROUTE_TIMEOUT_MS = 300;

export type SettingsSection =
  'general' | 'repo' | 'tags' | 'keyboard' | 'doctor' | 'connected-apps';

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
  /** A tag group's board, opened from its pinned row in the sidebar. */
  | { kind: 'board'; groupId: string }
  | { kind: 'settings'; section: SettingsSection }
  | DiffDetailRoute;

/**
 * A view the app can show, and one history entry. Bottom first: the workspace
 * route, then any settings / diff routes showing over it. A project is
 * `[project]`, and Settings opened from it is `[project, settings]`.
 */
export type ViewLocation = DetailRoute[];

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

/**
 * The history of a window that opens on a project: restored from the last
 * session, or seeded by the window that opened it.
 *
 * With a sidebar the project is the only entry. The grid was never visited,
 * so it is not a step Back, and the sidebar's "All projects" is how it is
 * reached. The mobile layout has no sidebar, so there the grid is put under
 * the project and Back reaches it in one tap, as the labelled Back button
 * that used to be in the bar's place did. The layout is read once, as the
 * window opens: a history is not rewritten because the window was resized.
 */
function openingHistory(projectId: string): NavigationHistory<ViewLocation> {
  const project: ViewLocation = [{ kind: 'project', projectId }];
  if (!viewport.isMobile) return createHistory(project);
  return { entries: [[rootRoute()], project], index: 1 };
}

const initialProject = initialProjectId();
const initialHistory: NavigationHistory<ViewLocation> = initialProject
  ? openingHistory(initialProject)
  : createHistory([rootRoute()]);
const initialLocation = currentEntry(initialHistory);

/**
 * The non-home view the workspace's project layer holds: the last project,
 * repos or board base route. It outlives the base route so the layer can stay
 * mounted (covered or clipped) while home is shown.
 */
export type ProjectLayer =
  { kind: 'project'; projectId: string } | { kind: 'repos' } | { kind: 'board'; groupId: string };

export const navigation = $state({
  activeView: 'workspace' as 'workspace' | 'settings',
  // Web mode seeds synchronously from the localStorage mirror so <ProjectHome>
  // can paint on the first frame. The snapshot is a no-op under Tauri
  // (webSnapshot.ts), where the history starts at the root and
  // restoreInitialRoute swaps in the persisted project before the workspace
  // renders; `listProjects()` validation follows in initNavigation.
  selectedProjectId: initialProject,
  showReposList: false,
  settingsSection: 'general' as SettingsSection,
  // The one history. It starts at the location the window opens on, with
  // nothing to go Back to (but see openingHistory for the mobile layout).
  history: initialHistory,
  // The location on screen: the history's current entry.
  detailStack: initialLocation,
  currentRoute: initialLocation[initialLocation.length - 1],
  // Whether there is an earlier / a later history entry to step to.
  canGoBack: canGoBack(initialHistory),
  canGoForward: false,
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
  // false even when the history starts at the root: under Tauri it sits there
  // until restoreInitialRoute restores the persisted project, and counting that
  // would mount the grid under a project it was never shown before.
  homeLayerRetained: false,
  // projectLayer: the last project / repos base route, kept while home is
  // active so the project layer can be retained rather than rebuilt.
  projectLayer: (initialProject
    ? { kind: 'project', projectId: initialProject }
    : null) as ProjectLayer | null,
});

// The history holds the initial location too. Read it back out so
// `detailStack` and the history's entry are one (proxied) array from the
// start, as they are after every navigation: routes are compared by identity.
navigation.detailStack = currentEntry(navigation.history);
navigation.currentRoute = navigation.detailStack[navigation.detailStack.length - 1];

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

/**
 * How long a source has to stay unchanged before a view in a hidden layer
 * follows it (followWhileVisible). It's longer than the gap between
 * keystrokes and shorter than the trip from a search field to the card or row
 * that brings the hidden layer back.
 */
export const HIDDEN_FOLLOW_DELAY_MS = 300;

/**
 * `read()`, for a view whose layer can be hidden. While the layer is visible,
 * `current` follows the source in the same flush. While it's hidden, `current`
 * follows only after the source has stayed unchanged for HIDDEN_FOLLOW_DELAY_MS.
 * The shared search query is the case in point. Following each keystroke
 * typed into the other layer's field would re-filter the hidden view per key.
 * Following none of them would leave all of that work for the flush that
 * brings the layer back: re-showing a hundred sidebar rows there added ~50 ms
 * to the navigation frame.
 *
 * A change still waiting when the layer becomes visible again is applied by
 * the `$effect.pre` in that flush, before the frame paints, so a view that
 * renders from `current` never shows a stale value. Call from component setup.
 */
export function followWhileVisible<T>(read: () => T): { readonly current: T } {
  const layer = useWorkspaceLayer();
  let value = $state.raw(untrack(read));
  $effect.pre(() => {
    const next = read();
    if (layer.visible) {
      value = next;
      return;
    }
    const timer = setTimeout(() => (value = next), HIDDEN_FOLLOW_DELAY_MS);
    return () => clearTimeout(timer);
  });
  return {
    get current() {
      return value;
    },
  };
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

function syncNavigationState(): void {
  const history = navigation.history;
  navigation.detailStack = currentEntry(history);
  navigation.canGoBack = canGoBack(history);
  navigation.canGoForward = canGoForward(history);
  const route = currentRoute();
  const base = workspaceRoute();
  navigation.currentRoute = route;
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
  } else if (base.kind === 'board') {
    if (!(layer?.kind === 'board' && layer.groupId === base.groupId))
      navigation.projectLayer = { kind: 'board', groupId: base.groupId };
  }
}

/**
 * Home becomes retained the first time it is the base route of a workspace
 * that renders — i.e. once the initial route has settled (App.svelte gates
 * <main> on that), so a root entry waiting on restoreInitialRoute doesn't count.
 */
function retainHomeLayerIfActive(): void {
  if (navigation.homeActive && navigation.initialRouteSettled) navigation.homeLayerRetained = true;
}

/**
 * Mark the initial route settled and retain the home layer if the grid is the
 * base route at that point. Every settle path must go through here: a window
 * that settles at the root without retaining the grid — e.g. a secondary
 * window whose seed never lands — renders two empty layers and stays blank,
 * with nothing at the root to trigger the goHome that would.
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

/**
 * Drop the retained board layer for `groupId` — because the group was deleted
 * or its board unpinned — so the layer unmounts rather than showing a board
 * that is no longer offered. Only while home is active, as for projects: on
 * screen, the board's route is what goes (leaveBoard).
 */
export function releaseBoardLayer(groupId: string): void {
  const layer = navigation.projectLayer;
  if (!navigation.homeActive || layer?.kind !== 'board' || layer.groupId !== groupId) return;
  navigation.projectLayer = null;
}

// ── The history ──

/** Whether two routes show the same view. A diff route is only ever the same
 *  as itself: opening one diff twice is two visits. */
function sameRoute(a: DetailRoute, b: DetailRoute): boolean {
  if (a === b) return true;
  if (a.kind === 'project') return b.kind === 'project' && a.projectId === b.projectId;
  if (a.kind === 'board') return b.kind === 'board' && a.groupId === b.groupId;
  if (a.kind === 'settings') return b.kind === 'settings' && a.section === b.section;
  if (a.kind === 'diff') return false;
  return a.kind === b.kind;
}

function sameLocation(a: ViewLocation, b: ViewLocation): boolean {
  return a.length === b.length && a.every((route, i) => sameRoute(route, b[i]));
}

/** Whether the location shows the project: as its workspace route, or in a
 *  diff of one of its branches. */
function locationShowsProject(location: ViewLocation, gone: (projectId: string) => boolean) {
  return location.some(
    (route) =>
      (route.kind === 'project' || route.kind === 'diff') &&
      !!route.projectId &&
      gone(route.projectId)
  );
}

function isDiffRoute(route: DetailRoute): route is DiffDetailRoute {
  return route.kind === 'diff';
}

/** Whether the window is still on the single root entry it started with:
 *  nothing restored or seeded, and the user hasn't navigated. */
function atPristineRoot(): boolean {
  return navigation.history.entries.length === 1 && currentRoute().kind === 'projects';
}

/**
 * Start the history over. For the route the window opens on (restored,
 * seeded or corrected at startup), which is not a navigation: nothing is
 * persisted or announced.
 */
function resetHistory(history: NavigationHistory<ViewLocation>): void {
  navigation.history = history;
  syncNavigationState();
}

/**
 * Make `next` the history, and do what arriving at its current location
 * calls for. Every navigation ends here, whether it added an entry, replaced
 * one, stepped Back or Forward or dropped entries, so these follow the view
 * on screen rather than the function that changed it:
 *
 * - A diff route that left the screen announces itself, `diffExit` saying
 *   how. `replaced` (`staged:diff-route-replaced`): the user went somewhere
 *   else, even if that is the project the diff was over. `closed`
 *   (`staged:diff-route-popped`): the diff was closed or stepped Back from,
 *   and the view is now what was under it, so a reference that led to the
 *   diff can come back; a step that lands anywhere else is `replaced`.
 *   `silent`: one diff standing in for another.
 * - The grid coming on screen restores its scroll position to the project
 *   the user came from.
 * - A change of workspace route is persisted as the last viewed project (the
 *   project, or none for the grid; the repos list and boards leave it), and
 *   a project coming on screen is marked read.
 */
function applyHistory(
  next: NavigationHistory<ViewLocation>,
  diffExit: 'replaced' | 'closed' | 'silent' = 'replaced'
): void {
  if (next === navigation.history) return;
  const previous = navigation.detailStack;
  const previousProjectId = navigation.selectedProjectId;
  navigation.history = next;
  syncNavigationState();
  const location = navigation.detailStack;
  if (location === previous) return;

  if (diffExit !== 'silent') {
    const closed = diffExit === 'closed' && sameLocation(location, previous.slice(0, -1));
    for (const route of previous) {
      if (route.kind !== 'diff' || location.some((other) => other === route)) continue;
      const type = closed ? 'staged:diff-route-popped' : 'staged:diff-route-replaced';
      window.dispatchEvent(new CustomEvent(type, { detail: { route } }));
    }
  }

  if (currentRoute().kind === 'projects' && previous[previous.length - 1].kind !== 'projects') {
    requestProjectsListRestore(previousProjectId);
  }

  const base = workspaceRoute(location);
  if (sameRoute(workspaceRoute(previous), base)) return;
  if (base.kind === 'projects') {
    persistLastProject(null);
  } else if (base.kind === 'project') {
    persistLastProject(base.projectId);
    if (projectStateStore.isUnread(base.projectId)) projectStateStore.markAsRead(base.projectId);
  }
}

/** Go to a location: a new history entry, unless it is the one on screen. */
function visitLocation(location: ViewLocation): void {
  applyHistory(visit(navigation.history, location, sameLocation));
}

/** Show a location in place of the current one, without a new history entry. */
function replaceLocation(location: ViewLocation, diffExit?: 'silent'): void {
  if (sameLocation(navigation.detailStack, location)) return;
  applyHistory(replaceCurrent(navigation.history, location), diffExit);
}

/**
 * Close the settings / diff route on top, showing what was under it. That is
 * a step Back when the user came from there, as they nearly always did, so
 * Forward reopens what was closed. Otherwise (a diff opened over a project
 * other than the one the user was on) it is a visit to what was under it.
 */
function closeTopRoute(): void {
  const location = navigation.detailStack;
  if (location.length <= 1) return;
  const beneath = location.slice(0, -1);
  const history = navigation.history;
  const earlier = history.entries[history.index - 1];
  if (earlier && sameLocation(earlier, beneath)) applyHistory(historyBack(history), 'closed');
  else applyHistory(visit(history, beneath, sameLocation), 'closed');
}

/** Drop the history entries that show a project `gone` says no longer
 *  exists, so Back and Forward never land on one. */
function forgetProjects(gone: (projectId: string) => boolean, keep?: ViewLocation): void {
  applyHistory(
    rewriteEntries(
      navigation.history,
      (location) => (location !== keep && locationShowsProject(location, gone) ? null : location),
      [rootRoute()],
      sameLocation
    )
  );
}

/**
 * Drop a project that is gone (deleted, here or elsewhere) from the history.
 * Callers showing the project navigate away first; if it is still the view
 * on screen, the nearest earlier entry takes its place, or the grid.
 */
export function forgetProject(projectId: string): void {
  forgetProjects((id) => id === projectId);
}

/**
 * The project a mounted project view shows is gone: deleted in another
 * window, or by anything else that didn't navigate away first. `onScreen`
 * says whether that view is the one showing. If so the window goes to the
 * grid; a view only retained under the grid has its layer released instead,
 * so it unmounts rather than show a missing project, and the view on screen
 * doesn't move. Either way the project leaves the history, so Back and
 * Forward don't return to it.
 */
export function leaveGoneProject(projectId: string, onScreen: boolean): void {
  if (onScreen) goHome();
  else releaseProjectLayer(projectId);
  forgetProject(projectId);
}

/**
 * Take the boards `gone` says are no longer offered out of the history.
 *
 * An entry that is the board alone is dropped. One with something over the
 * board (settings, where the unpinning most likely happened, or a diff)
 * keeps that over the grid instead, so the user isn't thrown out of what
 * they were doing. When the board itself is the view on screen, the nearest
 * earlier entry takes its place, as Back would, or the grid when there is
 * none. Whichever workspace route that leaves is what's persisted.
 */
function forgetBoards(gone: (groupId: string) => boolean): void {
  applyHistory(
    rewriteEntries(
      navigation.history,
      (location) => {
        const rest = location.filter((route) => !(route.kind === 'board' && gone(route.groupId)));
        if (rest.length === location.length) return location;
        if (rest.length === 0) return null;
        return [rootRoute(), ...rest];
      },
      [rootRoute()],
      sameLocation
    )
  );
}

/** Whether the board is one the sidebar offers: its group exists and its
 *  board is pinned, BoardView's rule for showing one. A tag catalog that has
 *  not loaded refuses nothing. */
function boardOffered(groupId: string): boolean {
  if (!tagsStore.loaded) return true;
  return !!tagsStore.catalog.groupsById.get(groupId)?.showInSidebar;
}

/** Drop the history entries showing a diff `gone` says can no longer be
 *  shown, other than the one on screen (the diff view there is the user's
 *  to close). */
function forgetDiffs(gone: (route: DiffDetailRoute) => boolean): void {
  const onScreen = navigation.detailStack;
  applyHistory(
    rewriteEntries(
      navigation.history,
      (location) =>
        location !== onScreen && location.some((route) => isDiffRoute(route) && gone(route))
          ? null
          : location,
      [rootRoute()],
      sameLocation
    )
  );
}

/**
 * Put `next` in the place of the diff route `stale`, in every history entry
 * that holds it. For a diff brought up to date before a step lands on it
 * (stepHistory), which is never the diff on screen.
 *
 * One diff route is shared by every entry built over it: Settings opened
 * from a diff leaves `[project, diff]` and `[project, diff, settings]`
 * holding the same object, and a diff route is the same as another only by
 * identity (sameRoute). Swapping it in one entry alone made those two
 * disagree: stepping between them announced a diff leaving the screen that
 * never left it, and closing Settings added an entry where it should have
 * stepped Back. So all of them get the one replacement.
 *
 * That replacement is made a state proxy here, once, before it goes into the
 * entries. `navigation` proxies what it holds lazily and per holder, so a
 * plain object put into two entries would be read back out of them as two
 * different proxies, and no longer be identical to itself.
 *
 * The swap is one diff standing in for another: no diff event is fired, so a
 * reference trail that led to the diff is still there to come back to.
 */
function replaceDiffRoute(stale: DiffDetailRoute, next: DiffDetailRoute): void {
  const route = $state(next);
  applyHistory(
    rewriteEntries(
      navigation.history,
      (location) =>
        location.includes(stale) ? location.map((r) => (r === stale ? route : r)) : location,
      [rootRoute()],
      sameLocation
    ),
    'silent'
  );
}

/**
 * One step Back or Forward, for goBack / goForward. Does nothing, and
 * resolves false, when there is no entry that way, and when stepping would
 * pull the view out from under something:
 *
 * - A dialog is open. Navigating would unmount the view that owns it along
 *   with whatever was typed into it (the rule openProjectExternally follows).
 * - The diff view could not save the comment being typed, so the text is
 *   still there to retry. Like the ⌘K palette, the step waits for that save
 *   first, and is dropped if the user navigated or opened a dialog meanwhile.
 *
 * A step never lands on something that is gone. Before it is taken:
 *
 * - Entries for projects deleted in another window are dropped (this
 *   window's own deletes drop theirs at once, see forgetProject), and so are
 *   entries for boards that are no longer offered. A board unpinned while it
 *   is mounted leaves through BoardView (leaveBoard); one that is only in
 *   the history has nothing mounted to notice, so it is caught here.
 * - When the entry the step would land on shows a diff, the backend is asked
 *   about it (refreshDiffRoute). A diff whose branch or review is gone is
 *   dropped, along with every other entry for that branch or review, and the
 *   step goes to the entry beyond it. Otherwise the diff is brought up to
 *   date with the branch, in every entry that shows it (replaceDiffRoute),
 *   so the diff view opens on today's commits rather than the ones it was
 *   first opened with. This is the one case where a step from a view other
 *   than the diff is not taken in the caller's tick.
 * - Not when the diff is already part of the view on screen: Settings over
 *   the diff stepping Back onto it, or the diff stepping Forward to Settings
 *   over it. The step leaves that route where it is, so nothing is asked
 *   and the step is taken in the caller's tick.
 */
async function stepHistory(
  step: (history: NavigationHistory<ViewLocation>) => NavigationHistory<ViewLocation>
): Promise<boolean> {
  if (dialogOpen()) return false;
  if (projectsDataStore.loaded) {
    forgetProjects((projectId) => !projectOpenable(projectId), navigation.detailStack);
  }
  forgetBoards((groupId) => !boardOffered(groupId));

  for (;;) {
    const from = navigation.history;
    const ahead = step(from);
    if (ahead === from) return false;
    const target = currentEntry(ahead);
    const diff = target.find(isDiffRoute);
    if (!diff || navigation.detailStack.includes(diff)) break;

    const refreshed = await refreshDiffRoute(diff);
    if (navigation.history !== from || dialogOpen()) return false;
    if (refreshed.status === 'unknown') break;
    if (refreshed.status === 'current') {
      replaceDiffRoute(diff, refreshed.route);
      break;
    }
    // Gone: drop it and look at the entry that is now ahead.
    const { branchId, reviewId } = diff;
    forgetDiffs(
      refreshed.what === 'branch'
        ? (route) => route.branchId === branchId
        : (route) => route.reviewId === reviewId
    );
    // Nothing was dropped, which cannot happen: the entry ahead holds the
    // diff and is not the one on screen. Stop rather than ask again forever.
    if (navigation.history === from) return false;
  }

  const diffSaved = diffLeaveCheck();
  if (diffSaved) {
    const from = navigation.history;
    let saved = false;
    try {
      saved = await diffSaved();
    } catch (e) {
      console.error('[Navigation] Could not save the diff comment before leaving:', e);
    }
    if (!saved || navigation.history !== from || dialogOpen()) return false;
  }

  applyHistory(step(navigation.history), 'closed');
  return true;
}

/** Step Back to the previous history entry, as a browser's Back button does.
 *  Resolves whether the view changed (see stepHistory for when it doesn't). */
export function goBack(): Promise<boolean> {
  return stepHistory(historyBack);
}

/** Step Forward to the next history entry: the one Back was last pressed on.
 *  Resolves whether the view changed. */
export function goForward(): Promise<boolean> {
  return stepHistory(historyForward);
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
 * Only the first window (label `main`) and web mode restore. A history already
 * seeded from the web snapshot is final; otherwise the persisted project is
 * read from the store (initialising it if initPreferences hasn't yet — the two
 * share one init) and becomes the window's first history entry if the user
 * hasn't navigated meanwhile. It replaces the root entry rather than being
 * visited from it: the window opens on the project, and Back has nowhere to
 * go (except in the mobile layout, see openingHistory). Settles
 * `navigation.initialRouteSettled` whatever happens.
 *
 * Store init is awaited unbounded: `<main>` is also gated on
 * `preferences.loaded`, which waits on the same init promise, so settling
 * early during init would only revert to the home-grid flash without
 * rendering anything sooner. Only the key read itself is bounded by
 * INITIAL_ROUTE_TIMEOUT_MS; a read that lands later is left to
 * initNavigation, which restores a pristine history the slow way.
 */
export async function restoreInitialRoute(): Promise<void> {
  if (navigation.initialRouteSettled) return;
  const label = getWindowLabel();
  if ((label && label !== 'main') || !atPristineRoot()) {
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
    } else if (lastProjectId && atPristineRoot()) {
      resetHistory(openingHistory(lastProjectId));
    }
  } catch (e) {
    console.warn('[Navigation] Could not read the persisted route:', e);
  } finally {
    clearTimeout(timer);
    // Same synchronous step as the restore above, so no render can observe
    // the settled flag with the history still at the root.
    settleInitialRoute();
  }
}

/**
 * Restore the last viewed project on app launch.
 *
 * Must be called after `initPersistentStore()` (which is done inside
 * `initPreferences()`) and after `restoreInitialRoute()`, which normally has
 * already restored the persisted project so this only validates it. If the
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
    resetHistory(openingHistory(seedProjectId));
    return;
  }

  const seeded = !atPristineRoot();
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
    // Web: the seeded history is already right. Tauri: restore the route,
    // but only if the user hasn't navigated while we were validating.
    if (!seeded && atPristineRoot()) {
      resetHistory(openingHistory(lastProjectId));
    }
  } else if (
    (!seeded && atPristineRoot()) ||
    navigation.detailStack.some(
      (route) => route.kind === 'project' && route.projectId === lastProjectId
    )
  ) {
    // Project was deleted — start over at home and clear the persisted values.
    resetHistory(createHistory([rootRoute()]));
    await setStoreValue(LAST_PROJECT_STORE_KEY, null);
    clearSnapshot(SNAPSHOT_KEYS.lastProject);
  }
  // Remove unread entries for projects that no longer exist
  await projectStateStore.pruneDeletedProjects(existingIds);
}

/** Navigate to the repos list view. */
export function showAllRepos(): void {
  visitLocation([{ kind: 'repos' }]);
}

/**
 * Navigate to a tag group's board (its pinned sidebar row). Like the repos
 * view it takes the project layer, and is not persisted as the last route.
 */
export function selectBoard(groupId: string): void {
  visitLocation([{ kind: 'board', groupId }]);
}

/**
 * The board for `groupId` is no longer offered (its group deleted, or its
 * board unpinned, here or in another window). Take it out of the history,
 * so Back and Forward never land on it (forgetBoards says what becomes of
 * each entry). A board only retained under home has no entry on screen; its
 * layer is released so it unmounts rather than show a board nobody offers.
 *
 * Called by the board's own view, which only notices while it is mounted.
 * A board that is in the history and nowhere else is dropped by the next
 * step (stepHistory).
 */
export function leaveBoard(groupId: string): void {
  forgetBoards((id) => id === groupId);
  releaseBoardLayer(groupId);
}

/**
 * Navigate to a specific project's detail view.
 *
 * The project is a new history entry, wherever it is opened from: Back
 * returns to the view the user was on, be that the grid, another project,
 * the repos list or a board. Opening the project already on screen changes
 * nothing.
 *
 * Settings and the diff view sit over the workspace rather than in it, and
 * the project is not opened under them: from Settings over project A, the
 * location becomes B alone, and Back returns to Settings over A. The ⌘K
 * palette is how a project gets opened from there.
 *
 * A diff route left this way announces itself (`staged:diff-route-replaced`),
 * where one that was closed or stepped Back from says
 * `staged:diff-route-popped`. Callers leaving the diff view from outside it
 * await diffLeaveCheck first.
 */
export function selectProject(projectId: string): void {
  visitLocation([{ kind: 'project', projectId }]);
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

/** Returns true when a modal or diff overlay is open. The ⌘K palette's rule
 *  (otherModalOpen in paletteState.svelte.ts) is the dialog half of this
 *  only: it opens over the diff view. */
function isModalOpen(): boolean {
  return currentRoute().kind === 'diff' || !!document.querySelector('[role="dialog"]');
}

/** Navigate to the previous project in the list, skipping any being deleted
 *  (they are already gone from the sidebar and the grid). */
export function selectPreviousProject(): void {
  if (currentRoute().kind !== 'project' || !navigation.selectedProjectId || isModalOpen()) return;
  const projects = projectsDataStore.liveProjects;
  const currentIndex = projects.findIndex((p) => p.id === navigation.selectedProjectId);
  if (currentIndex > 0) {
    selectProject(projects[currentIndex - 1].id);
  }
}

/** Navigate to the next project in the list, skipping any being deleted. */
export function selectNextProject(): void {
  if (currentRoute().kind !== 'project' || !navigation.selectedProjectId || isModalOpen()) return;
  const projects = projectsDataStore.liveProjects;
  const currentIndex = projects.findIndex((p) => p.id === navigation.selectedProjectId);
  if (currentIndex >= 0 && currentIndex < projects.length - 1) {
    selectProject(projects[currentIndex + 1].id);
  }
}

/**
 * Navigate to the projects list (landing page). A visit like any other: Back
 * from the grid returns to the view it was opened from.
 */
export function goHome(): void {
  visitLocation([rootRoute()]);
}

/**
 * Show the dedicated settings view and select a settings section. Opening
 * Settings is a history entry; switching sections inside it is not, so Back
 * leaves Settings rather than stepping through the sections looked at.
 */
export function openSettings(section: SettingsSection = 'general'): void {
  const location = navigation.detailStack;
  const route: DetailRoute = { kind: 'settings', section };
  if (currentRoute().kind === 'settings') {
    replaceLocation([...location.slice(0, -1), route]);
  } else {
    visitLocation([...location, route]);
  }
}

/** Return from settings to the workspace view under it (project or home). */
export function closeSettings(): void {
  if (currentRoute().kind === 'settings') {
    closeTopRoute();
  }
}

/**
 * Open the diff detail route from a branch card or timeline item.
 *
 * The diff shows over its project: over the view on screen when that is the
 * project (or the diff names none), and otherwise over the project's own
 * view, which the workspace switches to. From the diff view itself the new
 * diff takes the old one's place, in the same history entry (stepping
 * through references is the reference history's job, not this one's).
 */
export function openDiffRoute(route: Omit<DiffDetailRoute, 'kind'>): void {
  const diffRoute: DiffDetailRoute = { kind: 'diff', ...route };
  const location = navigation.detailStack;
  const current = currentRoute();

  if (current.kind === 'diff') {
    replaceLocation([...location.slice(0, -1), diffRoute], 'silent');
  } else if (
    route.projectId &&
    !(current.kind === 'project' && current.projectId === route.projectId)
  ) {
    visitLocation([{ kind: 'project', projectId: route.projectId }, diffRoute]);
  } else {
    visitLocation([...location, diffRoute]);
  }
}

/** Close the diff route only if the provided route is still the current route. */
export function closeDiffRouteIfCurrent(expectedRoute: DiffDetailRoute): void {
  if (currentRoute() !== expectedRoute) return;
  closeTopRoute();
}

// ── Leaving the diff view from outside it ──
//
// Comments typed into the diff view save as they are written, and the view's
// own ways out (Escape, its close button) wait for the last save and stay on
// the diff when it fails, so the text is still there to retry. A navigation
// the diff view didn't start (Back and Forward, the ⌘K palette opening a
// project) has to make the same check, or a failed save would go with the
// view.

/** Resolves true once what is typed into the diff view is saved, false when
 *  a save failed and the view should stay. */
export type DiffLeaveCheck = () => Promise<boolean>;

let registeredDiffLeaveCheck: DiffLeaveCheck | null = null;

/**
 * Called by the diff view while it is mounted, with the check its own close
 * runs. Returns the function that withdraws it.
 */
export function registerDiffLeaveCheck(check: DiffLeaveCheck): () => void {
  registeredDiffLeaveCheck = check;
  return () => {
    if (registeredDiffLeaveCheck === check) registeredDiffLeaveCheck = null;
  };
}

/**
 * The check to await before navigating away from the diff view, or null when
 * the route on screen isn't the diff (nothing to wait for, so the caller can
 * navigate in the same tick). Navigate only if it resolves true.
 */
export function diffLeaveCheck(): DiffLeaveCheck | null {
  return currentRoute().kind === 'diff' ? registeredDiffLeaveCheck : null;
}

// ── Opening a project for a caller outside the window ──
//
// `stagedctl project open` asks the backend to put a project on screen, and
// the backend hands the request to one window (project_open.rs), since only
// the window knows whether a dialog is open or a diff comment still needs
// saving. This is what that window does with it.

/**
 * What the window did with an outside request to show a project. The values
 * are the backend's `ProjectOpenAnswer` (src-tauri/src/project_open.rs) as it
 * deserializes them, so a result goes to `answer_project_open` unchanged.
 *
 * - `opened`: the project is on screen. The window switched to it, or was
 *   already showing it.
 * - `dialogOpen`: a dialog is open, and the project is unchanged.
 * - `diffCommentUnsaved`: the diff view could not save the comment being
 *   typed, so the window stayed on it.
 * - `projectNotFound`: the window knows no project with that id.
 */
export type ProjectOpenAnswer = 'opened' | 'dialogOpen' | 'diffCommentUnsaved' | 'projectNotFound';

/** Whether a dialog or alert dialog is open (design-system or hand-rolled,
 *  they all carry the role), not counting one that is only playing its exit
 *  animation. Unlike the palette's otherModalOpen, the ⌘K palette counts. */
function dialogOpen(): boolean {
  for (const dialog of document.querySelectorAll('[role="dialog"], [role="alertdialog"]')) {
    if (dialog.getAttribute('data-state') !== 'closed') return true;
  }
  return false;
}

/** Whether the window can show this project: it is in the list and has no
 *  delete in flight, the ⌘K palette's rule for a row it will open. A list that
 *  failed to load refuses nothing, as for a new window's seed (initNavigation):
 *  the backend checked the id against the database before it asked. */
function projectOpenable(projectId: string): boolean {
  if (!projectsDataStore.loaded) return true;
  return (
    projectsDataStore.projects.some((project) => project.id === projectId) &&
    !projectsDataStore.isProjectDeleting(projectId)
  );
}

/** The answer that needs no navigation, as things stand in this tick: the
 *  project cannot be shown, is already the view on screen, or is blocked by a
 *  dialog. Null when the project should be navigated to. */
function answerWithoutNavigating(projectId: string): ProjectOpenAnswer | null {
  if (!projectOpenable(projectId)) return 'projectNotFound';
  const route = currentRoute();
  if (route.kind === 'project' && route.projectId === projectId) return 'opened';
  if (dialogOpen()) return 'dialogOpen';
  return null;
}

/**
 * Show a project because something outside the window asked for it, and say
 * what happened. Unlike selectProject, which trusts its caller to have picked
 * a moment when navigating is safe, the request can land at any time, so this
 * makes the checks itself:
 *
 * - It waits for the first project list. The app may have been launched for
 *   this request, and the window still be loading.
 * - A project the window cannot show is `projectNotFound`, whatever else is
 *   going on, so the caller is not told to close a dialog and retry a request
 *   that can only fail.
 * - A project that is already the view on screen is `opened`, even under a
 *   dialog: there is nothing to navigate, so nothing for the dialog to block.
 * - Otherwise an open dialog refuses with `dialogOpen`. Navigating would
 *   unmount the view that owns it along with whatever was typed into it, and
 *   a pending confirmation would outlive the project it was asked about.
 * - Over the diff view it waits for the comment being typed to save, like the
 *   palette and the view's own close, and stays with `diffCommentUnsaved`
 *   when the save failed so the text is still there to retry.
 * - Last, it asks `stillWanted`, when one is given. The waits above take
 *   time, and whoever asked may have given up or been replaced meanwhile.
 *   When it resolves false nothing is navigated and the result is
 *   `withdrawn`, which is not an answer: there is nobody left to tell.
 *
 * Each wait is followed by a fresh look at the project and the dialogs, so
 * the navigation runs in the same tick as the checks it depends on. The
 * `stillWanted` round trip is the last wait: nothing is awaited between it
 * resolving true and the navigation. When it rejects, so does this, and
 * nothing is navigated.
 *
 * From Settings or the diff view the project takes their place, as
 * selectProject describes.
 */
export async function openProjectExternally(
  projectId: string,
  stillWanted?: () => Promise<boolean>
): Promise<ProjectOpenAnswer | 'withdrawn'> {
  await projectsDataStore.whenLoaded();
  let answer = answerWithoutNavigating(projectId);
  if (answer) return answer;

  const diffSaved = diffLeaveCheck();
  if (diffSaved) {
    let saved = false;
    try {
      saved = await diffSaved();
    } catch (e) {
      console.error('[Navigation] Could not save the diff comment before leaving:', e);
    }
    if (!saved) return 'diffCommentUnsaved';
    answer = answerWithoutNavigating(projectId);
    if (answer) return answer;
  }

  if (stillWanted) {
    if (!(await stillWanted())) return 'withdrawn';
    answer = answerWithoutNavigating(projectId);
    if (answer) return answer;
  }

  selectProject(projectId);
  return 'opened';
}
