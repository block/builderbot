/**
 * Shared project-list data store.
 *
 * Module-scoped runes store (following the repoBadgeStore pattern) that owns
 * the data every top-level view renders: the project list plus per-project
 * branches and repos. Because it lives at module scope the data survives
 * route changes — in the desktop app, where the SWR cache in cache.ts is
 * web-only (cachedInvoke/cachedCommand short-circuit to the network under
 * Tauri), this store is the in-memory cache that lets a revisited view paint
 * instantly instead of replaying the full IPC fetch cascade.
 *
 * ensureLoaded() applies the SwrResult render-stale-then-refresh contract in
 * both modes: the first call fetches the project list; later calls resolve
 * immediately with the in-memory data and kick a soft background revalidation
 * that does not discard in-flight per-project hydration. Readiness is two-level
 * so no view waits for data it doesn't paint — `loaded` means "the project list
 * landed", while per-project branches and repos are hydrated on demand
 * (hydrateProject / ensureProjectsHydrated) and through the idle queue for
 * everything nobody asked for. Views that need branch/repo details gate on
 * isProjectHydrated()/allProjectsHydrated rather than on `loaded`.
 *
 * Many projects are hydrated together: two all-projects reads, applied with
 * one assignment per map (see hydrateInBulk). Every map here is replaced
 * whole, so each assignment wakes every card, row and derived value that
 * reads it; applying one project at a time made that a pass over all of
 * them per project. One project on its own is still read on its own. That
 * holds for every path that can want many projects at once: a load, a
 * refresh, the projects a soft reload discovers, and the feed's lag
 * recovery.
 *
 * ProjectHome, ProjectsList, ProjectsSidebar, and ReposListView all read
 * this store directly; startListeners() is wired once from App.svelte.
 * View-lifecycle side effects (workspaceLifecycle.enqueueInitialSetup,
 * queued-session draining, run-action hydration) intentionally stay out of
 * the store — consuming views wire them by watching branchesByProject.
 *
 * A fetched project list keeps the object of every project it leaves
 * unchanged, and is not applied at all when it changes nothing (see
 * reuseUnchangedProjects), so a reload wakes the cards of the projects that
 * changed and no others.
 *
 * Authoritative freshness comes from the store change feed (project-changed,
 * branch-changed, repos-changed): every mutating backend store method
 * publishes, so a write in any window — or in the backend itself — refetches
 * here. A project-changed that names its project reloads the list and
 * refetches that one project. The feed's lag recovery, which names none,
 * reloads everything, as do the two other cases listed at
 * handleProjectChanged. The remaining imperative entry points (projectCreated, projectRenamed,
 * setBranchesByProject, refreshProject) exist for immediacy: they paint the
 * local window's own mutation without waiting a coalescing window for the
 * echo; the event-driven refetch then confirms. Project tags are not here:
 * tagsStore owns them, and a tag write never publishes project-changed.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import * as commands from '../commands';
import { repoBadgeStore } from './repoBadges.svelte';
import { LatestWins } from '../shared/latestWins';
import { reuseUnchangedRows } from '../shared/reuseUnchangedRows';
import type {
  Branch,
  BranchChangedEvent,
  ImageCreatedPayload,
  PrStatusChangedEvent,
  Project,
  ProjectChangedEvent,
  ProjectRepo,
  ProjectTransferDirection,
  ProjectTransferPhase,
  ProjectTransferProgress,
  RepoHomeItem,
  ReposChangedEvent,
  SessionStatusPayload,
} from '../types';

/**
 * requestIdleCallback timeout for each background hydration step. Not a fixed
 * delay: a step runs at the next idle period, and this is only the ceiling if
 * the main thread stays busy that long. Where requestIdleCallback is missing
 * (older WKWebView, tests) steps run on a macrotask with no delay at all.
 */
const BACKGROUND_HYDRATION_IDLE_TIMEOUT_MS = 3000;

/**
 * Fewest projects that are hydrated with the two all-projects reads. Fewer
 * are read one project at a time, two reads each: that is no more calls, and
 * it does not fetch every project's lists for the sake of one or two (the
 * selected project, a project a soft revalidation just discovered).
 */
const BULK_HYDRATION_MIN_PROJECTS = 3;

/**
 * The readOrder key under which the feed's lag recovery is recorded as it
 * arrives: a hydration sent before it may have read what the recovery says
 * is out of date, and one sent after it has not (see flushBranchChanges).
 */
const BRANCH_LAG_RECOVERY = 'branch-lag-recovery';

/**
 * What offering a fetched branch list resolves to when a local write landed
 * after the read was sent: neither applied nor refused, to be read again.
 */
const BRANCH_READ_SET_ASIDE = 'set-aside';

/**
 * Merge incoming branches with existing ones, preserving worktreePath when
 * a stale async response would overwrite an already-populated value with null.
 * Only for a branch that is local on both sides: a remote branch has no
 * worktree, and a branch whose type changed was moved by a transfer, which
 * deleted the worktree the old path points at.
 */
export function mergeBranchesPreservingWorktree(existing: Branch[], incoming: Branch[]): Branch[] {
  return incoming.map((newBranch) => {
    const prev = existing.find((b) => b.id === newBranch.id);
    if (
      prev?.worktreePath &&
      !newBranch.worktreePath &&
      prev.branchType === 'local' &&
      newBranch.branchType === 'local'
    ) {
      return { ...newBranch, worktreePath: prev.worktreePath };
    }
    return newBranch;
  });
}

/**
 * A fetched project list, carrying over from the list shown the object of
 * every project the fetch left unchanged. Returns `current` itself when
 * nothing changed at all: same projects, same order, same values.
 *
 * Every read of the backend builds new objects, so applying a list as
 * fetched hands every card a new `project` and re-renders all of them for a
 * change to one, or to none. With the objects kept, only what reads a
 * changed project re-runs, and an unchanged list wakes nothing. A project is
 * flat, which is what the shared comparison needs (see reuseUnchangedRows).
 */
export function reuseUnchangedProjects(current: Project[], fetched: Project[]): Project[] {
  return reuseUnchangedRows(current, fetched, projectId);
}

function projectId(project: Project): string {
  return project.id;
}

/** `map` without the entries of projects that are not listed: the map itself
 *  when there are none, so its readers are not woken for nothing. */
function withoutUnlisted<V>(map: Map<string, V>, listed: ReadonlySet<string>): Map<string, V> {
  let pruned: Map<string, V> | null = null;
  for (const projectId of map.keys()) {
    if (listed.has(projectId)) continue;
    pruned ??= new Map(map);
    pruned.delete(projectId);
  }
  return pruned ?? map;
}

/** Whether `map` holds exactly these projects, in this order. */
function hasProjectsInOrder(map: Map<string, unknown>, projects: Project[]): boolean {
  if (map.size !== projects.length) return false;
  let index = 0;
  for (const projectId of map.keys()) {
    if (projectId !== projects[index++].id) return false;
  }
  return true;
}

/** Whether two branch lists hold the same branch objects in the same order. */
function sameBranchList(existing: Branch[] | undefined, next: Branch[]): boolean {
  const current = existing ?? [];
  if (current === next) return true;
  return current.length === next.length && current.every((branch, i) => branch === next[i]);
}

/**
 * Where a project transfer is: its latest `project-transfer-progress`, or the
 * confirm that started it (no transfer id yet) until the first event lands.
 */
export interface ProjectTransferState {
  transferId: string | null;
  direction: ProjectTransferDirection;
  phase: ProjectTransferPhase;
  branchId: string | null;
  projectRepoId: string | null;
  detail: string | null;
  bytes: number | null;
}

/** Run a callback during idle time, falling back to a macrotask where
 *  requestIdleCallback is unavailable (Safari WKWebView, tests). */
function scheduleDeferredTask(callback: () => void, timeout = 1500): () => void {
  const schedule =
    typeof requestIdleCallback === 'function'
      ? (cb: () => void) => requestIdleCallback(cb, { timeout })
      : (cb: () => void) => setTimeout(cb, 0) as unknown as number;
  const cancel =
    typeof cancelIdleCallback === 'function'
      ? (handle: number) => cancelIdleCallback(handle)
      : (handle: number) => clearTimeout(handle);

  const handle = schedule(callback);
  return () => cancel(handle);
}

/** Run a callback on the next animation frame, falling back to a macrotask
 *  where requestAnimationFrame is unavailable (tests). */
function scheduleFrame(callback: () => void): () => void {
  if (typeof requestAnimationFrame === 'function') {
    const handle = requestAnimationFrame(callback);
    return () => cancelAnimationFrame(handle);
  }
  const handle = setTimeout(callback, 0);
  return () => clearTimeout(handle);
}

class ProjectsDataStore {
  private _projects = $state<Project[]>([]);
  private _branchesByProject = $state<Map<string, Branch[]>>(new Map());
  private _reposByProject = $state<Map<string, ProjectRepo[]>>(new Map());
  private _loading = $state(false);
  private _error = $state<string | null>(null);
  private _loaded = $state(false);
  private _deletingProjectNames = $state<Map<string, string>>(new Map());
  /**
   * Projects being transferred, mapped to the direction. Replaced only when a
   * transfer starts or ends, so every row's status doesn't recompute on each
   * progress event; the progress itself lives in _transferProgress.
   */
  private _transferringProjects = $state<Map<string, ProjectTransferDirection>>(new Map());
  private _transferProgress = $state<Map<string, ProjectTransferState>>(new Map());
  /** Transfers known to be over, so a progress event that arrives after the
   *  result (web mode: the socket and the HTTP response race) is dropped. */
  private finishedTransferIds = new Set<string>();

  /**
   * Projects whose branches + repos have been fetched at least once, mapped to
   * the load generation that fetched them. Membership is what view gates read
   * (isProjectHydrated); the generation lets the idle drip skip a project some
   * foreground caller already fetched under the current load.
   *
   * Entries are written once a fetch *settles* — success or failure, or a
   * failed fetch would gate a view forever — and survive generation bumps:
   * they mean "we have data to paint", so a refresh never re-blanks a painted
   * view. Soft revalidations preserve the generation and the in-flight set;
   * freshness for already-painted projects comes from change-feed refetches,
   * project selection, and hard refreshes rather than redoing the whole drip.
   */
  private _hydratedProjects = $state<Map<string, number>>(new Map());

