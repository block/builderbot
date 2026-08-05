/**
 * ActionRunner — the shared action-runner state machine behind a card's
 * action surfaces (running pills, primary run button, Actions submenu,
 * output modal).
 *
 * Owns the configured action list, the live running-execution set (hydrated
 * via get_running_branch_actions, updated by action_status and
 * action:run-phase-changed events), stop/fade-out bookkeeping, and the output
 * modal state. The execution pipeline treats its routing id as an opaque
 * string, so the runner is parameterized by a scope id — a branch id, or the
 * synthetic repo scope id from repoActionScopeId() — plus loadActions/run
 * callbacks, letting branch cards and repo cards share one implementation.
 */

import { toast } from 'svelte-sonner';
import type { ProjectAction } from '../../api/commands';
import {
  clearActionExecution,
  getRunningBranchActions,
  getRunPhase,
  stopBranchAction,
  type ActionStatusEvent,
  type ActionType,
  type RunPhase,
} from './actions';
import { onBranchActionStatus, onBranchRunPhaseChanged } from '../../services/branchEventService';
import {
  getPrimaryActionExecution,
  getPrimaryRunAction,
  getRemainingRunActions,
  getSecondaryRunningActions,
  groupActionsByType,
} from './actionGroups';

export type RunningAction = {
  executionId: string;
  actionId: string;
  actionName: string;
  actionType: ActionType;
  status: 'running' | 'completed' | 'failed' | 'stopped';
  exitCode?: number | null;
  startedAt?: number;
  completedAt?: number | null;
  fading?: boolean;
};

export interface ActionOutputModalState {
  executionId: string;
  actionId: string;
  actionName: string;
  isStopping: boolean;
}

export interface ActionRunnerOptions {
  /**
   * Opaque id the scope's executions are routed under: a branch id, or the
   * synthetic repo scope id from repoActionScopeId(). Read lazily so
   * subscribe() re-tracks it when called inside an $effect.
   */
  getScopeId: () => string;
  /** Load the scope's configured actions (listProjectActions / listRepoActions). */
  loadActions: () => Promise<ProjectAction[]>;
  /** Start an action and return its execution id (runBranchAction / runRepoAction). */
  run: (actionId: string) => Promise<string>;
}

function notifyError(title: string, e: unknown): void {
  toast.error(title, {
    description: e instanceof Error ? e.message : String(e),
    duration: Infinity,
  });
}

export class ActionRunner {
  private getScopeId: () => string = undefined!;
  private load: () => Promise<ProjectAction[]> = undefined!;
  private run: (actionId: string) => Promise<string> = undefined!;

  actions = $state<ProjectAction[]>([]);
  runningActions = $state<RunningAction[]>([]);
  stoppingExecutions = $state<Set<string>>(new Set());

  // Run phase tracking for run actions (building, running, endpoint detection)
  runPhases = $state(new Map<string, RunPhase>());

  outputModal = $state<ActionOutputModalState | null>(null);

  groupedActions = $derived(groupActionsByType(this.actions));
  primaryRunAction = $derived(getPrimaryRunAction(this.groupedActions));
  remainingRunActions = $derived(getRemainingRunActions(this.groupedActions));
  primaryActionExecution = $derived(
    getPrimaryActionExecution(this.runningActions, this.primaryRunAction?.id ?? null)
  );
  secondaryRunningActions = $derived(
    getSecondaryRunningActions(this.runningActions, this.primaryRunAction?.id ?? null)
  );

  constructor(opts: ActionRunnerOptions) {
    this.getScopeId = opts.getScopeId;
    this.load = opts.loadActions;
    this.run = opts.run;
  }

  /**
   * Subscribe to status and run-phase events for the current scope id.
   * Call inside an $effect and return the unlisten so a scope-id change
   * re-subscribes.
   */
  subscribe(): () => void {
    const scopeId = this.getScopeId();

    const unlistenActionStatus = onBranchActionStatus(scopeId, (payload) =>
      this.applyStatusEvent(payload)
    );

    const unlistenRunPhaseChanged = onBranchRunPhaseChanged(scopeId, (event) => {
      this.runPhases.set(event.executionId, event.phase);
      this.runPhases = new Map(this.runPhases);
    });

    return () => {
      unlistenActionStatus();
      unlistenRunPhaseChanged();
    };
  }

