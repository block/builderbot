/**
 * Project actions service.
 *
 * Wraps Tauri commands for action detection, execution, and management.
 * Actions are project-specific commands (build, test, format, lint, etc.)
 * that can be run in branch worktrees with real-time output streaming.
 */

import { invokeCommand, listenToEvent, type UnlistenFn } from '../../transport';

/** Action types available for project actions. */
export type ActionType = 'build' | 'test' | 'format' | 'check' | 'prerun' | 'run' | 'cleanUp';

/** Status of a running action. */
export type ActionStatus = 'running' | 'completed' | 'failed' | 'stopped';

/** How a "run" action detects whether the process is running and/or its endpoint. */
export type RunDetectionMode =
  | { type: 'autodetect' }
  | { type: 'endpointRegex'; pattern: string }
  | { type: 'runningRegex'; pattern: string }
  | { type: 'noDetection' };

/** Current phase of a running "run" action. */
export type RunPhase =
  | { type: 'building' }
  | { type: 'running'; endpoint: string | null }
  | { type: 'autodetectPending' }
  | { type: 'noDetection' };

/** Event payload for run phase changes. */
export interface RunPhaseChangedEvent {
  executionId: string;
  branchId: string;
  actionName: string;
  phase: RunPhase;
}

/** A configured project action. */
export interface ProjectAction {
  id: string;
  contextId: string;
  name: string;
  command: string;
  actionType: ActionType;
  sortOrder: number;
  autoCommit: boolean;
  runDetectionMode?: RunDetectionMode;
  /** Whether the action gets its own button in a card header. */
  pinned: boolean;
  /** Kebab-case Lucide icon name, or null for the action type's default. */
  icon: string | null;
  createdAt: number;
  updatedAt: number;
}

/** A chunk of output from a running action. */
export interface OutputChunk {
  chunk: string;
  stream: 'stdout' | 'stderr';
  timestamp: number;
}

/** Event payload for action output. */
export interface ActionOutputEvent {
  executionId: string;
  chunk: string;
  stream: 'stdout' | 'stderr';
}

/** Event payload for action status changes. */
export interface ActionStatusEvent {
  executionId: string;
  branchId: string;
  actionId: string;
  actionName: string;
  actionType: ActionType;
  status: ActionStatus;
  exitCode?: number;
  startedAt?: number;
  completedAt?: number;
}

/** Event payload for auto-commit notifications. */
export interface ActionAutoCommitEvent {
  executionId: string;
  branchId: string;
  actionName: string;
}

/** Event payload for repo action detection status (header badge). */
export interface RepoActionsDetectionEvent {
  githubRepo: string;
  subpath: string | null;
  detecting: boolean;
}

/** Information about a running action. */
export interface RunningActionInfo {
  executionId: string;
  branchId: string;
  actionId: string;
  actionName: string;
  actionType: ActionType;
  startedAt: number;
}

/**
 * A running action with its run phase attached, as returned by the bulk
 * `get_all_running_actions` query — the inline phase is what lets a caller skip
 * the per-execution `get_run_phase` round trip.
 */
export interface RunningActionSnapshot extends RunningActionInfo {
  phase: RunPhase | null;
}

/**
 * Detect available actions for a repo+subpath context and persist the new
 * suggestions, resolving to the context's resulting action list.
 *
 * The backend persists inside the window it reports as detecting, so callers
 * have nothing to write themselves — and the `detecting: false` broadcast every
 * action surface listens for means the list is already final.
 */
export function detectRepoActions(
  githubRepo: string,
  subpath?: string,
  provider?: string
): Promise<ProjectAction[]> {
  return invokeCommand<ProjectAction[]>('detect_repo_actions', {
    githubRepo,
    subpath: subpath ?? null,
    provider: provider ?? null,
  });
}

/**
 * Run an action for a branch.
 * Returns an execution ID that can be used to track status and stop the action.
 */
export function runBranchAction(
  branchId: string,
  actionId: string,
  provider?: string
): Promise<string> {
  return invokeCommand<string>('run_branch_action', { branchId, actionId, provider });
}

/**
 * Stop a running action by execution ID.
 */