  // null = never loaded; distinguishes "no repos" from "not fetched yet".
  private _homeRepos = $state<RepoHomeItem[] | null>(null);
  private _homeReposLoading = $state(false);

  /**
   * Guards every async apply: bumped by each full load so responses that
   * resolve after a newer load started are discarded instead of clobbering
   * fresher state. Same role as ProjectHome's loadGeneration.
   */
  private loadGeneration = 0;
  /**
   * Orders the reads within one generation by when they were sent. The
   * refresh, hydrate and branch-refetch paths can each have a read of the
   * same project in flight, and the backend serves them concurrently, so an
   * older response can land last. Keyed per list: `projects`,
   * `branches:<project>` and `repos:<project>`. `branch-writes:<project>`
   * records the view's own writes to a branch list (setBranchesByProject),
   * which a branch read sent before them must not be applied over, and
   * `project-writes` does the same for the project list (projectRenamed).
   */
  private readOrder = new LatestWins();
  private initialLoad: Promise<void> | null = null;
  private revalidatePending = false;
  private revalidateQueued = false;
  private revalidateQueuedForce = false;
  private revalidateQueuedHard = false;
  /** The projects that the events behind the queued reload named. */
  private revalidateQueuedProjectIds = new Set<string>();
  /**
   * A reload, or a bulk hydration, has failed since a hard reload last
   * landed. A failed reload may have cancelled hydrations it never
   * restarted, or dropped the refetch of the project its event named. A
   * failed bulk hydration leaves every project it covered without the lists
   * it was to bring (see hydrationFailed). A soft reload, which refetches the
   * one project its event names, repairs none of that. So the next
   * project-changed reloads hard (see handleProjectChanged).
   */
  private reloadFailed = false;
  private backgroundHydrationCancel: (() => void) | null = null;
  /** In-flight hydrations by project, so the foreground fetch, the idle queue
   *  and the grid's sweep share one request instead of racing three. A bulk
   *  hydration is entered under every project it covers. */
  private hydrationInFlight = new Map<string, Promise<void>>();
  /** When each hydration in flight was sent: its place in readOrder. */
  private hydrationSentAt = new WeakMap<Promise<void>, number>();
  /**
   * Projects whose last hydration failed. They are marked hydrated all the
   * same, or a view gated on hydration would hang, but what they show is the
   * seeded empty lists, or lists from before the read that failed. So the
   * callers that ask for a project to be fetched (ensureProjectHydrated,
   * ensureProjectsHydrated) read these again, where they pass over a project
   * whose lists did land. An entry goes when a read of the project's lists
   * lands, or the project leaves the list.
   */
  private hydrationFailed = new Set<string>();
  /** refreshProject reads in flight, by project. Each marks its project
   *  hydrated when it lands, so an event that finds the project not fetched
   *  yet waits for it as it does for a first hydration (see whenHydrated). */
  private refreshInFlight = new Map<string, Promise<unknown>>();

  private homeReposInFlight: Promise<void> | null = null;
  private homeReposFetchToken = 0;

  private listening = false;
  private unlisteners: UnlistenFn[] = [];
  private pendingPrStatusEvents: PrStatusChangedEvent[] = [];
  private prStatusFlushCancel: (() => void) | null = null;
  private pendingBranchChanges: BranchChangedEvent[] = [];
  private branchChangedFlushCancel: (() => void) | null = null;
  private branchRefetchInFlight = new Set<string>();
  private branchRefetchQueued = new Set<string>();
  private repoCountsMemo: {
    projects: Project[];
    reposByProject: Map<string, ProjectRepo[]>;
    counts: Map<string, number>;
  } | null = null;

  // ── Reactive reads ──

  /** Every listed project, including any with a delete in flight. Rendered
   *  lists, counts and pickers read liveProjects instead; this is for guards,
   *  lookups by id and list positions that must still see a doomed project. */
  get projects(): Project[] {
    return this._projects;
  }

  /**
   * The project list minus projects with a delete in flight: what the home
   * grid, the sidebar, the filter chips and the project pickers show. A
   * confirmed delete hides the project here at once rather than when the
   * backend's worktree teardown finishes, and projectDeleteFailed brings it
   * back by clearing the marker. See the project-delete lifecycle below.
   *
   * Derived rather than a getter: half a dozen lists and counts read it per
   * pass, so it filters once per change and hands them all the same array,
   * which keeps their own $deriveds from seeing a new list on every read.
   * With nothing deleting it is projects itself.
   */
  readonly liveProjects: Project[] = $derived.by(() => {
    const deleting = this._deletingProjectNames;
    if (deleting.size === 0) return this._projects;
    return this._projects.filter((project) => !deleting.has(project.id));
  });

  get branchesByProject(): Map<string, Branch[]> {
    return this._branchesByProject;
  }

  get reposByProject(): Map<string, ProjectRepo[]> {
    return this._reposByProject;
  }

  /**
   * Repo count per project. Falls back to 1 for un-hydrated single-repo
   * projects (githubRepo set) so counts render sensibly before repos load.
   *
   * Memoised on the identity of the two maps it reads: every sidebar row
   * derives from this, and each per-project hydration reassigns
   * `_reposByProject`, so rebuilding on every read was O(projects²) per
   * startup. Both inputs are only ever reassigned (never mutated in place),
   * so identity is a complete change signal.
   */
  get repoCountsByProject(): Map<string, number> {
    const projects = this._projects;
    const reposByProject = this._reposByProject;
    const memo = this.repoCountsMemo;
    if (memo && memo.projects === projects && memo.reposByProject === reposByProject) {
      return memo.counts;
    }
    const counts = new Map(
      projects.map((project) => {
        const repos = reposByProject.get(project.id);
        return [project.id, repos ? repos.length : project.githubRepo ? 1 : 0] as const;
      })
    );
    this.repoCountsMemo = { projects, reposByProject, counts };
    return counts;
  }

  get loading(): boolean {
    return this._loading;
  }

  get error(): string | null {
    return this._error;
  }

  /** True once the project list has landed. Branches and repos may still be
   *  hydrating — gate on isProjectHydrated()/allProjectsHydrated for those. */
  get loaded(): boolean {
    return this._loaded;
  }

  /** Whether this project's branches and repos have been fetched. */
  isProjectHydrated(projectId: string): boolean {
    return this._hydratedProjects.has(projectId);
  }

  /** True when every known project has been hydrated at least once. */
  get allProjectsHydrated(): boolean {
    return this._projects.every((p) => this._hydratedProjects.has(p.id));
  }

  get deletingProjectNames(): Map<string, string> {
    return this._deletingProjectNames;
  }

  isProjectDeleting(projectId: string): boolean {
    return this._deletingProjectNames.has(projectId);
  }

  get transferringProjects(): ReadonlyMap<string, ProjectTransferDirection> {
    return this._transferringProjects;
  }

  isProjectTransferring(projectId: string): boolean {
    return this._transferringProjects.has(projectId);
  }

  getProjectTransfer(projectId: string): ProjectTransferState | null {
    return this._transferProgress.get(projectId) ?? null;
  }

  get homeRepos(): RepoHomeItem[] {
    return this._homeRepos ?? [];
  }

  get homeReposLoaded(): boolean {
    return this._homeRepos !== null;
  }

  get homeReposLoading(): boolean {
    return this._homeReposLoading;
  }

  // ── Loading ──

  /**
   * Make sure the store holds the project list. The first call fetches it and
   * resolves as soon as it lands — per-project branches and repos are *not*
   * on this critical path, so a cold start costs one round trip rather than
   * one per project. Later calls resolve immediately — the in-memory data is
   * already renderable — and kick a background revalidation.
   *
   * Either way per-project hydration drips through the idle queue behind
   * this; callers that need it sooner use hydrateProject() (one project,
   * always refetched), ensureProjectHydrated() (one project, only if not
   * fetched yet) or ensureProjectsHydrated() (all of them).
   *
   * Load failures don't reject; they surface through `error` so callers can
   * render them, mirroring the views' loadData() pattern.
   */
  async ensureLoaded(): Promise<void> {
    if (this._loaded) {
      void this.revalidate({ soft: true });
      return;
    }
    this.initialLoad ??= this.loadProjectsAndHydrate().finally(() => {
      this.initialLoad = null;
    });
    return this.initialLoad;
  }

  /** Wait for the first project list without forcing an already-loaded store to
   *  revalidate. Used by startup validation paths that must not throw away work
   *  a view has already kicked off. */
  whenLoaded(): Promise<void> {
    return this._loaded ? Promise.resolve() : this.ensureLoaded();
  }

  /** Full reload (used by cache-stale and the project-delete flow). Always
   *  starts a new load — the generation bump discards in-flight applies —
   *  and refetches whatever was already hydrated so no painted view goes
   *  stale. */
  async refresh(): Promise<void> {
    await this.loadProjectsAndHydrate();
    await this.hydrateProjects(this.projectsToRefresh(), this.loadGeneration);
    if (this._homeRepos !== null) {
      void this.startHomeReposFetch();
    }
  }

  /**
   * The projects a refresh reads again at once: the hydrated ones, which are
   * what is painted. When they go out as the two all-projects reads, those
   * replies carry every project's lists whichever projects asked, so the
   * rest of the list is covered by the same hydration. The idle step the
   * reload queued then finds them all fetched, where it would have sent the
   * same two reads again for the projects left out.
   */
  private projectsToRefresh(): string[] {
    const hydrated = [...this._hydratedProjects.keys()];
    const bulk = hydrated.length > 0 && this._projects.length >= BULK_HYDRATION_MIN_PROJECTS;
    return bulk ? this._projects.map((project) => project.id) : hydrated;
  }

  /**
   * Hydrate branches + repos for one project. Foreground (default) fetches
   * immediately — use for the selected project. Background defers to the
   * idle queue so it doesn't compete with a view transition. Deduped against
   * any hydration already in flight for the project.
   */
  async hydrateProject(
    projectId: string,
    options: { priority?: 'foreground' | 'background' } = {}
  ): Promise<void> {
    const generation = this.loadGeneration;
    if (options.priority === 'background') {
      scheduleDeferredTask(() => {
        // Deleted while it waited its turn: reading it would only fail.
        if (!this.hasProject(projectId)) return;
        void this.hydrateOnce(projectId, generation);
      }, BACKGROUND_HYDRATION_IDLE_TIMEOUT_MS);
      return;
    }
    await this.hydrateOnce(projectId, generation);
  }