  private applyStatusEvent(payload: ActionStatusEvent): void {
    const existingIndex = this.runningActions.findIndex(
      (a) => a.executionId === payload.executionId
    );

    if (payload.status === 'running') {
      if (existingIndex === -1) {
        this.runningActions.push({
          executionId: payload.executionId,
          actionId: payload.actionId,
          actionName: payload.actionName,
          actionType: payload.actionType,
          status: 'running',
          startedAt: payload.startedAt ?? Date.now(),
        });
      }
    } else {
      // Action completed/failed/stopped - update status
      if (existingIndex !== -1) {
        this.runningActions[existingIndex].status = payload.status;
        this.runningActions[existingIndex].exitCode = payload.exitCode;
        this.runningActions[existingIndex].completedAt = payload.completedAt;

        // Clean up stopping state and run phase when action reaches terminal state
        if (
          payload.status === 'stopped' ||
          payload.status === 'completed' ||
          payload.status === 'failed'
        ) {
          const updated = new Set(this.stoppingExecutions);
          updated.delete(payload.executionId);
          this.stoppingExecutions = updated;

          this.runPhases.delete(payload.executionId);
          this.runPhases = new Map(this.runPhases);
        }

        // Auto-remove terminal states after a delay
        const action = this.runningActions[existingIndex];
        const isPrimaryAction =
          this.primaryRunAction && action.actionId === this.primaryRunAction.id;

        // Determine delay based on status: completed shows briefly, stopped/failed show longer
        let displayTime: number;
        if (payload.status === 'completed') {
          displayTime = isPrimaryAction ? 1000 : 2000;
        } else {
          // stopped/failed: show status briefly then clean up so rerun works cleanly
          displayTime = isPrimaryAction ? 2000 : 3000;
        }

        setTimeout(() => {
          const foundAction = this.runningActions.find(
            (a) => a.executionId === payload.executionId
          );
          if (foundAction && !isPrimaryAction) {
            // Secondary actions fade out
            foundAction.fading = true;
          }
          // Remove after animation completes (or immediately for primary)
          setTimeout(
            () => {
              this.runningActions = this.runningActions.filter(
                (a) => a.executionId !== payload.executionId
              );
            },
            isPrimaryAction ? 0 : 300
          ); // Match CSS transition duration for secondary
        }, displayTime);
      }
    }
  }

  async loadActions(): Promise<void> {
    try {
      this.actions = await this.load();
    } catch (e) {
      console.error('Failed to load actions:', e);
      this.actions = [];
    }
  }

  async loadRunningActions(): Promise<void> {
    try {
      const running = await getRunningBranchActions(this.getScopeId());

      for (const info of running) {
        const existingIndex = this.runningActions.findIndex(
          (a) => a.executionId === info.executionId
        );
        if (existingIndex === -1) {
          this.runningActions.push({
            executionId: info.executionId,
            actionId: info.actionId,
            actionName: info.actionName,
            actionType: info.actionType,
            status: 'running',
            startedAt: info.startedAt,
          });
        }

        try {
          const phase = await getRunPhase(info.executionId);
          if (phase) {
            this.runPhases.set(info.executionId, phase);
          } else if (info.actionType === 'run') {
            this.runPhases.set(info.executionId, { type: 'running', endpoint: null });
          }
        } catch {
          // Phase not available for this execution
        }
      }
      this.runPhases = new Map(this.runPhases);
    } catch (e) {
      console.error('Failed to load running actions:', e);
    }
  }

  /** Drop terminal executions of an action and clear their output buffers. */
  private clearStaleExecutions(actionId: string): void {
    const staleExecutions = this.runningActions.filter(
      (a) => a.actionId === actionId && a.status !== 'running'
    );
    for (const stale of staleExecutions) {
      clearActionExecution(stale.executionId).catch(() => {});
    }
    this.runningActions = this.runningActions.filter(
      (a) => !(a.actionId === actionId && a.status !== 'running')
    );
  }

  /** Run an action, or open the output modal if it's already running. */
  async runAction(action: ProjectAction): Promise<void> {
    this.clearStaleExecutions(action.id);

    const existingExecution = this.runningActions.find(
      (a) => a.actionId === action.id && a.status === 'running'
    );

    if (existingExecution) {
      this.outputModal = {
        executionId: existingExecution.executionId,
        actionId: action.id,
        actionName: action.name,
        isStopping: this.stoppingExecutions.has(existingExecution.executionId),
      };
      return;
    }

    try {
      await this.run(action.id);
    } catch (e) {
      console.error('Failed to run action:', e);
      notifyError(`Failed to run action "${action.name}"`, e);
    }
  }

  async stopAction(executionId: string, actionName: string): Promise<void> {
    if (this.stoppingExecutions.has(executionId)) {
      return;
    }

    this.stoppingExecutions = new Set(this.stoppingExecutions).add(executionId);

    try {
      await stopBranchAction(executionId);
    } catch (e) {
      const updated = new Set(this.stoppingExecutions);
      updated.delete(executionId);
      this.stoppingExecutions = updated;
      console.error(`Failed to stop action ${actionName}:`, e);
      notifyError(`Failed to stop action "${actionName}"`, e);
    }
  }

  showOutput(execution: RunningAction): void {
    this.outputModal = {
      executionId: execution.executionId,
      actionId: execution.actionId,
      actionName: execution.actionName,
      isStopping: this.stoppingExecutions.has(execution.executionId),
    };
  }

  /** Re-run the output modal's action, keeping the modal open on the new execution. */
  async runAgain(): Promise<void> {
    const action = this.actions.find((a) => a.id === this.outputModal?.actionId);
    if (!action) return;

    this.clearStaleExecutions(action.id);

    // If already running, just switch the modal to that execution
    const existingExecution = this.runningActions.find(
      (a) => a.actionId === action.id && a.status === 'running'
    );
    if (existingExecution) {
      this.outputModal = {
        executionId: existingExecution.executionId,
        actionId: action.id,
        actionName: action.name,
        isStopping: this.stoppingExecutions.has(existingExecution.executionId),
      };
      return;
    }

    try {
      const newExecutionId = await this.run(action.id);
      // Keep the modal open and switch to the new execution
      this.outputModal = {
        executionId: newExecutionId,
        actionId: action.id,
        actionName: action.name,
        isStopping: false,
      };
    } catch (e) {
      console.error('Failed to run action:', e);
      notifyError(`Failed to run action "${action.name}"`, e);
    }
  }

  closeOutputModal(): void {
    this.outputModal = null;
  }
}
