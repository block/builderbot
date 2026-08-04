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
 * both modes: the first call does the full fetch; later calls resolve
 * immediately with the in-memory data and kick a background revalidation.
 *
 * ProjectHome, ProjectsList, ProjectsSidebar, and ReposListView all read
 * this store directly; startListeners() is wired once from App.svelte.
 * View-lifecycle side effects (workspaceLifecycle.enqueueInitialSetup,
 * queued-session draining, run-action hydration) intentionally stay out of
 * the store — consuming views wire them by watching branchesByProject.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import * as commands from '../commands';
import { repoBadgeStore } from './repoBadges.svelte';
import type {
  Branch,
  PrStatusChangedEvent,
  Project,
  ProjectRepo,
  RepoHomeItem,
  SessionStatusPayload,
} from '../types';

/** Idle delay between background hydration steps (one project per step). */
const BACKGROUND_HYDRATION_DELAY_MS = 3000;

/**
 * Merge incoming branches with existing ones, preserving worktreePath when
 * a stale async response would overwrite an already-populated value with null.
 */
export function mergeBranchesPreservingWorktree(existing: Branch[], incoming: Branch[]): Branch[] {
  return incoming.map((newBranch) => {
    const prev = existing.find((b) => b.id === newBranch.id);
    if (prev?.worktreePath && !newBranch.worktreePath) {
      return { ...newBranch, worktreePath: prev.worktreePath };
    }
    return newBranch;
  });
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

  // null = never loaded; distinguishes "no repos" from "not fetched yet".
  private _homeRepos = $state<RepoHomeItem[] | null>(null);
  private _homeReposLoading = $state(false);

  /**
   * Guards every async apply: bumped by each full load so responses that
   * resolve after a newer load started are discarded instead of clobbering
   * fresher state. Same role as ProjectHome's loadGeneration.
   */
  private loadGeneration = 0;
  private initialLoad: Promise<void> | null = null;
  private revalidatePending = false;
  private backgroundHydrationCancel: (() => void) | null = null;

  private homeReposInFlight: Promise<void> | null = null;
  private homeReposFetchToken = 0;

  private listening = false;
  private unlisteners: UnlistenFn[] = [];
  private pendingPrStatusEvents: PrStatusChangedEvent[] = [];
  private prStatusFlushCancel: (() => void) | null = null;

  // ── Reactive reads ──

  get projects(): Project[] {
    return this._projects;
  }

  get branchesByProject(): Map<string, Branch[]> {
    return this._branchesByProject;
  }

  get reposByProject(): Map<string, ProjectRepo[]> {
    return this._reposByProject;
  }

  /**
   * Repo count per project. Falls back to 1 for un-hydrated single-repo
   * projects (githubRepo set) so counts render sensibly before repos load.
   */
  get repoCountsByProject(): Map<string, number> {
    return new Map(
      this._projects.map((project) => {
        const repos = this._reposByProject.get(project.id);
        return [project.id, repos ? repos.length : project.githubRepo ? 1 : 0] as const;
      })
    );
  }

  get loading(): boolean {
    return this._loading;
  }

  get error(): string | null {
    return this._error;
  }

  get loaded(): boolean {
    return this._loaded;
  }

  get deletingProjectNames(): Map<string, string> {
    return this._deletingProjectNames;
  }

  isProjectDeleting(projectId: string): boolean {
    return this._deletingProjectNames.has(projectId);
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
   * Make sure the store holds data. The first call performs the full fetch
   * (project list, then branches + repos for every project) and resolves when
   * it completes. Later calls resolve immediately — the in-memory data is
   * already renderable — and kick a background revalidation that drips
   * per-project hydration through the idle queue.
   *
   * Load failures don't reject; they surface through `error` so callers can
   * render them, mirroring the views' loadData() pattern.
   */
  async ensureLoaded(): Promise<void> {
    if (this._loaded) {
      void this.revalidate();
      return;
    }
    this.initialLoad ??= this.loadProjectsAndHydrate({ hydration: 'eager' }).finally(() => {
      this.initialLoad = null;
    });
    return this.initialLoad;
  }

  /** Full reload (used by cache-stale and the project-delete flow). Always
   *  starts a new load — the generation bump discards in-flight applies. */
  async refresh(): Promise<void> {
    await this.loadProjectsAndHydrate({ hydration: 'eager' });
    if (this._homeRepos !== null) {
      void this.startHomeReposFetch();
    }
  }

  /**
   * Hydrate branches + repos for one project. Foreground (default) fetches
   * immediately — use for the selected project. Background defers to the
   * idle queue so it doesn't compete with a view transition.
   */
  async hydrateProject(
    projectId: string,
    options: { priority?: 'foreground' | 'background' } = {}
  ): Promise<void> {
    const generation = this.loadGeneration;
    if (options.priority === 'background') {
      scheduleDeferredTask(() => {
        if (generation !== this.loadGeneration) return;
        this.hydrateProjectInternal(projectId, generation).catch((e) => {
          console.error(`[projectsData] Failed to background hydrate project '${projectId}':`, e);
        });
      }, BACKGROUND_HYDRATION_DELAY_MS);
      return;
    }
    await this.hydrateProjectInternal(projectId, generation);
  }

  /**
   * Refetch the project list plus one project's branches and repos. Used
   * after mutations that reshape a single project (repo added or removed)
   * and by the project-setup-progress listener. Invalidates the project's
   * branch timelines so downstream views refetch them.
   */
  async refreshProject(projectId: string): Promise<void> {
    const generation = this.loadGeneration;
    try {
      const [projectsResult, branchesResult, reposResult] = await Promise.all([
        commands.listProjects(),
        commands.listBranchesForProject(projectId),
        commands.listProjectRepos(projectId),
      ]);
      if (generation !== this.loadGeneration) return;
      this._projects = projectsResult.data;
      const mergedBranches = this.applyProjectBranches(projectId, branchesResult.data, generation);
      if (mergedBranches) {
        commands.invalidateProjectBranchTimelines(mergedBranches.map((b) => b.id));
      }
      this.applyProjectRepos(projectId, reposResult.data, generation);
    } catch (e) {
      console.error(`[projectsData] Failed to refresh project '${projectId}':`, e);
    }
  }

  /**
   * Register a newly created project immediately — closing the creation
   * modal must not wait for a reload — then hydrate its branches and repos
   * in the background.
   */
  projectCreated(project: Project): void {
    if (!this._projects.some((p) => p.id === project.id)) {
      this._projects = [...this._projects, project];
    }
    if (!this._branchesByProject.has(project.id)) {
      this._branchesByProject = new Map(this._branchesByProject).set(project.id, []);
    }
    this.hydrateProject(project.id).catch((e) => {
      console.error(`[projectsData] Failed to hydrate newly created project '${project.id}':`, e);
    });
  }

  /**
   * Replace the branch map wholesale. Entry point for synchronous
   * view-driven updates (workspaceLifecycle status/worktree writes, branch
   * rename/delete) so every consumer sees them; fetch-driven applies go
   * through the generation-guarded paths instead.
   */
  setBranchesByProject(next: Map<string, Branch[]>): void {
    this._branchesByProject = next;
  }

  private async revalidate(): Promise<void> {
    if (this.revalidatePending) return;
    this.revalidatePending = true;
    try {
      await this.loadProjectsAndHydrate({ hydration: 'background' });
    } finally {
      this.revalidatePending = false;
    }
  }

  private async loadProjectsAndHydrate(options: {
    hydration: 'eager' | 'background';
  }): Promise<void> {
    const generation = ++this.loadGeneration;
    this.cancelBackgroundHydration();
    if (this._projects.length === 0) {
      this._loading = true;
    }
    this._error = null;
    await repoBadgeStore.loadAll();
    try {
      const { data, revalidating } = await commands.listProjects();
      if (generation !== this.loadGeneration) return;
      await this.applyProjectList(data, generation, options);
      if (generation !== this.loadGeneration) return;
      this._loaded = true;

      if (revalidating) {
        // Applied outside the awaited chain so callers aren't blocked on the
        // SWR refresh — they already have renderable data.
        revalidating
          .then((fresh) => this.applyProjectList(fresh, generation, options))
          .catch((e) => {
            console.error('[projectsData] Failed to revalidate project list:', e);
          });
      }
    } catch (e) {
      if (generation !== this.loadGeneration) return;
      this._error = e instanceof Error ? e.message : String(e);
    } finally {
      if (generation === this.loadGeneration) {
        this._loading = false;
      }
    }
  }

  /**
   * Apply a fetched project list: seed branch entries so per-project
   * consumers can render immediately, prune state for removed projects, and
   * hydrate branches + repos — in parallel for eager loads, via the idle
   * drip for background revalidations.
   */
  private async applyProjectList(
    projectList: Project[],
    generation: number,
    options: { hydration: 'eager' | 'background' }
  ): Promise<void> {
    if (generation !== this.loadGeneration) return;
    this._projects = projectList;

    const branchMap = new Map<string, Branch[]>();
    for (const project of projectList) {
      branchMap.set(project.id, this._branchesByProject.get(project.id) || []);
    }
    this._branchesByProject = branchMap;

    const projectIds = new Set(projectList.map((p) => p.id));
    const prunedRepos = new Map<string, ProjectRepo[]>();
    for (const [projectId, repos] of this._reposByProject) {
      if (projectIds.has(projectId)) prunedRepos.set(projectId, repos);
    }
    this._reposByProject = prunedRepos;

    if (options.hydration === 'eager') {
      await Promise.all(
        projectList.map(async (project) => {
          try {
            await this.hydrateProjectInternal(project.id, generation);
          } catch (e) {
            console.error(`[projectsData] Failed to hydrate project '${project.id}':`, e);
          }
        })
      );
    } else {
      this.scheduleBackgroundHydration(
        projectList.map((p) => p.id),
        generation
      );
    }
  }

  private async hydrateProjectInternal(projectId: string, generation: number): Promise<void> {
    const [branchesResult, reposResult] = await Promise.all([
      commands.listBranchesForProject(projectId),
      commands.listProjectRepos(projectId),
    ]);
    if (generation !== this.loadGeneration) return;

    this.applyProjectBranches(projectId, branchesResult.data, generation);
    this.applyProjectRepos(projectId, reposResult.data, generation);

    if (branchesResult.revalidating) {
      branchesResult.revalidating
        .then((fresh) => {
          this.applyProjectBranches(projectId, fresh, generation);
        })
        .catch((e) => {
          console.error(`[projectsData] Failed to revalidate branches for '${projectId}':`, e);
        });
    }

    if (reposResult.revalidating) {
      reposResult.revalidating
        .then((fresh) => this.applyProjectRepos(projectId, fresh, generation))
        .catch((e) => {
          console.error(`[projectsData] Failed to revalidate repos for '${projectId}':`, e);
        });
    }
  }

  private applyProjectBranches(
    projectId: string,
    branches: Branch[],
    generation: number
  ): Branch[] | null {
    if (generation !== this.loadGeneration) return null;

    const mergedBranches = mergeBranchesPreservingWorktree(
      this._branchesByProject.get(projectId) || [],
      branches
    );
    this._branchesByProject = new Map(this._branchesByProject).set(projectId, mergedBranches);
    return mergedBranches;
  }

  private applyProjectRepos(projectId: string, repos: ProjectRepo[], generation: number): void {
    if (generation !== this.loadGeneration) return;
    this._reposByProject = new Map(this._reposByProject).set(projectId, repos);
    void repoBadgeStore.ensureForRepos(
      repos.map((r) => ({ githubRepo: r.githubRepo, subpath: r.subpath }))
    );
  }

  private cancelBackgroundHydration(): void {
    this.backgroundHydrationCancel?.();
    this.backgroundHydrationCancel = null;
  }

  /** Hydrate projects one at a time through the idle queue so background
   *  refreshes never contend with foreground work. */
  private scheduleBackgroundHydration(projectIds: string[], generation: number): void {
    this.cancelBackgroundHydration();

    const queue = [...projectIds];
    if (queue.length === 0) return;

    let cancelled = false;
    let cancelScheduledTask: (() => void) | null = null;

    const hydrateNext = () => {
      cancelScheduledTask = null;
      if (cancelled || generation !== this.loadGeneration) return;

      const projectId = queue.shift();
      if (!projectId) return;

      this.hydrateProjectInternal(projectId, generation)
        .catch((e) => {
          console.error(`[projectsData] Failed to background hydrate project '${projectId}':`, e);
        })
        .finally(() => {
          if (cancelled || generation !== this.loadGeneration || queue.length === 0) return;
          cancelScheduledTask = scheduleDeferredTask(hydrateNext, BACKGROUND_HYDRATION_DELAY_MS);
        });
    };

    cancelScheduledTask = scheduleDeferredTask(hydrateNext, BACKGROUND_HYDRATION_DELAY_MS);
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
  // Replaces the staged:project-delete-start/end window-event relay between
  // ProjectsList and ProjectHome: the delete flow calls these directly and
  // every consumer sees the same deletingProjectNames.

  projectDeleteStarted(projectId: string, name: string): void {
    this._deletingProjectNames = new Map(this._deletingProjectNames).set(projectId, name);
  }

  /** Mark a delete finished. `removed` prunes the project from the store
   *  (backend deletion succeeded); omit it when the delete failed. */
  projectDeleteFinished(projectId: string, options: { removed?: boolean } = {}): void {
    const next = new Map(this._deletingProjectNames);
    next.delete(projectId);
    this._deletingProjectNames = next;
    if (options.removed) {
      this.removeProject(projectId);
    }
  }

  private removeProject(projectId: string): void {
    this._projects = this._projects.filter((p) => p.id !== projectId);
    const branches = new Map(this._branchesByProject);
    branches.delete(projectId);
    this._branchesByProject = branches;
    const repos = new Map(this._reposByProject);
    repos.delete(projectId);
    this._reposByProject = repos;
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
    // sprout/draft-PR icon flips as soon as the first commit lands.
    this.unlisteners.push(
      listenToEvent<SessionStatusPayload>('session-status-changed', (payload) => {
        void this.handleCommitSessionCompleted(payload);
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

    const onCacheStale = () => {
      void this.refresh();
    };
    window.addEventListener('cache-stale', onCacheStale);
    this.unlisteners.push(() => window.removeEventListener('cache-stale', onCacheStale));

    const onPinnedReposChanged = () => {
      if (this._homeRepos !== null || this.homeReposInFlight) {
        void this.startHomeReposFetch();
      }
    };
    window.addEventListener('staged:pinned-repos-changed', onPinnedReposChanged);
    this.unlisteners.push(() =>
      window.removeEventListener('staged:pinned-repos-changed', onPinnedReposChanged)
    );
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
    this.cancelBackgroundHydration();
    this.listening = false;
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

  private async handleCommitSessionCompleted(payload: SessionStatusPayload): Promise<void> {
    if (payload.status !== 'completed') return;
    if (payload.sessionType !== 'commit') return;
    const projectId = payload.projectId;
    if (!projectId || !this._branchesByProject.has(projectId)) return;
    const generation = this.loadGeneration;
    try {
      const { data: branches, revalidating } = await commands.listBranchesForProject(projectId);
      this.applyProjectBranches(projectId, branches, generation);
      if (revalidating) {
        revalidating
          .then((fresh) => {
            this.applyProjectBranches(projectId, fresh, generation);
          })
          .catch((e) => {
            console.error(
              `[projectsData] Failed to revalidate branches for project ${projectId} after commit:`,
              e
            );
          });
      }
    } catch (e) {
      console.error(
        `[projectsData] Failed to refresh branches for project ${projectId} after commit:`,
        e
      );
    }
  }
}

export const projectsDataStore = new ProjectsDataStore();