  /**
   * Hydrate one project only if it hasn't been fetched under the current load —
   * the single-project counterpart of ensureProjectsHydrated().
   *
   * Unlike hydrateProject(), which always refetches (hydrateOnce dedupes
   * against in-flight requests, not settled ones), this no-ops once the project
   * is hydrated and otherwise joins whatever drip/sweep/foreground fetch is
   * already running. Entry point for consumers that must not act on the seeded
   * fallbacks — notably the project-delete safety check. A project whose last
   * hydration failed still shows those fallbacks, so it is read again.
   */
  async ensureProjectHydrated(projectId: string): Promise<void> {
    if (this.hasFetchedLists(projectId)) return;
    await this.hydrateOnce(projectId, this.loadGeneration);
  }

  /** Hydrate every project that hasn't been fetched yet: together when there
   *  are enough of them (see hydrateProjects). Entry point for the projects
   *  grid and the board, which paint complete or not at all. A project whose
   *  last hydration failed counts as not fetched (see hydrationFailed), so
   *  the views that ask on mount and on a list change are also the retry. */
  async ensureProjectsHydrated(): Promise<void> {
    const generation = this.loadGeneration;
    const pending = this._projects.filter((p) => !this.hasFetchedLists(p.id));
    if (pending.length === 0) return;
    await this.hydrateProjects(
      pending.map((project) => project.id),
      generation
    );
  }

  /**
   * Refetch the project list plus one project's branches and repos. Used
   * after mutations that reshape a single project (repo added or removed)
   * and by the project-setup-progress listener. Invalidates the project's
   * branch timelines so downstream views refetch them.
   *
   * Deliberately un-deduped: it is the post-mutation refetch, so it must not
   * be absorbed into a hydration that started before the mutation.
   *
   * `replaceBranches` applies the branch list as fetched, without carrying
   * worktree paths over from the old one. Resolves whether the result was
   * applied: false when a newer load overtook it, or the fetch failed.
   *
   * Callers await this to know the store shows what their mutation left
   * (the branch delete clears its "Deleting…" state on it). So when the
   * branch list is set aside because a local write landed while it was out
   * (see offerProjectBranches), this reads the list again itself and resolves
   * only once a list read after that write has been offered. Any local write
   * to the project sets the read aside, related to the refresh or not, such
   * as a workspace status change on another branch.
   */
  refreshProject(projectId: string, options: { replaceBranches?: boolean } = {}): Promise<boolean> {
    const refresh = this.readProjectAfterWrite(projectId, options);
    // Every read still out for the project, so a second refresh that settles
    // first does not hide the one before it.
    const earlier = this.refreshInFlight.get(projectId);
    const settled: Promise<unknown> = earlier ? Promise.all([earlier, refresh]) : refresh;
    this.refreshInFlight.set(projectId, settled);
    void settled.then(() => {
      if (this.refreshInFlight.get(projectId) === settled) this.refreshInFlight.delete(projectId);
    });
    return refresh;
  }

  /** The reads behind refreshProject. Never rejects: a failure is logged and
   *  resolves false. */
  private async readProjectAfterWrite(
    projectId: string,
    { replaceBranches = false }: { replaceBranches?: boolean } = {}
  ): Promise<boolean> {
    const generation = this.loadGeneration;
    const ticket = this.readOrder.issue();
    try {
      const [projectsResult, branchesResult, reposResult] = await Promise.all([
        commands.listProjects({ force: true }),
        commands.listBranchesForProject(projectId, { force: true }),
        commands.listProjectRepos(projectId, { force: true }),
      ]);
      if (generation !== this.loadGeneration) return false;
      if (this.admitProjectList(ticket)) {
        this.showProjectList(projectsResult.data);
      }
      let offered = this.offerProjectBranches(projectId, branchesResult.data, generation, ticket, {
        replace: replaceBranches,
      });
      this.applyProjectRepos(projectId, reposResult.data, generation, ticket);
      this.markProjectHydrated(projectId, generation);
      this.recordHydrationOutcome([projectId], generation, 'read');
      // Each further round needs another local write to land during a read,
      // so this ends as soon as one read comes back with none.
      while (offered === BRANCH_READ_SET_ASIDE) {
        const again = this.readOrder.issue();
        const { data } = await commands.listBranchesForProject(projectId, { force: true });
        if (generation !== this.loadGeneration) return false;
        offered = this.offerProjectBranches(projectId, data, generation, again);
      }
      if (offered) {
        commands.invalidateProjectBranchTimelines(offered.map((b) => b.id));
      }
      return true;
    } catch (e) {
      console.error(`[projectsData] Failed to refresh project '${projectId}':`, e);
      return false;
    }
  }

  /**
   * Refetch a project a transfer of it just ended: its location and its
   * branches, applied together so nothing pairs the new location with the old
   * branch types. The branch list replaces the old one outright, since the
   * transfer removed the worktrees the old paths point at. A reload that
   * overtakes the fetch is retried, so this settles on lists read after the
   * transfer whenever it can.
   */
  async refreshTransferredProject(projectId: string): Promise<void> {
    for (let attempt = 0; attempt < 3; attempt++) {
      if (!this.hasProject(projectId)) return;
      if (await this.refreshProject(projectId, { replaceBranches: true })) return;
    }
  }

  /**
   * Register a newly created project immediately — closing the creation
   * modal must not wait for a reload — then hydrate its branches and repos
   * in the background.
   *
   * Seeds it at the head: the list is newest first, so this is where the
   * change-feed refetch will land it and nothing jumps once that arrives.
   */
  projectCreated(project: Project): void {
    if (!this._projects.some((p) => p.id === project.id)) {
      this._projects = [project, ...this._projects];
    }
    if (!this._branchesByProject.has(project.id)) {
      this._branchesByProject = new Map(this._branchesByProject).set(project.id, []);
    }
    // Count it hydrated right away — the seeded empty branch list is accurate
    // at creation time, and selecting the project as the modal closes must not
    // land on a view that treats it as un-hydrated and blanks for a beat.
    this.markProjectHydrated(project.id, this.loadGeneration);
    void this.hydrateProject(project.id);
  }

  /**
   * Paint a project's new name at once — the detail-page title, sidebar row
   * and sticky top-bar echo all read this list — ahead of the project-changed
   * refetch that makes it authoritative. (The window title is derived from
   * the sidebar filters, not the project name, so it is unaffected.)
   */
  projectRenamed(projectId: string, name: string): void {
    // A read of the project list still in flight was sent before this write
    // and may have read the store before the rename. Mark the write so that
    // read is not applied over it (see admitProjectList). Only a name that
    // really changes counts, as in setBranchesByProject.
    if (this._projects.some((p) => p.id === projectId && p.name !== name)) {
      this.readOrder.wrote('project-writes');
    }
    this._projects = this._projects.map((p) => (p.id === projectId ? { ...p, name } : p));
  }

  /**
   * Whether a fetched project list may replace the one shown.
   *
   * Refused when a list sent after it has already landed. Set aside when the
   * view's own rename (projectRenamed) landed while it was out: it may have
   * read the store before the rename, and would put the old name back until
   * the next list arrived. A list that is set aside is read again in the
   * background, since it may be the only one carrying some other change.
   *
   * The checks run in that order and the list is recorded only when it is
   * applied, as in offerProjectBranches, so a list that was set aside is
   * never taken for one that landed.
   */
  private admitProjectList(ticket: number): boolean {
    if (this.readOrder.overtaken('projects', ticket)) return false;
    if (this.readOrder.overtaken('project-writes', ticket)) {
      void this.revalidate({ force: true, soft: true });
      return false;
    }
    return this.readOrder.admit('projects', ticket);
  }

  /**
   * Replace the branch map wholesale. Entry point for synchronous
   * view-driven updates (workspaceLifecycle status/worktree writes, branch
   * rename/delete) so every consumer sees them; fetch-driven applies go
   * through the generation-guarded paths instead.
   */
  setBranchesByProject(next: Map<string, Branch[]>): void {
    // A read of a project's branches still in flight was sent before this
    // write and may have read the store before it. Mark the write so that
    // read is not applied over it (see offerProjectBranches). Only lists that
    // really differ count: callers rebuild the array even when no branch in
    // it matched, and such a call must not turn a first hydration away.
    for (const [projectId, branches] of next) {
      if (!sameBranchList(this._branchesByProject.get(projectId), branches)) {
        this.readOrder.wrote(`branch-writes:${projectId}`);
      }
    }
    this._branchesByProject = next;
  }

  /**
   * Reload the project list. `soft` keeps the load generation, and with it
   * every hydration in flight. `changedProjectIds` are the projects a
   * project-changed event named: a soft reload refetches their branches and
   * repos once its list has landed (see refetchChangedProjects).
   */
  private async revalidate({
    force = false,
    soft = false,
    changedProjectIds = [],
  }: {
    force?: boolean;
    soft?: boolean;
    changedProjectIds?: readonly string[];
  } = {}): Promise<void> {
    if (this.revalidatePending) {
      // A change arrived while a reload was in flight; that reload may have
      // read the list before the write committed, so run once more after it.
      // Preserve force and hardness across the queue: if any queued event says
      // the backend changed, the follow-up fetch must not accept a cached
      // answer, and if any queued caller asked for a hard reload the follow-up
      // must bump the generation. Overlapping soft callers — two views
      // mounting in one navigation — stay soft, or the follow-up would discard
      // the in-flight hydrations the soft path exists to keep. The projects
      // the queued events named go to the follow-up too: the reload in flight
      // was sent before them, so only the follow-up's list can say whether
      // each is still there to refetch.
      this.revalidateQueued = true;
      this.revalidateQueuedForce ||= force;
      this.revalidateQueuedHard ||= !soft;
      for (const projectId of changedProjectIds) this.revalidateQueuedProjectIds.add(projectId);
      return;
    }
    this.revalidatePending = true;
    try {
      await this.loadProjectsAndHydrate({ force, soft, changedProjectIds });
    } finally {
      this.revalidatePending = false;
      if (this.revalidateQueued) {
        const queuedForce = this.revalidateQueuedForce;
        const queuedProjectIds = [...this.revalidateQueuedProjectIds];
        // An event queued behind a reload that failed reloads hard, as one
        // arriving after it would (see handleProjectChanged).
        const queuedHard =
          this.revalidateQueuedHard || (this.reloadFailed && queuedProjectIds.length > 0);
        this.revalidateQueued = false;
        this.revalidateQueuedForce = false;
        this.revalidateQueuedHard = false;
        this.revalidateQueuedProjectIds.clear();
        void this.revalidate({
          force: queuedForce,
          soft: !queuedHard,
          changedProjectIds: queuedProjectIds,
        });
      }
    }
  }