export function stopBranchAction(executionId: string): Promise<void> {
  return invokeCommand<void>('stop_branch_action', { executionId });
}

/**
 * Stop a running action with state management.
 * Handles stopping flag to prevent duplicate requests and provides error handling.
 *
 * @param executionId - The execution ID of the action to stop
 * @param stoppingSet - A Set to track which executions are currently stopping
 * @param onError - Optional callback for error handling
 * @returns Promise that resolves when the stop request completes
 */
export async function stopBranchActionWithState(
  executionId: string,
  stoppingSet: Set<string>,
  onError?: (error: Error) => void
): Promise<void> {
  // Prevent duplicate stop requests
  if (stoppingSet.has(executionId)) {
    return;
  }

  stoppingSet.add(executionId);
  try {
    await stopBranchAction(executionId);
    // Backend will emit 'stopped' status event
  } catch (e) {
    // Remove from stopping set on error so user can retry
    stoppingSet.delete(executionId);
    const error = e instanceof Error ? e : new Error(String(e));
    if (onError) {
      onError(error);
    } else {
      throw error;
    }
  }
}

/**
 * Get all currently running actions for a branch.
 * Returns an array of running action info with execution IDs, action details, and timestamps.
 */
export function getRunningBranchActions(branchId: string): Promise<RunningActionInfo[]> {
  return invokeCommand<RunningActionInfo[]>('get_running_branch_actions', { branchId });
}

/**
 * Get buffered output for an action execution.
 * Useful for retrieving output history when joining an already-running action.
 */
export function getActionOutputBuffer(executionId: string): Promise<OutputChunk[] | null> {
  return invokeCommand<OutputChunk[] | null>('get_action_output_buffer', { executionId });
}

/**
 * Clear buffered output for a completed action execution.
 * Returns true if the execution was found and cleared, false otherwise.
 */
export function clearActionExecution(executionId: string): Promise<boolean> {
  return invokeCommand<boolean>('clear_action_execution', { executionId });
}

/**
 * Run all prerun actions for a branch after creation.
 * Returns an array of execution IDs for the started actions.
 */
export function runPrerunActions(branchId: string, provider?: string): Promise<string[]> {
  return invokeCommand<string[]>('run_prerun_actions', { branchId, provider });
}

/**
 * Listen for real-time action output events.
 * Returns an unlisten function to stop listening.
 */
export function listenToActionOutput(callback: (event: ActionOutputEvent) => void): UnlistenFn {
  return listenToEvent<ActionOutputEvent>('action_output', callback);
}

/**
 * Listen for action status change events.
 * Returns an unlisten function to stop listening.
 */
export function listenToActionStatus(callback: (event: ActionStatusEvent) => void): UnlistenFn {
  return listenToEvent<ActionStatusEvent>('action_status', callback);
}

/**
 * Listen for action auto-commit events.
 * Returns an unlisten function to stop listening.
 */
export function listenToActionAutoCommit(
  callback: (event: ActionAutoCommitEvent) => void
): UnlistenFn {
  return listenToEvent<ActionAutoCommitEvent>('action_auto_commit', callback);
}

/** Listen for repo action detection start/stop updates. */
export function listenToRepoActionsDetection(
  callback: (event: RepoActionsDetectionEvent) => void
): UnlistenFn {
  return listenToEvent<RepoActionsDetectionEvent>('repo-actions-detection', callback);
}

/**
 * Get the current run phase for a running action execution.
 * Returns null if the execution is not found or has no phase.
 */
export function getRunPhase(executionId: string): Promise<RunPhase | null> {
  return invokeCommand<RunPhase | null>('get_run_phase', { executionId });
}

/**
 * Update the run detection mode for an action.
 */
export function updateRunDetectionMode(actionId: string, mode: RunDetectionMode): Promise<void> {
  return invokeCommand('update_run_detection_mode', { actionId, mode });
}

/**
 * Listen for run phase change events.
 * Returns an unlisten function to stop listening.
 */
export function listenToRunPhaseChanged(
  callback: (event: RunPhaseChangedEvent) => void
): UnlistenFn {
  return listenToEvent<RunPhaseChangedEvent>('action:run-phase-changed', callback);
}
