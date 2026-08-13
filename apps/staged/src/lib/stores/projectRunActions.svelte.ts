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
   * Hydrate initial state by querying running actions for all known branches.
   */
  private async hydrateFromBranches(branchIds: Iterable<string>, force: boolean): Promise<void> {
    const uniqueBranchIds = Array.from(new Set(branchIds));
    const branchIdsToHydrate = force
      ? uniqueBranchIds
      : uniqueBranchIds.filter((branchId) => !this.hydratedBranchIds.has(branchId));
    if (branchIdsToHydrate.length === 0) return;

    const results = await Promise.allSettled(
      branchIdsToHydrate.map(async (branchId) => {
        const actions = await getRunningBranchActions(branchId);
        return { branchId, actions };
      })
    );

    // Collect all run actions, then fetch their phases in parallel
    const runActions: { executionId: string; branchId: string }[] = [];
    for (const result of results) {
      if (result.status !== 'fulfilled') continue;
      this.hydratedBranchIds.add(result.value.branchId);
      for (const action of result.value.actions) {
        if (action.actionType !== 'run') continue;
        runActions.push({ executionId: action.executionId, branchId: action.branchId });
      }
    }

    const phaseResults = await Promise.allSettled(
      runActions.map(async ({ executionId, branchId }) => {
        let phase: RunPhase = { type: 'building' };
        try {
          const currentPhase = await getRunPhase(executionId);
          if (currentPhase) phase = currentPhase;
        } catch {
          // Use default building phase
        }
        return { executionId, branchId, phase };
      })
    );

    for (const result of phaseResults) {
      if (result.status !== 'fulfilled') continue;
      const { executionId, branchId, phase } = result.value;
      this.addExecution(executionId, branchId, phase);
    }
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