  private async loadProjectsAndHydrate({
    force = false,
    soft = false,
    changedProjectIds = [],
  }: {
    force?: boolean;
    soft?: boolean;
    changedProjectIds?: readonly string[];
  } = {}): Promise<void> {
    const generation = soft ? this.loadGeneration : ++this.loadGeneration;
    if (!soft) {
      this.cancelBackgroundHydration();
      // Those promises are already no-ops under the new generation; drop them so
      // callers after the bump start fresh fetches.
      this.hydrationInFlight.clear();
    }
    if (this._projects.length === 0) {
      this._loading = true;
    }
    this._error = null;
    const ticket = this.readOrder.issue();
    try {
      const [{ data, revalidating }] = await Promise.all([
        commands.listProjects({ force }),
        repoBadgeStore.loadAll({ force }),
      ]);
      if (generation !== this.loadGeneration) return;
      this.applyProjectList(data, generation, ticket, { scheduleHydration: !soft });
      this._loaded = true;
      // A hard load hydrates every project again, the changed ones included.
      if (soft) this.refetchChangedProjects(changedProjectIds);
      else this.reloadFailed = false;

      if (revalidating) {
        // Applied outside the awaited chain so callers aren't blocked on the
        // SWR refresh — they already have renderable data.
        revalidating
          .then((fresh) =>
            this.applyProjectList(fresh, generation, ticket, { scheduleHydration: !soft })
          )
          .catch((e) => {
            console.error('[projectsData] Failed to revalidate project list:', e);
          });
      }
    } catch (e) {
      if (generation !== this.loadGeneration) return;
      this._error = e instanceof Error ? e.message : String(e);
      this.reloadFailed = true;
    } finally {
      if (generation === this.loadGeneration) {
        this._loading = false;
      }
    }
  }

  /**
   * Show a fetched project list, keeping the object of every project it
   * leaves unchanged. A list that changes nothing is not assigned, so nothing
   * reading the list is woken (see reuseUnchangedProjects).
   */
  private showProjectList(fetched: Project[]): void {
    const next = reuseUnchangedProjects(this._projects, fetched);
    if (next !== this._projects) this._projects = next;
  }

  /**
   * Apply a fetched project list: seed branch entries so per-project
   * consumers can render immediately and prune state for removed projects.
   * Synchronous by design — this is what `loaded` waits for. Per-project
   * branches and repos are left to the background hydration kicked here on
   * hard loads, or to whichever view asks for them sooner. Soft revalidations
   * reuse the current one rather than cancelling and restarting it.
   *
   * Each piece of state is assigned only when the list changes it. Every one
   * is read by every card, so a list that adds and removes no project leaves
   * the maps alone, and one that changes nothing at all wakes no reader.
   */
  private applyProjectList(
    projectList: Project[],
    generation: number,
    ticket: number,
    { scheduleHydration = true }: { scheduleHydration?: boolean } = {}
  ): void {
    if (generation !== this.loadGeneration) return;
    // A list sent after this one has already landed, or the view's own
    // rename has (see admitProjectList): keep the list shown, and seed, prune
    // and hydrate against it.
    if (!this.admitProjectList(ticket)) projectList = this._projects;
    // The projects no list has seeded yet, so nothing has arranged to hydrate
    // them. Asked of the branch entries and not of the list shown:
    // refreshProject shows a list without seeding it, so a project it brought
    // in is listed already and would otherwise never be fetched.
    const unseededProjectIds = projectList
      .map((project) => project.id)
      .filter((projectId) => !this._branchesByProject.has(projectId));
    this.showProjectList(projectList);

    // One entry per listed project, in list order. Rebuilt unless the map is
    // exactly that already, so skipping it changes nothing a reader can see.
    if (!hasProjectsInOrder(this._branchesByProject, projectList)) {
      const branchMap = new Map<string, Branch[]>();
      for (const project of projectList) {
        branchMap.set(project.id, this._branchesByProject.get(project.id) || []);
      }
      this._branchesByProject = branchMap;
    }

    const projectIds = new Set(projectList.map((p) => p.id));
    const prunedRepos = withoutUnlisted(this._reposByProject, projectIds);
    if (prunedRepos !== this._reposByProject) this._reposByProject = prunedRepos;

    const prunedHydrated = withoutUnlisted(this._hydratedProjects, projectIds);
    if (prunedHydrated !== this._hydratedProjects) this._hydratedProjects = prunedHydrated;

    // A project that vanished from the fetched list finished deleting: drop
    // its in-progress marker in the same apply, so liveProjects, which has
    // hidden it since the delete started, never sees it again in between.
    const prunedDeleting = withoutUnlisted(this._deletingProjectNames, projectIds);
    if (prunedDeleting !== this._deletingProjectNames) this._deletingProjectNames = prunedDeleting;

    // Housekeeping: nothing asks whether an unlisted project was fetched, so
    // this only keeps the set from growing with deleted projects.
    for (const projectId of this.hydrationFailed) {
      if (!projectIds.has(projectId)) this.hydrationFailed.delete(projectId);
    }

    if (scheduleHydration) {
      this.scheduleBackgroundHydration(
        projectList.map((p) => p.id),
        generation
      );
    } else if (unseededProjectIds.length > 0) {
      // Soft revalidations do not restart the whole drip, but a project the
      // store has not seeded has no existing branch/repo fetch to join. Put
      // just those new cards onto the background hydration queue, in one
      // step: a reload after time away, or one that folds a batch of creates
      // together, can bring in many, and they then go out as one bulk
      // hydration (see hydrateProjects) and not as a read and a pass over
      // the cards per project.
      scheduleDeferredTask(() => {
        // A no-op once a newer load has started (see hydrateProjects).
        void this.hydrateProjects(
          // Deleted while it waited its turn: reading it would only fail.
          unseededProjectIds.filter((projectId) => this.hasProject(projectId)),
          generation
        );
      }, BACKGROUND_HYDRATION_IDLE_TIMEOUT_MS);
    }
  }

  /**
   * Hydrate these projects, resolving once every one has settled. A project
   * with a hydration already in flight joins it. The rest go out together as
   * one bulk hydration when there are BULK_HYDRATION_MIN_PROJECTS of them,
   * and one project at a time otherwise. Never rejects (see hydrateOnce).
   */
  private async hydrateProjects(projectIds: string[], generation: number): Promise<void> {
    if (generation !== this.loadGeneration) return;
    const hydrations = new Set<Promise<void>>();
    const unsent: string[] = [];
    for (const projectId of projectIds) {
      const inFlight = this.hydrationInFlight.get(projectId);
      if (inFlight) hydrations.add(inFlight);
      else unsent.push(projectId);
    }
    if (unsent.length >= BULK_HYDRATION_MIN_PROJECTS) {
      hydrations.add(this.hydrateInBulk(unsent, generation));
    } else {
      for (const projectId of unsent) hydrations.add(this.hydrateOnce(projectId, generation));
    }
    await Promise.all(hydrations);
  }

  /**
   * Hydrate many projects with two reads, every project's branches and every
   * project's repos, and apply them once: one assignment to each of the
   * branch, repo and hydrated maps, where hydrating them one by one made
   * three per project, each waking every reader of the map.
   *
   * Entered in hydrationInFlight under every project it covers, so a caller
   * that wants one of them meanwhile joins this instead of reading it again,
   * and a change event for one of them is replayed once this settles (see
   * refetchBranchesIfHydrated). `force` reads past the web cache, for a
   * caller that knows the lists it holds are out of date.
   *
   * Never rejects, like hydrateOnce. A read that fails is tried once more.
   * If that fails too it is logged and the projects are still marked
   * settled, but not left that way for good: see hydrateInBulkInternal.
   */
  private hydrateInBulk(
    projectIds: string[],
    generation: number,
    { force = false }: { force?: boolean } = {}
  ): Promise<void> {
    const covered: ReadonlySet<string> = new Set(projectIds);
    const ticket = this.readOrder.issue();
    const hydration = this.hydrateInBulkInternal(covered, generation, ticket, force).catch((e) => {
      console.error(`[projectsData] Failed to hydrate ${covered.size} projects in bulk:`, e);
    });
    this.hydrationSentAt.set(hydration, ticket);
    for (const projectId of covered) this.hydrationInFlight.set(projectId, hydration);
    void hydration.finally(() => {
      // Identity check: a newer load may have cleared or replaced the entries.
      for (const projectId of covered) {
        if (this.hydrationInFlight.get(projectId) === hydration) {
          this.hydrationInFlight.delete(projectId);
        }
      }
    });
    return hydration;
  }

