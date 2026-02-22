/**
 * Project actions service.
 *
 * Wraps Tauri commands for action detection, execution, and management.
 * Actions are project-specific commands (build, test, format, lint, etc.)
 * that can be run in branch worktrees with real-time output streaming.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** Action types available for project actions. */
export type ActionType = 'build' | 'test' | 'format' | 'check' | 'prerun' | 'run' | 'cleanUp';

/** Status of a running action. */
export type ActionStatus = 'running' | 'completed' | 'failed' | 'stopped';

/** A configured project action. */
export interface ProjectAction {
  id: string;
  contextId: string;
  name: string;
  command: string;
  actionType: ActionType;
  sortOrder: number;
  autoCommit: boolean;
  createdAt: number;
  updatedAt: number;
}

/** An action suggested by AI detection. */
export interface SuggestedAction {
  name: string;
  command: string;
  actionType: ActionType;
  autoCommit: boolean;
  source: string; // e.g., "package.json", "justfile"
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
  startedAt: number;
}

/** Detect available actions for a repo+subpath context. */
export function detectRepoActions(
  githubRepo: string,
  subpath?: string
): Promise<SuggestedAction[]> {
  return invoke<SuggestedAction[]>('detect_repo_actions', { githubRepo, subpath: subpath ?? null });
}

/**
 * Run an action for a branch.
 * Returns an execution ID that can be used to track status and stop the action.
 */
export function runBranchAction(branchId: string, actionId: string): Promise<string> {
  return invoke<string>('run_branch_action', { branchId, actionId });
}

/**
 * Stop a running action by execution ID.
 */
export function stopBranchAction(executionId: string): Promise<void> {
  return invoke<void>('stop_branch_action', { executionId });
}

/**
 * Get all currently running actions for a branch.
 * Returns an array of running action info with execution IDs, action details, and timestamps.
 */
export function getRunningBranchActions(branchId: string): Promise<RunningActionInfo[]> {
  return invoke<RunningActionInfo[]>('get_running_branch_actions', { branchId });
}

/**
 * Get buffered output for an action execution.
 * Useful for retrieving output history when joining an already-running action.
 */
export function getActionOutputBuffer(executionId: string): Promise<OutputChunk[] | null> {
  return invoke<OutputChunk[] | null>('get_action_output_buffer', { executionId });
}

/**
 * Clear buffered output for a completed action execution.
 * Returns true if the execution was found and cleared, false otherwise.
 */
export function clearActionExecution(executionId: string): Promise<boolean> {
  return invoke<boolean>('clear_action_execution', { executionId });
}

/**
 * Run all prerun actions for a branch after creation.
 * Returns an array of execution IDs for the started actions.
 */
export function runPrerunActions(branchId: string): Promise<string[]> {
  return invoke<string[]>('run_prerun_actions', { branchId });
}

/**
 * Listen for real-time action output events.
 * Returns an unlisten function to stop listening.
 */
export function listenToActionOutput(
  callback: (event: ActionOutputEvent) => void
): Promise<UnlistenFn> {
  return listen<ActionOutputEvent>('action_output', (event) => {
    callback(event.payload);
  });
}

/**
 * Listen for action status change events.
 * Returns an unlisten function to stop listening.
 */
export function listenToActionStatus(
  callback: (event: ActionStatusEvent) => void
): Promise<UnlistenFn> {
  return listen<ActionStatusEvent>('action_status', (event) => {
    callback(event.payload);
  });
}

/**
 * Listen for action auto-commit events.
 * Returns an unlisten function to stop listening.
 */
export function listenToActionAutoCommit(
  callback: (event: ActionAutoCommitEvent) => void
): Promise<UnlistenFn> {
  return listen<ActionAutoCommitEvent>('action_auto_commit', (event) => {
    callback(event.payload);
  });
}

/** Listen for repo action detection start/stop updates. */
export function listenToRepoActionsDetection(
  callback: (event: RepoActionsDetectionEvent) => void
): Promise<UnlistenFn> {
  return listen<RepoActionsDetectionEvent>('repo-actions-detection', (event) => {
    callback(event.payload);
  });
}
