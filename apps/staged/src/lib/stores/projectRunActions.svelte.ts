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
  listenToActionStatus,
  listenToRunPhaseChanged,
} from '../features/actions/actions';
import { bulkRunningSnapshot } from '../features/actions/repoActionsBulk';

export type RunActionPhase = 'building' | 'running' | null;

interface ExecutionInfo {
  branchId: string;
  phase: RunPhase;
}

/** The in-flight snapshot query; the object's identity is its ownership token. */
interface PendingSnapshotRequest {
  /** Settles when the query does, whether or not its result was applied. */
  promise: Promise<void>;
}

class ProjectRunActionsStore {
  /** executionId → execution info (branchId + phase) */
  private executions = $state<Map<string, ExecutionInfo>>(new Map());

  /** branchId → projectId lookup */
  private branchToProject = $state<Map<string, string>>(new Map());

  /** Manual reactivity version counter */
  private version = $state(0);

  /**
   * Whether a running-actions snapshot has been applied since the last
   * (re)start. `get_all_running_actions` covers every scope, so one snapshot
   * hydrates every branch for the store's lifetime: a run that starts after
   * it arrives through the action_status listener, and a run already live on
   * a branch whose project loads later is in the snapshot already, keyed by
   * branch id, and surfaces once the branch→project map learns the branch.
   */
  private hydrated = false;

  /**
   * The snapshot request in flight, if any. Recorded before the request is
   * awaited so overlapping hydrate calls (every project's branch fetch
   * reassigns the branch map during startup) await the outstanding request
   * instead of issuing another one.
   */
  private snapshotRequest: PendingSnapshotRequest | null = null;

  /**
   * Sorted `projectId/branchId` signature of the last map handed to
   * `hydrateFromProjectBranches`. The data store reassigns `branchesByProject`
   * once per project while loading, so the surfaces' effects re-run with the
   * same membership many times; an unchanged signature skips the whole call.
   * Cleared when something leaves a branch unhydrated so the next call
   * re-enters.
   */
  private lastMembershipSignature: string | null = null;

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
    this.hydrated = false;
    // A request still in flight no longer matches the token and discards its
    // result on arrival.
    this.snapshotRequest = null;
    this.lastMembershipSignature = null;
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
   * branch data. A call whose project/branch membership matches the previous
   * one returns immediately; otherwise it resolves once the running-actions
   * snapshot has been applied (at most one request, shared by every caller).
   */
  async hydrateFromProjectBranches(branchesByProject: Map<string, Branch[]>): Promise<void> {
    const branchProjectMap = new Map<string, string[]>();
    const keys: string[] = [];
    for (const [projectId, branches] of branchesByProject) {
      branchProjectMap.set(
        projectId,
        branches.map((b) => b.id)
      );
      for (const b of branches) {
        keys.push(`${projectId}/${b.id}`);
      }
    }
    const signature = keys.sort().join('\n');
    if (signature === this.lastMembershipSignature) return;
    this.lastMembershipSignature = signature;
    this.updateBranchProjectMap(branchProjectMap);
    // A failed request has already cleared the membership signature so the
    // next call retries; callers only need to know the attempt has settled.
    await this.hydrateSnapshot().catch(() => {});
  }

  /**
   * Fetch the running-actions snapshot once and apply it. The entry is
   * recorded in `snapshotRequest` before the fetch starts and stays until it
   * settles; only the request that still owns the entry when its data
   * arrives applies it, so `stopListening` makes an older request a no-op. A
   * rejected request leaves the store unhydrated and clears the membership
   * signature so a later call retries.
   */
  private hydrateSnapshot(): Promise<void> {
    if (this.hydrated) return Promise.resolve();
    if (this.snapshotRequest) return this.snapshotRequest.promise;
    // `promise` is assigned synchronously below, before any caller can read
    // the entry; the object itself is the ownership token the fetch checks.
    const request = {} as PendingSnapshotRequest;
    this.snapshotRequest = request;
    request.promise = this.fetchSnapshot(request)
      .catch((error: unknown) => {
        this.lastMembershipSignature = null;
        throw error;
      })
      .finally(() => {
        if (this.snapshotRequest === request) {
          this.snapshotRequest = null;
        }
      });
    return request.promise;
  }

  private async fetchSnapshot(request: PendingSnapshotRequest): Promise<void> {
    const snapshots = await bulkRunningSnapshot();
    if (this.snapshotRequest !== request) return;
    this.hydrated = true;
    for (const snapshot of snapshots) {
      if (snapshot.actionType !== 'run') continue;
      // A snapshot without a phase is an execution the backend has started
      // but not yet classified; it shows as building until a phase event.
      this.addExecution(
        snapshot.executionId,
        snapshot.branchId,
        snapshot.phase ?? {
          type: 'building',
        }
      );
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