  /**
   * The reads and applies of a bulk hydration. `sentAt` is the ticket of the
   * first read (see readOrder).
   *
   * One reply carries every project, so one failed read would leave every
   * covered project settled on what it had: for a first load, the seeded
   * empty lists. The backend leaves a project it cannot read out of the
   * reply, which is the `unlisted` case below, so what fails here is the
   * whole read: the project list could not be read, or the call did not get
   * through. Three things keep that from sticking:
   *
   * - The two reads are sent once more, at once. A failure that was passing
   *   costs nothing more than that.
   * - If they fail again, the projects are recorded in hydrationFailed, so
   *   whoever next asks for them to be fetched reads them again: a view
   *   mounting, a list change, the project-delete check.
   * - reloadFailed is set, so the next project-changed reloads everything,
   *   as every project-changed did before events were targeted.
   *
   * It is not retried one project at a time: that is a read and a pass over
   * the cards per project, sent to a backend that has just failed twice.
   */
  private async hydrateInBulkInternal(
    covered: ReadonlySet<string>,
    generation: number,
    sentAt: number,
    force: boolean
  ): Promise<void> {
    const read = () =>
      Promise.all([
        // Plain reads unless forced: on the web they are served from the
        // cache first.
        force ? commands.listBranchesByProject({ force }) : commands.listBranchesByProject(),
        force
          ? commands.listProjectReposByProject({ force })
          : commands.listProjectReposByProject(),
      ]);
    // One ticket for the whole reply. It orders each project's lists against
    // the reads and local writes of that project alone, exactly as the ticket
    // of a one-project hydration does (see readOrder).
    let ticket = sentAt;
    // Covered projects missing from either reply; see below.
    let unlisted: string[] = [];
    let outcome: 'read' | 'failed' = 'read';
    try {
      let replies: Awaited<ReturnType<typeof read>>;
      try {
        replies = await read();
      } catch (e) {
        if (generation !== this.loadGeneration) return;
        console.warn(
          `[projectsData] Reading ${covered.size} projects in bulk failed, trying once more:`,
          e
        );
        // Sent later than the first read, so ordered later.
        ticket = this.readOrder.issue();
        replies = await read();
      }
      const [branchesResult, reposResult] = replies;
      if (generation !== this.loadGeneration) return;

      unlisted = this._projects
        .map((project) => project.id)
        .filter(
          (projectId) =>
            covered.has(projectId) &&
            (branchesResult.data[projectId] === undefined ||
              reposResult.data[projectId] === undefined)
        );
      this.applyBranchesByProject(branchesResult.data, covered, generation, ticket);
      this.applyReposByProject(reposResult.data, covered, generation, ticket);

      if (branchesResult.revalidating) {
        branchesResult.revalidating
          .then((fresh) => this.applyBranchesByProject(fresh, covered, generation, ticket))
          .catch((e) => {
            console.error('[projectsData] Failed to revalidate the branches of all projects:', e);
          });
      }
      if (reposResult.revalidating) {
        reposResult.revalidating
          .then((fresh) => this.applyReposByProject(fresh, covered, generation, ticket))
          .catch((e) => {
            console.error('[projectsData] Failed to revalidate the repos of all projects:', e);
          });
      }
    } catch (e) {
      outcome = 'failed';
      // Only for the load this read belongs to: a newer one hydrates every
      // project again itself.
      if (generation === this.loadGeneration) this.reloadFailed = true;
      throw e;
    } finally {
      // In the same step as the applies above, so the three maps change
      // together and readers recompute once.
      const unlistedIds = new Set(unlisted);
      const settled = [...covered].filter((projectId) => !unlistedIds.has(projectId));
      this.markProjectsHydrated(settled, generation);
      this.recordHydrationOutcome(settled, generation, outcome);
    }

    // The backend lists every project it holds and could read, so a project
    // missing from a reply was not in the store when it was read, its own
    // read failed there, or the reply is a cached copy that does not carry
    // it (web: from before the project existed, or with the project taken
    // out when its list changed). Either way that reply says nothing about
    // it: read the project on its own, which settles it as well, and reports
    // its error if it has one. Still under this hydration, so it stays in
    // flight until then.
    await Promise.all(
      unlisted.map((projectId) =>
        this.hydrateProjectInternal(projectId, generation).catch((e) => {
          console.error(`[projectsData] Failed to hydrate project '${projectId}':`, e);
        })
      )
    );
  }

  /** Hydrate a project unless the same hydration is already in flight, so the
   *  foreground fetch, the idle queue and the grid's sweep share one request.
   *  Never rejects: failures are logged and still mark the project settled, or
   *  a view gated on isProjectHydrated() would hang forever. */
  private hydrateOnce(projectId: string, generation: number): Promise<void> {
    if (generation !== this.loadGeneration) return Promise.resolve();
    const inFlight = this.hydrationInFlight.get(projectId);
    if (inFlight) return inFlight;

    const ticket = this.readOrder.issue();
    const hydration = this.hydrateProjectInternal(projectId, generation, ticket).catch((e) => {
      console.error(`[projectsData] Failed to hydrate project '${projectId}':`, e);
    });
    this.hydrationSentAt.set(hydration, ticket);
    this.hydrationInFlight.set(projectId, hydration);
    void hydration.finally(() => {
      // Identity check: a newer load may have cleared or replaced the entry.
      if (this.hydrationInFlight.get(projectId) === hydration) {
        this.hydrationInFlight.delete(projectId);
      }
    });
    return hydration;
  }

  private async hydrateProjectInternal(
    projectId: string,
    generation: number,
    ticket = this.readOrder.issue()
  ): Promise<void> {
    let outcome: 'read' | 'failed' = 'read';
    try {
      const [branchesResult, reposResult] = await Promise.all([
        commands.listBranchesForProject(projectId),
        commands.listProjectRepos(projectId),
      ]);
      if (generation !== this.loadGeneration) return;

      this.applyProjectBranches(projectId, branchesResult.data, generation, ticket);
      this.applyProjectRepos(projectId, reposResult.data, generation, ticket);

      if (branchesResult.revalidating) {
        branchesResult.revalidating
          .then((fresh) => {
            this.applyProjectBranches(projectId, fresh, generation, ticket);
          })
          .catch((e) => {
            console.error(`[projectsData] Failed to revalidate branches for '${projectId}':`, e);
          });
      }

      if (reposResult.revalidating) {
        reposResult.revalidating
          .then((fresh) => this.applyProjectRepos(projectId, fresh, generation, ticket))
          .catch((e) => {
            console.error(`[projectsData] Failed to revalidate repos for '${projectId}':`, e);
          });
      }
    } catch (e) {
      outcome = 'failed';
      throw e;
    } finally {
      this.markProjectHydrated(projectId, generation);
      this.recordHydrationOutcome([projectId], generation, outcome);
    }
  }

  private hasProject(projectId: string): boolean {
    return this._projects.some((project) => project.id === projectId);
  }

  /** Whether the store holds lists that were read for this project: it is
   *  hydrated, and not by a hydration that failed (see hydrationFailed). */
  private hasFetchedLists(projectId: string): boolean {
    return this._hydratedProjects.has(projectId) && !this.hydrationFailed.has(projectId);
  }

  /**
   * Mark a project settled under a load. Not assigned when it is so marked
   * already: the map is read by every view gated on hydration, and a read
   * that fails again must not wake them to ask again (see hydrationFailed).
   */
  private markProjectHydrated(projectId: string, generation: number): void {
    if (generation !== this.loadGeneration || !this.hasProject(projectId)) return;
    if (this._hydratedProjects.get(projectId) === generation) return;
    this._hydratedProjects = new Map(this._hydratedProjects).set(projectId, generation);
  }

  /** markProjectHydrated for many projects, with one assignment. */
  private markProjectsHydrated(projectIds: string[], generation: number): void {
    if (generation !== this.loadGeneration) return;
    const known = new Set(this._projects.map((project) => project.id));
    let next: Map<string, number> | null = null;
    for (const projectId of projectIds) {
      if (!known.has(projectId)) continue;
      if (this._hydratedProjects.get(projectId) === generation) continue;
      next ??= new Map(this._hydratedProjects);
      next.set(projectId, generation);
    }
    if (next) this._hydratedProjects = next;
  }

  /**
   * Record how a hydration of these projects ended: that their lists were
   * read, or that the read failed and they are settled without them (see
   * hydrationFailed). Nothing is recorded for a load that has been replaced,
   * which reads every project again itself.
   */
  private recordHydrationOutcome(
    projectIds: Iterable<string>,
    generation: number,
    outcome: 'read' | 'failed'
  ): void {
    if (generation !== this.loadGeneration) return;
    for (const projectId of projectIds) {
      if (outcome === 'read') this.hydrationFailed.delete(projectId);
      else this.hydrationFailed.add(projectId);
    }
  }

  /**
   * Offer a fetched branch list to the store. Resolves to the list applied,
   * or to null when it was refused: a newer load has started, or a read sent
   * after this one (`ticket`, see readOrder) has already been applied. A
   * `replace` list is applied even then: it is read after the transfer that
   * asked for it, and it alone drops the worktree paths the transfer removed.
   *
   * A list sent before a local write landed (setBranchesByProject) is not
   * applied either: it may predate the rename, delete or status change the
   * view has already painted, and would put the old row back until the
   * write's change event refetched. That is BRANCH_READ_SET_ASIDE, not a
   * refusal, since the list may be the only read carrying some other change:
   * the caller must have the list read again.
   */
  private offerProjectBranches(
    projectId: string,
    branches: Branch[],
    generation: number,
    ticket: number,
    { replace = false }: { replace?: boolean } = {}
  ): Branch[] | typeof BRANCH_READ_SET_ASIDE | null {
    if (generation !== this.loadGeneration || !this.hasProject(projectId)) return null;
    const admitted = this.admitBranchRead(projectId, ticket, replace);
    if (admitted !== true) return admitted ? BRANCH_READ_SET_ASIDE : null;

    const mergedBranches = replace
      ? branches
      : mergeBranchesPreservingWorktree(this._branchesByProject.get(projectId) || [], branches);
    this._branchesByProject = new Map(this._branchesByProject).set(projectId, mergedBranches);
    return mergedBranches;
  }

  /**
   * Whether a branch list read with `ticket` may be applied to a project
   * (see offerProjectBranches, which states the rules): true, and it is
   * recorded as landed; false when a list sent after it already has;
   * BRANCH_READ_SET_ASIDE when a local write landed while it was out. The
   * one decision for a list read on its own and for a project's list in an
   * all-projects reply.
   */
  private admitBranchRead(
    projectId: string,
    ticket: number,
    replace = false
  ): boolean | typeof BRANCH_READ_SET_ASIDE {
    const readKey = `branches:${projectId}`;
    if (!replace) {
      if (this.readOrder.overtaken(readKey, ticket)) return false;
      if (this.readOrder.overtaken(`branch-writes:${projectId}`, ticket)) {
        return BRANCH_READ_SET_ASIDE;
      }
    }
    // Only a list that is applied counts as landed, so a read is refused
    // only for a later-sent list the store really shows. A list set aside
    // above leaves no mark: an older read still out is then set aside too,
    // not refused, and its caller reads again (refreshProject relies on it).
    this.readOrder.admit(readKey, ticket);
    return true;
  }

