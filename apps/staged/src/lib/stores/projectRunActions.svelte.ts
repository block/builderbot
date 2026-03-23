/**
 * Project-level run-action state store.
 *
 * Tracks which projects have running "run" actions and their current phase
 * (building vs running). Listens globally to action_status and
 * action:run-phase-changed Tauri events and aggregates state per project
 * using a branchId → projectId lookup map.
 */

import type { UnlistenFn } from '@tauri-apps/api/event';
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

  private unlisteners: UnlistenFn[] = [];
  private initialized = false;

  /**
   * Start listening to global Tauri events.
   * Call once when ProjectsList mounts.
   */
  async startListening(): Promise<void> {
    if (this.initialized) return;
    this.initialized = true;

    const unlistenStatus = await listenToActionStatus((event: ActionStatusEvent) => {
      if (event.actionType !== 'run') return;

      if (event.status === 'running') {
        this.addExecution(event.executionId, event.branchId, { type: 'building' });
      } else {
        // completed, failed, stopped — remove
        this.removeExecution(event.executionId);
      }
    });

    const unlistenPhase = await listenToRunPhaseChanged((event: RunPhaseChangedEvent) => {
      const { executionId, branchId, phase } = event;
      const existing = this.executions.get(executionId);
      if (existing) {
        existing.phase = phase;
        this.version++;
      } else {
        // Might arrive before the status event — add it
        this.addExecution(executionId, branchId, phase);
      }
    });

    this.unlisteners.push(unlistenStatus, unlistenPhase);
  }

  /**
   * Stop listening and reset state. Call on cleanup.
   */
  stopListening(): void {
    for (const unlisten of this.unlisteners) {
      unlisten();
    }
    this.unlisteners = [];
    this.executions = new Map();
    this.branchToProject = new Map();
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
   * Hydrate initial state by querying running actions for all known branches.
   */
  async hydrateFromBranches(branchIds: string[]): Promise<void> {
    const results = await Promise.allSettled(
      branchIds.map(async (branchId) => {
        const actions = await getRunningBranchActions(branchId);
        return { branchId, actions };
      })
    );

    for (const result of results) {
      if (result.status !== 'fulfilled') continue;
      const { actions } = result.value;
      for (const action of actions) {
        if (action.actionType !== 'run') continue;
        // Get the current phase for this execution
        let phase: RunPhase = { type: 'building' };
        try {
          const currentPhase = await getRunPhase(action.executionId);
          if (currentPhase) phase = currentPhase;
        } catch {
          // Use default building phase
        }
        this.addExecution(action.executionId, action.branchId, phase);
      }
    }
  }

  /**
   * Get the aggregated run-action phase for a project.
   * Returns 'running' if any action is in running phase,
   * 'building' if any is building, null otherwise.
   */
  getRunActionPhase(projectId: string): RunActionPhase {
    // Access version for reactivity
    this.version;

    let hasBuilding = false;
    for (const [, exec] of this.executions) {
      const execProjectId = this.branchToProject.get(exec.branchId);
      if (execProjectId !== projectId) continue;

      if (exec.phase.type === 'running') return 'running';
      if (exec.phase.type === 'building') hasBuilding = true;
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
