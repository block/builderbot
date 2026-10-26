/**
 * Project-level run-action state store.
 *
 * Tracks which projects have running "run" actions and their current phase
 * (building vs running). Listens globally to action_status and
 * action:run-phase-changed Tauri events and aggregates state per project
 * using a branchId → projectId lookup map.
 */

import type { UnlistenFn } from '../transport';
import type { Branch } from '../types';
import {
  type ActionStatusEvent,
  type RunPhase,
  type RunPhaseChangedEvent,
  getRunningBranchActions,
  getRunPhase,
  listenToActionStatus,
  listenToRunPhaseChanged,
} from '../features/actions/actions';

export type RunActionPhase = 'building' | 'running' | null;

interface ExecutionInfo {
  branchId: string;
  phase: RunPhase;
}

class ProjectRunActionsStore {
  /** executionId → execution info (branchId + phase) */
  private executions = $state<Map<string, ExecutionInfo>>(new Map());

  /** branchId → projectId lookup */
  private branchToProject = $state<Map<string, string>>(new Map());

  /** Manual reactivity version counter */
  private version = $state(0);

  /** Branches whose current running-action state has already been queried. */
  private hydratedBranchIds = new Set<string>();

  /**
   * branchId → the request currently fetching that branch's state. Recorded
   * before the request is awaited so that overlapping hydrate calls (every
   * project's branch fetch reassigns the branch map during startup) await the
   * outstanding request instead of issuing another one for the same branch.
   */
  private pendingBranchRequests = new Map<string, Promise<void>>();

  private unlisteners: UnlistenFn[] = [];
  private initialized = false;

  /**
   * Start listening to global Tauri events.
   * Called once from App.svelte — the state feeds the shared project filters,
   * which every route renders via the sidebar or the landing grid.
   */
  startListening(): void {
    if (this.initialized) return;
    this.initialized = true;

    const unlistenStatus = listenToActionStatus((event: ActionStatusEvent) => {
      if (event.actionType !== 'run') return;

      if (event.status === 'running') {
        // Only add if not already tracked — avoids regressing an execution
        // whose phase-changed event arrived before the action_status event.
        if (!this.executions.has(event.executionId)) {
          this.addExecution(event.executionId, event.branchId, { type: 'building' });
        }
      } else {
        // completed, failed, stopped — remove
        this.removeExecution(event.executionId);
      }
    });

    const unlistenPhase = listenToRunPhaseChanged((event: RunPhaseChangedEvent) => {
      const { executionId, branchId, phase } = event;
      // Always use addExecution to replace the map entry and bump version
      // consistently — avoids relying on direct mutation of $state internals.
      this.addExecution(executionId, branchId, phase);
    });

    this.unlisteners.push(unlistenStatus, unlistenPhase);
  }

  /**
   * Stop listening and reset state. Called on app teardown.
   */
  stopListening(): void {
    for (const unlisten of this.unlisteners) {
      unlisten();
    }
    this.unlisteners = [];
    this.executions = new Map();
    this.branchToProject = new Map();
    this.hydratedBranchIds.clear();
    // Requests still in flight no longer match their map entry and discard
    // their results on arrival.
    this.pendingBranchRequests.clear();
    this.initialized = false;
    this.version++;
  }

  /**
   * Update the branchId → projectId lookup map.
   * Should be called whenever projectBranches changes.
   */
  updateBranchProjectMap(projectBranches: Map<string, string[]>): void {
    const map = new Map<string, string>();
    for (const [projectId, branchIds] of projectBranches) {
      for (const branchId of branchIds) {
        map.set(branchId, projectId);
      }
    }
    this.branchToProject = map;
  }

  /**
   * Update the branch→project map and hydrate run-action state from a
   * project→branches map. Convenience wrapper the project surfaces
   * (ProjectsList, ProjectHome, ProjectsSidebar) run against the shared
   * branch data; already-queried branches are skipped unless forced.
   */
  async hydrateFromProjectBranches(
    branchesByProject: Map<string, Branch[]>,
    options: { branchIds?: Iterable<string>; force?: boolean } = {}
  ): Promise<void> {
    const branchProjectMap = new Map<string, string[]>();
    const allBranchIds: string[] = [];
    for (const [projectId, branches] of branchesByProject) {
      branchProjectMap.set(
        projectId,
        branches.map((b) => b.id)
      );
      for (const b of branches) {
        allBranchIds.push(b.id);
      }
    }
    this.updateBranchProjectMap(branchProjectMap);
    await this.hydrateFromBranches(options.branchIds ?? allBranchIds, options.force ?? false);
  }