  /**
   * Apply an all-projects branch reply to the projects a bulk hydration
   * covers, with one assignment to the branch map.
   *
   * Each project's list goes through what a list read on its own does (see
   * offerProjectBranches): refused when a list sent later has landed for
   * that project, set aside and read again when a local write has, and
   * merged over the list shown so a worktree path is not lost to a stale
   * null. So one project's later read or local write holds back that
   * project's entry alone, and the rest of the reply still applies. A
   * project the reply does not list, or that left the project list while
   * the read was out, is skipped.
   */
  private applyBranchesByProject(
    branchesByProject: Record<string, Branch[]>,
    covered: ReadonlySet<string>,
    generation: number,
    ticket: number
  ): void {
    if (generation !== this.loadGeneration) return;
    let next: Map<string, Branch[]> | null = null;
    const setAside: string[] = [];
    for (const { id: projectId } of this._projects) {
      const branches = covered.has(projectId) ? branchesByProject[projectId] : undefined;
      if (branches === undefined) continue;
      const admitted = this.admitBranchRead(projectId, ticket);
      if (admitted === BRANCH_READ_SET_ASIDE) setAside.push(projectId);
      if (admitted !== true) continue;
      next ??= new Map(this._branchesByProject);
      next.set(
        projectId,
        mergeBranchesPreservingWorktree(this._branchesByProject.get(projectId) || [], branches)
      );
    }
    if (next) this._branchesByProject = next;
    for (const projectId of setAside) void this.refetchProjectBranches(projectId);
  }

  /**
   * Offer a fetched branch list (see offerProjectBranches), and have the
   * list read again in the background when it is set aside. For the reads
   * nothing awaits: hydration and the event-driven refetch. refreshProject,
   * which callers do await, reads again itself.
   */
  private applyProjectBranches(
    projectId: string,
    branches: Branch[],
    generation: number,
    ticket: number
  ): void {
    const offered = this.offerProjectBranches(projectId, branches, generation, ticket);
    if (offered === BRANCH_READ_SET_ASIDE) void this.refetchProjectBranches(projectId);
  }

  private applyProjectRepos(
    projectId: string,
    repos: ProjectRepo[],
    generation: number,
    ticket: number
  ): void {
    if (generation !== this.loadGeneration || !this.hasProject(projectId)) return;
    if (!this.readOrder.admit(`repos:${projectId}`, ticket)) return;
    this._reposByProject = new Map(this._reposByProject).set(projectId, repos);
    void repoBadgeStore.ensureForRepos(
      repos.map((r) => ({ githubRepo: r.githubRepo, subpath: r.subpath }))
    );
  }

  /**
   * Apply an all-projects repo reply to the projects a bulk hydration covers,
   * with one assignment to the repo map and one badge request, which names
   * each repo once. Each project's list is ordered against that project's
   * other repo reads, as in applyProjectRepos.
   */
  private applyReposByProject(
    reposByProject: Record<string, ProjectRepo[]>,
    covered: ReadonlySet<string>,
    generation: number,
    ticket: number
  ): void {
    if (generation !== this.loadGeneration) return;
    let next: Map<string, ProjectRepo[]> | null = null;
    // Each repo once, however many projects attach it: the request is passed
    // on whole to the backend, which names the missing badges from it.
    const badgeRepos = new Map<string, { githubRepo: string; subpath: string | null }>();
    for (const { id: projectId } of this._projects) {
      const repos = covered.has(projectId) ? reposByProject[projectId] : undefined;
      if (repos === undefined) continue;
      if (!this.readOrder.admit(`repos:${projectId}`, ticket)) continue;
      next ??= new Map(this._reposByProject);
      next.set(projectId, repos);
      for (const { githubRepo, subpath } of repos) {
        // NUL can't occur in either part, so no two repos share a key.
        const key = `${githubRepo}\u0000${subpath ?? ''}`;
        if (!badgeRepos.has(key)) badgeRepos.set(key, { githubRepo, subpath });
      }
    }
    if (next) this._reposByProject = next;
    void repoBadgeStore.ensureForRepos([...badgeRepos.values()]);
  }

  private cancelBackgroundHydration(): void {
    this.backgroundHydrationCancel?.();
    this.backgroundHydrationCancel = null;
  }

  /**
   * Hydrate projects through the idle queue so neither the first fill nor a
   * background refresh contends with foreground work. Enough of them
   * (BULK_HYDRATION_MIN_PROJECTS) go out together in one idle step, as one
   * bulk hydration applied once: a hard reload of the list is then two reads
   * and one pass over the cards however many projects there are. Fewer go
   * one at a time, a project per idle step.
   */
  private scheduleBackgroundHydration(projectIds: string[], generation: number): void {
    this.cancelBackgroundHydration();

    const queue = [...projectIds];
    if (queue.length === 0) return;

    if (queue.length >= BULK_HYDRATION_MIN_PROJECTS) {
      this.backgroundHydrationCancel = scheduleDeferredTask(() => {
        this.backgroundHydrationCancel = null;
        if (generation !== this.loadGeneration) return;
        // Skip what someone already fetched under this load (the selected
        // project, or the grid's sweep), as the one-at-a-time queue below
        // does. What is still in flight is joined, not read again.
        void this.hydrateProjects(
          queue.filter((projectId) => this._hydratedProjects.get(projectId) !== generation),
          generation
        );
      }, BACKGROUND_HYDRATION_IDLE_TIMEOUT_MS);
      return;
    }

    let cancelled = false;
    let cancelScheduledTask: (() => void) | null = null;

    const scheduleNext = () => {
      if (cancelled || generation !== this.loadGeneration || queue.length === 0) return;
      cancelScheduledTask = scheduleDeferredTask(hydrateNext, BACKGROUND_HYDRATION_IDLE_TIMEOUT_MS);
    };

    const hydrateNext = () => {
      cancelScheduledTask = null;
      if (cancelled || generation !== this.loadGeneration) return;

      const projectId = queue.shift();
      if (!projectId) return;

      // Someone already fetched this project under the current load (the
      // selected project, or the grid's sweep) — the drip exists to fill in
      // what nobody asked for, not to refetch what just landed. The next load
      // bumps the generation, so a revalidation still refreshes everything.
      // A project deleted while it waited is skipped too: a targeted reload
      // prunes it without cancelling this queue, and reading it would only
      // fail.
      if (!this.hasProject(projectId) || this._hydratedProjects.get(projectId) === generation) {
        scheduleNext();
        return;
      }

      void this.hydrateOnce(projectId, generation).finally(scheduleNext);
    };

    cancelScheduledTask = scheduleDeferredTask(hydrateNext, BACKGROUND_HYDRATION_IDLE_TIMEOUT_MS);
    this.backgroundHydrationCancel = () => {
      cancelled = true;
      cancelScheduledTask?.();
    };
  }

  // ── Home repos (shared listReposForHome cache) ──

  /** Same contract as ensureLoaded(): first call fetches, later calls
   *  resolve instantly and revalidate in the background. */
  async ensureHomeReposLoaded(): Promise<void> {
    if (this._homeRepos !== null) {
      if (!this.homeReposInFlight) void this.startHomeReposFetch();
      return;
    }
    await (this.homeReposInFlight ?? this.startHomeReposFetch());
  }

  /** Force a home-repos refetch after a pin/clone mutation changed them. */
  async refreshHomeRepos(): Promise<void> {
    await this.startHomeReposFetch();
  }

  private startHomeReposFetch(): Promise<void> {
    // Token instead of generation: a pinned-repos change mid-fetch must win
    // over the response already in flight.
    const token = ++this.homeReposFetchToken;
    if (this._homeRepos === null) {
      this._homeReposLoading = true;
    }
    const fetchPromise = (async () => {
      try {
        const repos = await commands.listReposForHome();
        if (token !== this.homeReposFetchToken) return;
        this._homeRepos = repos;
      } catch (e) {
        console.error('[projectsData] Failed to load home repos:', e);
      } finally {
        if (token === this.homeReposFetchToken) {
          this._homeReposLoading = false;
        }
      }
    })();
    this.homeReposInFlight = fetchPromise;
    void fetchPromise.finally(() => {
      // Identity check: a newer fetch may have taken over the in-flight slot.
      if (this.homeReposInFlight === fetchPromise) {
        this.homeReposInFlight = null;
      }
    });
    return fetchPromise;
  }

  // ── Project-delete lifecycle ──
  //
  // Confirming a delete sets the project's marker, and liveProjects drops the
  // project at once: its card leaves the grid and the sidebar without waiting
  // on the backend, whose worktree teardown runs before the row delete and can
  // take seconds. projects still lists it, so guards and list positions keep
  // working.
  //
  // On the success path the data side is event-driven: the backend delete
  // publishes project-changed, whose list refetch removes the project and its
  // marker in one apply (see applyProjectList). Until that lands, the marker
  // keeps the project hidden — even if a fetch started before the delete
  // resolves late and still contains it — so a stale refetch never brings
  // the card back. Only projectDeleteFailed does that, alongside its toast.
  // The trade: a delete that fails shows the card vanish and then return.
  //
  // The marker does more than hide the project, though: flushBranchChanges
  // suppresses branch refetches for a deleting project, since the cascade
  // emits one branch-changed per branch and refetching a doomed list N times
  // would be pure churn. That debt has to be paid back when the delete fails —
  // see projectDeleteFailed.

  projectDeleteStarted(projectId: string, name: string): void {
    this._deletingProjectNames = new Map(this._deletingProjectNames).set(projectId, name);
  }

  /** Clear the in-progress marker for a delete that failed — the project is
   *  still alive, so no project-changed refetch will prune it, and clearing
   *  the marker puts it back in liveProjects. The cascade may have deleted
   *  branch rows before failing, and flushBranchChanges dropped those
   *  branch-changed events on the floor, so refetch the list here: it is the
   *  only signal that will ever repair it. Clearing the marker first also
   *  lets any trailing branch-changed from the same cascade take the normal
   *  path instead of being dropped a second time. */
  projectDeleteFailed(projectId: string): void {
    const next = new Map(this._deletingProjectNames);
    next.delete(projectId);
    this._deletingProjectNames = next;
    // Same guard as flushBranchChanges: an un-hydrated project has nothing
    // painted to repair. In practice this always fires — the delete flow awaits
    // ensureProjectHydrated before starting — so the guard is consistency, not
    // a live filter here.
    this.refetchBranchesIfHydrated(projectId);
  }

  // ── Project-transfer lifecycle ──
  //
  // project-transfer-progress reaches every window, so a transfer started
  // elsewhere shows here too; the window that started it marks it at once
  // and clears it when transfer_project settles. Each time the event's
  // channel goes live (startup, every web-mode reconnect or event gap) the
  // backend's registry is read too, so a window loaded mid-transfer shows it
  // and one that missed a terminal event lets it go. The data side is
  // event-driven like a delete: the switch publishes project-changed and
  // branch-changed, which refetch the project's location and branches.

