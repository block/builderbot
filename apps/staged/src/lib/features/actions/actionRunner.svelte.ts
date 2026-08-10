/**
 * ActionRunner — the shared action-runner state machine behind a card's
 * action surfaces (running pills, primary run button, Actions submenu,
 * output modal).
 *
 * Owns the configured action list, the live running-execution set (hydrated
 * via get_running_branch_actions or an injected bulk loader, updated by
 * action_status and action:run-phase-changed events), stop/fade-out
 * bookkeeping, and the output modal state. The execution pipeline treats its
 * routing id as an opaque string, so the runner is parameterized by a scope id
 * — a branch id, or the synthetic repo scope id from repoActionScopeId() —
 * plus loadActions/run callbacks, letting branch cards and repo cards share
 * one implementation.
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
  type RunningActionInfo,
  type RunningActionSnapshot,
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
  /**
   * Optional bulk source for the scope's live executions, each with its run
   * phase inline. When given, hydration uses it instead of
   * get_running_branch_actions plus a get_run_phase per execution — which is
   * how a surface full of cards hydrates from a single call.
   */
  loadRunning?: () => Promise<RunningActionSnapshot[]>;
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
  private loadRunning: (() => Promise<RunningActionSnapshot[]>) | undefined;

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
    this.loadRunning = opts.loadRunning;
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

  /**
   * Load the scope's configured actions. Returns whether the read succeeded so
   * callers can tell "this context has no actions" from "the read failed" —
   * a failure also empties the list, and surfaces that offer an empty-context
   * affordance would otherwise show it after a transient IPC error.
   */
  async loadActions(): Promise<boolean> {
    try {
      this.actions = await this.load();
      return true;
    } catch (e) {
      console.error('Failed to load actions:', e);
      this.actions = [];
      return false;
    }
  }

  /**
   * Adopt an action list the caller already has in hand, skipping a reload —
   * detect_repo_actions returns the context's actions once it has persisted
   * them, so the Detect Actions flow needs no second read.
   */
  setActions(actions: ProjectAction[]): void {
    this.actions = actions;
  }

  /** Add a hydrated execution to the live set unless it's already tracked. */
  private trackHydratedExecution(info: RunningActionInfo): void {
    if (this.runningActions.some((a) => a.executionId === info.executionId)) return;
    this.runningActions.push({
      executionId: info.executionId,
      actionId: info.actionId,
      actionName: info.actionName,
      actionType: info.actionType,
      status: 'running',
      startedAt: info.startedAt,
    });
  }

  /** Record a hydrated run phase, defaulting run actions to a phaseless run. */
  private applyHydratedPhase(info: RunningActionInfo, phase: RunPhase | null): void {
    if (phase) {
      this.runPhases.set(info.executionId, phase);
    } else if (info.actionType === 'run') {
      this.runPhases.set(info.executionId, { type: 'running', endpoint: null });
    }
  }

  async loadRunningActions(): Promise<void> {
    try {
      if (this.loadRunning) {
        // Bulk path: phases arrive inline, so there are no per-execution calls.
        for (const snapshot of await this.loadRunning()) {
          this.trackHydratedExecution(snapshot);
          this.applyHydratedPhase(snapshot, snapshot.phase);
        }
      } else {
        for (const info of await getRunningBranchActions(this.getScopeId())) {
          this.trackHydratedExecution(info);
          try {
            this.applyHydratedPhase(info, await getRunPhase(info.executionId));
          } catch {
            // Phase not available for this execution
          }
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

  /**
   * Start an action, or focus the output modal on it when it's already
   * running. `keepModal` is the only difference between the two entry points:
   * a fresh start from the run button leaves the modal alone, while Run Again
   * repoints the open modal at the new execution.
   */
  private async startOrFocus(
    action: ProjectAction,
    { keepModal }: { keepModal: boolean }
  ): Promise<void> {
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
      const executionId = await this.run(action.id);
      if (keepModal) {
        this.outputModal = {
          executionId,
          actionId: action.id,
          actionName: action.name,
          isStopping: false,
        };
      }
    } catch (e) {
      console.error('Failed to run action:', e);
      notifyError(`Failed to run action "${action.name}"`, e);
    }
  }

  /** Run an action, or open the output modal if it's already running. */
  async runAction(action: ProjectAction): Promise<void> {
    await this.startOrFocus(action, { keepModal: false });
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
    await this.startOrFocus(action, { keepModal: true });
  }

  closeOutputModal(): void {
    this.outputModal = null;
  }
}