  /**
   * Hydrate state by querying running actions for the given branches, one
   * request per branch. Branches already hydrated are skipped and branches
   * with a request in flight are awaited rather than re-requested, unless
   * `force` is set, in which case a fresh request is issued for every branch
   * and supersedes any outstanding one.
   */
  private async hydrateFromBranches(branchIds: Iterable<string>, force: boolean): Promise<void> {
    const requests: Promise<void>[] = [];
    for (const branchId of new Set(branchIds)) {
      if (!force) {
        if (this.hydratedBranchIds.has(branchId)) continue;
        const pending = this.pendingBranchRequests.get(branchId);
        if (pending) {
          requests.push(pending);
          continue;
        }
      }
      requests.push(this.hydrateBranch(branchId));
    }
    if (requests.length === 0) return;
    await Promise.allSettled(requests);
  }

  /**
   * Fetch one branch's running actions and their run phases, then apply them.
   * The request is tracked in `pendingBranchRequests` until it settles; only
   * the request that still owns the entry when its data arrives applies it,
   * so a forced refresh or `stopListening` makes an older request a no-op. A
   * rejected request leaves the branch unhydrated so a later call retries it.
   */
  private hydrateBranch(branchId: string): Promise<void> {
    const run = async (): Promise<void> => {
      const actions = await getRunningBranchActions(branchId);
      const runActions = actions.filter((action) => action.actionType === 'run');
      const phases = await Promise.all(
        runActions.map(async (action) => ({
          executionId: action.executionId,
          branchId: action.branchId,
          phase: await this.fetchRunPhase(action.executionId),
        }))
      );
      if (this.pendingBranchRequests.get(branchId) !== request) return;
      this.hydratedBranchIds.add(branchId);
      for (const { executionId, branchId: actionBranchId, phase } of phases) {
        this.addExecution(executionId, actionBranchId, phase);
      }
    };
    const request: Promise<void> = run().finally(() => {
      if (this.pendingBranchRequests.get(branchId) === request) {
        this.pendingBranchRequests.delete(branchId);
      }
    });
    this.pendingBranchRequests.set(branchId, request);
    return request;
  }

  private async fetchRunPhase(executionId: string): Promise<RunPhase> {
    try {
      const currentPhase = await getRunPhase(executionId);
      if (currentPhase) return currentPhase;
    } catch {
      // Fall through to the default building phase
    }
    return { type: 'building' };
  }

  /**
   * Get the aggregated run-action phase for a project.
   * Returns 'running' if any action is past the building phase
   * (running, autodetectPending, noDetection — all shown as the wave
   * animation), 'building' if any is still building, null otherwise.
   */
  getRunActionPhase(projectId: string): RunActionPhase {
    // Access version for reactivity
    this.version;

    let hasBuilding = false;
    for (const [, exec] of this.executions) {
      const execProjectId = this.branchToProject.get(exec.branchId);
      if (execProjectId !== projectId) continue;

      if (exec.phase.type === 'building') {
        hasBuilding = true;
      } else {
        // Any non-building phase (running, autodetectPending, noDetection)
        // means the app is up or detection is pending — show wave animation
        return 'running';
      }
    }

    return hasBuilding ? 'building' : null;
  }

  /**
   * Check if a project has any running run-actions.
   */
  hasRunningRunActions(projectId: string): boolean {
    return this.getRunActionPhase(projectId) !== null;
  }

  private addExecution(executionId: string, branchId: string, phase: RunPhase): void {
    this.executions.set(executionId, { branchId, phase });
    this.version++;
  }

  private removeExecution(executionId: string): void {
    if (this.executions.delete(executionId)) {
      this.version++;
    }
  }
}

export const projectRunActionsStore = new ProjectRunActionsStore();