  /** Mark a transfer this window just confirmed, ahead of its first event. */
  projectTransferStarted(projectId: string, direction: ProjectTransferDirection): void {
    if (this._transferringProjects.has(projectId)) return;
    this.setTransferState(projectId, {
      transferId: null,
      direction,
      phase: 'checking',
      branchId: null,
      projectRepoId: null,
      detail: null,
      bytes: null,
    });
  }

  /**
   * Clear a transfer once it is over. `transferId` is the result's, on
   * success, or a terminal event's. Without one (transfer_project rejected; a
   * failure's id is only known from its events) only this window's
   * unconfirmed start is cleared: a transfer with an id may be another
   * window's, still running, and its own `failed` event clears it. The
   * registry is read again in case it ended without one.
   */
  projectTransferFinished(projectId: string, transferId?: string): void {
    if (transferId) this.finishedTransferIds.add(transferId);
    const current = this._transferProgress.get(projectId);
    if (!current) return;
    if (current.transferId !== null && current.transferId !== transferId) {
      if (!transferId) void this.syncTransfers();
      return;
    }
    const transferring = new Map(this._transferringProjects);
    transferring.delete(projectId);
    this._transferringProjects = transferring;
    const progress = new Map(this._transferProgress);
    progress.delete(projectId);
    this._transferProgress = progress;
  }

  /**
   * Reconcile the transfers shown with the backend's registry. A transfer it
   * no longer lists is over; one it lists that this window hasn't seen (or
   * only as an unconfirmed start) is shown at its latest progress. Only
   * transfers known before the read are cleared by it: one whose first event
   * landed while the read was in flight is newer than the answer.
   */
  private async syncTransfers(): Promise<void> {
    const knownBefore = new Set(
      [...this._transferProgress.values()].map((state) => state.transferId)
    );
    let live: ProjectTransferProgress[];
    try {
      live = await commands.listTransferringProjects();
    } catch (e) {
      console.error('[projectsData] Failed to read the running transfers:', e);
      return;
    }
    const liveIds = new Set(live.map((progress) => progress.transferId));
    for (const [projectId, state] of [...this._transferProgress]) {
      if (state.transferId && knownBefore.has(state.transferId) && !liveIds.has(state.transferId)) {
        this.projectTransferFinished(projectId, state.transferId);
      }
    }
    for (const progress of live) {
      // Events this window already applied for it are at least as recent.
      if (this._transferProgress.get(progress.projectId)?.transferId === progress.transferId) {
        continue;
      }
      this.applyTransferProgress(progress);
    }
  }

  private applyTransferProgress(event: ProjectTransferProgress): void {
    if (this.finishedTransferIds.has(event.transferId)) return;
    if (event.phase === 'completed' || event.phase === 'failed') {
      this.projectTransferFinished(event.projectId, event.transferId);
      return;
    }
    this.setTransferState(event.projectId, {
      transferId: event.transferId,
      direction: event.direction,
      phase: event.phase,
      branchId: event.branchId,
      projectRepoId: event.projectRepoId,
      detail: event.detail,
      bytes: event.bytes,
    });
  }

  private setTransferState(projectId: string, state: ProjectTransferState): void {
    if (this._transferringProjects.get(projectId) !== state.direction) {
      this._transferringProjects = new Map(this._transferringProjects).set(
        projectId,
        state.direction
      );
    }
    this._transferProgress = new Map(this._transferProgress).set(projectId, state);
  }

  // ── Event listeners ──

  /** Start the global backend/window listeners. Idempotent; call once at app
   *  startup (App.svelte) rather than per-view. */
  startListeners(): void {
    if (this.listening) return;
    this.listening = true;

    // A PR-polling cycle emits one `pr-status-changed` per branch, so a storm
    // of N branches arrives as N separate events. Rebuilding the branch map
    // per event means N allocations + N derivation re-runs; buffer the events
    // and apply a single rebuild per frame so a burst coalesces into one
    // reactive flush without dropping any update.
    this.unlisteners.push(
      listenToEvent<PrStatusChangedEvent>('pr-status-changed', (payload) => {
        this.pendingPrStatusEvents.push(payload);
        this.prStatusFlushCancel ??= scheduleFrame(() => this.flushPrStatusEvents());
      })
    );

    // Refresh a project's branches when a commit session completes so the
    // sprout/draft-PR icon flips as soon as the first commit lands. Session
    // starts (running/queued) also invalidate the branch timeline: sessions
    // launched outside this client (stagedctl, another window) otherwise
    // never surface their pending artifact stub.
    this.unlisteners.push(
      listenToEvent<SessionStatusPayload>('session-status-changed', (payload) => {
        if ((payload.status === 'running' || payload.status === 'queued') && payload.branchId) {
          commands.invalidateProjectBranchTimelines([payload.branchId]);
        }
        this.handleCommitSessionCompleted(payload);
      })
    );

    // Images created outside this client (stagedctl) land directly in a
    // branch timeline; the creating client refreshes itself, everyone else
    // refreshes here.
    this.unlisteners.push(
      listenToEvent<ImageCreatedPayload>('image-created', (payload) => {
        if (payload.branchId) {
          commands.invalidateProjectBranchTimelines([payload.branchId]);
        }
      })
    );

    // Backend-driven setup progress: emitted after repo creation, worktree
    // setup, and prerun actions. Only refresh display state here — setup
    // itself is owned by the backend.
    this.unlisteners.push(
      listenToEvent<string>('project-setup-progress', (projectId) => {
        void this.refreshProject(projectId);
      })
    );

    // Store change feed: a project write in any window (or the backend)
    // reloads the list, and refetches the project it names. Create/rename
    // land the new row; a delete's refetch prunes the project and its
    // deleting marker together. See handleProjectChanged.
    this.unlisteners.push(
      listenToEvent<ProjectChangedEvent>('project-changed', (payload) => {
        this.handleProjectChanged(payload?.projectId ?? null);
      })
    );

    // Store change feed: refetch the branch lists a branch write touched.
    // Buffered per frame like pr-status-changed — a bulk operation arrives
    // as one event per branch, and each refetch replaces the whole list.
    this.unlisteners.push(
      listenToEvent<BranchChangedEvent>('branch-changed', (payload) => {
        // The feed's lag recovery: place its arrival in the read order, so
        // the flush can tell the hydrations sent before it from those sent
        // after (see flushBranchChanges).
        if (payload.branchId === null) this.readOrder.wrote(BRANCH_LAG_RECOVERY);
        this.pendingBranchChanges.push(payload);
        this.branchChangedFlushCancel ??= scheduleFrame(() => this.flushBranchChanges());
      })
    );

    // Project transfers, from this window or any other: the status spinner
    // and ProjectHome's progress banner. The registry read on each
    // establishment covers what the event stream can't: a transfer already
    // running when this window loaded, and events lost to a socket gap.
    this.unlisteners.push(
      listenToEvent<ProjectTransferProgress>(
        'project-transfer-progress',
        (payload) => {
          this.applyTransferProgress(payload);
        },
        {
          onEstablished: () => {
            void this.syncTransfers();
          },
        }
      )
    );

    // Store change feed: badges and the home repo list (pins, recents,
    // affinities) are all repo writes.
    this.unlisteners.push(
      listenToEvent<ReposChangedEvent>('repos-changed', () => {
        void repoBadgeStore.loadAll({ force: true });
        if (this._homeRepos !== null || this.homeReposInFlight) {
          void this.startHomeReposFetch();
        }
      })
    );

    const onCacheStale = () => {
      void this.refresh();
    };
    window.addEventListener('cache-stale', onCacheStale);
    this.unlisteners.push(() => window.removeEventListener('cache-stale', onCacheStale));
  }

  /** Tear down all listeners (tests, symmetry with startListeners). */
  stopListeners(): void {
    for (const unlisten of this.unlisteners) {
      unlisten();
    }
    this.unlisteners = [];
    this.prStatusFlushCancel?.();
    this.prStatusFlushCancel = null;
    this.pendingPrStatusEvents = [];
    this.branchChangedFlushCancel?.();
    this.branchChangedFlushCancel = null;
    this.pendingBranchChanges = [];
    this.cancelBackgroundHydration();
    this.listening = false;
  }

  /**
   * A project write: the store change feed's project-changed.
   *
   * The event names the project that was written to, and a write to one
   * project changes no other (a branch move publishes once for each side).
   * So the reload is targeted: the list is read again softly, and only that
   * project's branches and repos are refetched (refetchChangedProjects).
   * Every other project keeps its lists, its hydration in flight, and, the
   * list being applied through reuseUnchangedProjects, its object.
   *
   * This used to be a hard reload, whose generation bump threw away every
   * apply still in flight, as possibly read before the write. The soft
   * reload keeps the generation, so each of those is ordered on its own
   * instead (see readOrder), by when it was sent:
   *
   * - A project list sent before this one is refused once this one has
   *   landed (admitProjectList), so it cannot bring a deleted project back
   *   or take a new one away. If it lands first, this one replaces it.
   * - A read of the named project sent before the event loses to the
   *   refetch, which is sent after it, in whichever order the two land
   *   (admitBranchRead, applyProjectRepos). That covers a hydration in
   *   flight, a bulk one included: it still applies for every other project.
   * - A deleted project is pruned with its deleting marker by the list
   *   (applyProjectList). Whatever lands for it afterwards is dropped, as it
   *   is no longer listed, and it is not refetched.
   *
   * Three cases keep the hard reload. An event that names no project is the
   * feed's lag recovery: it lost changes it cannot describe, so every list
   * is suspect. Before the first list has landed there is nothing shown for
   * a targeted reload to keep, so the load is simply started again. And
   * after a reload or a bulk hydration that failed (see reloadFailed), the
   * next event starts everything again, as every event used to.
   */
  private handleProjectChanged(projectId: string | null): void {
    if (projectId === null || !this._loaded || this.reloadFailed) {
      void this.revalidate({ force: true });
      return;
    }
    void this.revalidate({ force: true, soft: true, changedProjectIds: [projectId] });
  }

  /**
   * Refetch the branches and repos of the projects a project-changed event
   * named: a repo attached or removed, a branch moved in or out, a transfer.
   * Called once the list read after the event has landed, since that list is
   * what says which of them the store should show:
   *
   * - Not listed: the project was deleted, and the list has just pruned it.
   *   Reading it would only fail.
   * - Listed and hydrated, or with its first fetch in flight: refetched, now
   *   or once that fetch settles (see whenHydrated).
   * - Listed and never fetched, which includes a project this list has just
   *   brought in: left to its first hydration, which is yet to be sent and
   *   so reads what the write left. applyProjectList queues that hydration
   *   for any project nothing has seeded.
   */
  private refetchChangedProjects(projectIds: readonly string[]): void {
    for (const projectId of projectIds) {
      this.whenHydrated(projectId, () => {
        void this.refetchProjectBranches(projectId);
        void this.refetchProjectRepos(projectId);
      });
    }
  }

  private flushPrStatusEvents(): void {
    this.prStatusFlushCancel = null;
    if (this.pendingPrStatusEvents.length === 0) return;
    const events = this.pendingPrStatusEvents;
    this.pendingPrStatusEvents = [];

    // Apply every buffered event onto one fresh Map. Each event re-scans the
    // in-progress map, so multiple updates to the same project compound
    // correctly instead of clobbering one another.
    const next = new Map(this._branchesByProject);
    for (const payload of events) {
      for (const [projectId, branches] of next) {
        const branchIndex = branches.findIndex((b) => b.id === payload.branchId);
        if (branchIndex !== -1) {
          const updatedBranches = [...branches];
          updatedBranches[branchIndex] = {
            ...branches[branchIndex],
            prState: payload.prState,
            prChecksStatus: payload.prChecksStatus,
            prReviewDecision: payload.prReviewDecision,
            prMergeable: payload.prMergeable,
            prDraft: payload.prDraft,
            prHeadSha: payload.prHeadSha,
            prFetchedAt: payload.prFetchedAt,
          };
          next.set(projectId, updatedBranches);
          break;
        }
      }
    }
    this._branchesByProject = next;
  }

  private handleCommitSessionCompleted(payload: SessionStatusPayload): void {
    if (payload.status !== 'completed') return;
    if (payload.sessionType !== 'commit') return;
    const projectId = payload.projectId;
    if (!projectId) return;
    this.refetchBranchesIfHydrated(projectId);
  }

  /**
   * Map a burst of branch-changed events to the set of projects whose branch
   * lists need refetching: the project the backend resolved, plus any known
   * project whose list still holds the branch. The feed names every project a
   * write touches — a move publishes once per side — so that scan is the
   * fallback for what the backend couldn't see: an unresolved projectId, or a
   * list this window holds that no longer matches the row. Collection stays
   * broad; the single hydration filter is in refetchBranchesIfHydrated.
   *
   * A null branchId is the feed's lag recovery — it can't name what it
   * dropped, so every list the store holds is suspect. Those are read again
   * together, as one bulk hydration, when there are enough of them (see
   * projectsToRereadAfterLag): one project at a time is a read and a pass
   * over every card per project, at the moment the backend is busy enough to
   * have lagged.
   */
  private flushBranchChanges(): void {
    this.branchChangedFlushCancel = null;
    const events = this.pendingBranchChanges;
    this.pendingBranchChanges = [];
    if (events.length === 0) return;

    const projectIds = new Set<string>();
    let lagRecovery = false;
    for (const { branchId, projectId } of events) {
      if (branchId === null) {
        lagRecovery = true;
        continue;
      }
      if (projectId) projectIds.add(projectId);
      for (const [knownProjectId, branches] of this._branchesByProject) {
        if (branches.some((b) => b.id === branchId)) projectIds.add(knownProjectId);
      }
    }
    if (lagRecovery) {
      const suspect = this.projectsToRereadAfterLag();
      if (suspect.length >= BULK_HYDRATION_MIN_PROJECTS) {
        // Sent now, so after every event in this burst: the projects it
        // covers need no read of their own for the events that named them.
        void this.hydrateInBulk(suspect, this.loadGeneration, { force: true });
        for (const projectId of suspect) projectIds.delete(projectId);
      } else {
        for (const projectId of suspect) projectIds.add(projectId);
      }
    }
    for (const projectId of projectIds) {
      // A deleting project's teardown emits one event per branch — refetching a
      // list that's mid-cascade-delete would just churn. If that delete fails,
      // projectDeleteFailed refetches once to repair what was skipped.
      if (this.isProjectDeleting(projectId)) continue;
      this.refetchBranchesIfHydrated(projectId);
    }
  }

  /**
   * The projects whose lists the feed's lag recovery puts in doubt: every
   * one the store holds fetched lists for, or is about to.
   *
   * - Hydrated: its lists were read before the recovery arrived.
   * - A hydration in flight that was sent before the recovery arrived: it
   *   may have read what the recovery says is out of date. One sent after it
   *   already reads what the recovery asks for, and is left alone. That is
   *   the usual case: the recovery's project-changed, which names no project
   *   either, started a hard reload in the same dispatch, and its hydration
   *   may be out by the time this flush runs a frame later.
   * - A refreshProject read in flight: it marks its project hydrated when it
   *   lands (see whenHydrated).
   *
   * A hydration sent after the recovery that has also landed by this flush,
   * inside one frame, is not told apart from an older one: its projects are
   * read once more than they needed.
   *
   * A project never fetched is left to its first hydration, as for a named
   * event, and a deleting one is skipped as flushBranchChanges skips it.
   */
  private projectsToRereadAfterLag(): string[] {
    return this._projects
      .map((project) => project.id)
      .filter((projectId) => {
        if (this.isProjectDeleting(projectId)) return false;
        const inFlight = this.hydrationInFlight.get(projectId);
        if (inFlight) {
          const sentAt = this.hydrationSentAt.get(inFlight) ?? 0;
          return this.readOrder.overtaken(BRANCH_LAG_RECOVERY, sentAt);
        }
        return this._hydratedProjects.has(projectId) || this.refreshInFlight.has(projectId);
      });
  }

  /**
   * Refetch a project's branch list only when the store holds — or is about to
   * hold — real fetched data to repair.
   *
   * Hydrated: refetch now. First hydration in flight, on its own or as part
   * of a bulk one: refetch once it settles, so this event's fresh read applies
   * last — the in-flight fetch may have read pre-mutation state, and skipping
   * outright would leave that stale read painted until the project's next
   * event. Otherwise skip: the entry
   * applyProjectList seeds for every listed project is `[]`, which no view ever
   * painted (they gate on isProjectHydrated), and the eventual first hydration
   * reads post-event state anyway. Fetching there would also half-hydrate the
   * project — branches without repos, still unmarked — so the idle drip would
   * refetch it regardless.
   *
   * Unknown projects are subsumed: hydrated ⊆ known, since markProjectHydrated
   * is only reached for listed or just-created projects and applyProjectList
   * prunes _hydratedProjects against the same fetched list.
   */
  private refetchBranchesIfHydrated(projectId: string): void {
    this.whenHydrated(projectId, () => void this.refetchProjectBranches(projectId));
  }

  /**
   * Run a refetch of a project's lists only when the store holds, or is
   * about to hold, fetched lists to repair: now when the project is
   * hydrated, once its first hydration settles when that is in flight, and
   * not at all otherwise. refetchBranchesIfHydrated gives the reasons.
   *
   * A project that left the list while its hydration was in flight is not
   * refetched: there is nothing left to repair, and the read would fail.
   *
   * A refreshProject read in flight counts as well. It marks the project
   * hydrated when it lands, and it may have been sent before the write this
   * refetch is for. Skipping would leave what it read painted, and the idle
   * drip passes over a project already fetched under its load. So the
   * question is asked again once that read settles.
   */
  private whenHydrated(projectId: string, refetch: () => void): void {
    if (this._hydratedProjects.has(projectId)) {
      refetch();
      return;
    }
    const inFlight = this.hydrationInFlight.get(projectId);
    if (inFlight) {
      void inFlight.then(() => {
        if (this.hasProject(projectId)) refetch();
      });
      return;
    }
    const refresh = this.refreshInFlight.get(projectId);
    if (refresh) void refresh.then(() => this.whenHydrated(projectId, refetch));
  }

  /** Refetch one project's branch list and apply it under the current load
   *  generation. */
  private async refetchProjectBranches(projectId: string): Promise<void> {
    if (this.branchRefetchInFlight.has(projectId)) {
      // The pending read may predate this change. Run one more fetch after it
      // settles rather than racing two responses that can apply out of order.
      this.branchRefetchQueued.add(projectId);
      return;
    }
    this.branchRefetchInFlight.add(projectId);
    const generation = this.loadGeneration;
    const ticket = this.readOrder.issue();
    try {
      const { data: branches, revalidating } = await commands.listBranchesForProject(projectId, {
        force: true,
      });
      this.applyProjectBranches(projectId, branches, generation, ticket);
      if (revalidating) {
        revalidating
          .then((fresh) => {
            this.applyProjectBranches(projectId, fresh, generation, ticket);
          })
          .catch((e) => {
            console.error(
              `[projectsData] Failed to revalidate branches for project ${projectId}:`,
              e
            );
          });
      }
    } catch (e) {
      console.error(`[projectsData] Failed to refetch branches for project ${projectId}:`, e);
    } finally {
      this.branchRefetchInFlight.delete(projectId);
      if (this.branchRefetchQueued.delete(projectId)) {
        void this.refetchProjectBranches(projectId);
      }
    }
  }

  /**
   * Refetch one project's repo list and apply it under the current load
   * generation. Not coalesced like the branch refetch: the events that lead
   * here are already folded into one reload at a time (see revalidate), and
   * two reads that do overlap are ordered by when they were sent.
   */
  private async refetchProjectRepos(projectId: string): Promise<void> {
    const generation = this.loadGeneration;
    const ticket = this.readOrder.issue();
    try {
      const { data: repos, revalidating } = await commands.listProjectRepos(projectId, {
        force: true,
      });
      this.applyProjectRepos(projectId, repos, generation, ticket);
      if (revalidating) {
        revalidating
          .then((fresh) => this.applyProjectRepos(projectId, fresh, generation, ticket))
          .catch((e) => {
            console.error(`[projectsData] Failed to revalidate repos for project ${projectId}:`, e);
          });
      }
    } catch (e) {
      console.error(`[projectsData] Failed to refetch repos for project ${projectId}:`, e);
    }
  }
}

export const projectsDataStore = new ProjectsDataStore();
