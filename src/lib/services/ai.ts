/**
 * AI service for smart diff analysis.
 */

import { invoke } from '@tauri-apps/api/core';
import type { SmartDiffResult, ChangesetSummary, ChangesetAnalysis, DiffSpec } from '../types';

/**
 * Check if an AI CLI tool is available.
 *
 * @returns The name of the available tool ('goose' or 'claude')
 * @throws Error if no AI tool is found
 */
export async function checkAiAvailable(): Promise<string> {
  return invoke<string>('check_ai_available');
}

/**
 * Analyze a diff using AI.
 *
 * The backend handles file listing and content loading - frontend just provides
 * the diff spec. Returns summary, key changes, concerns, and per-file annotations.
 *
 * @param repoPath - Path to the repository (null for current directory)
 * @param spec - The diff specification (base..head)
 * @returns ChangesetAnalysis with summary, key changes, concerns, and per-file annotations
 */
export async function analyzeDiff(
  repoPath: string | null,
  spec: DiffSpec
): Promise<ChangesetAnalysis> {
  return invoke<ChangesetAnalysis>('analyze_diff', { repoPath, spec });
}

// =============================================================================
// AI Analysis Persistence
// =============================================================================

/**
 * Save a changeset summary to the database.
 */
export async function saveChangesetSummary(
  repoPath: string | null,
  spec: DiffSpec,
  summary: ChangesetSummary
): Promise<void> {
  return invoke('save_changeset_summary', { repoPath, spec, summary });
}

/**
 * Get a saved changeset summary from the database.
 */
export async function getChangesetSummary(
  repoPath: string | null,
  spec: DiffSpec
): Promise<ChangesetSummary | null> {
  return invoke<ChangesetSummary | null>('get_changeset_summary', { repoPath, spec });
}

/**
 * Save a file analysis to the database.
 */
export async function saveFileAnalysis(
  repoPath: string | null,
  spec: DiffSpec,
  filePath: string,
  result: SmartDiffResult
): Promise<void> {
  return invoke('save_file_analysis', { repoPath, spec, filePath, result });
}

/**
 * Get all saved file analyses for a diff.
 */
export async function getAllFileAnalyses(
  repoPath: string | null,
  spec: DiffSpec
): Promise<Array<[string, SmartDiffResult]>> {
  return invoke<Array<[string, SmartDiffResult]>>('get_all_file_analyses', { repoPath, spec });
}

/**
 * Delete all AI analyses for a diff (used when refreshing).
 */
export async function deleteAllAnalyses(repoPath: string | null, spec: DiffSpec): Promise<void> {
  return invoke('delete_all_analyses', { repoPath, spec });
}

// =============================================================================
// Agent Chat
// =============================================================================

/**
 * Response from the AI agent including session ID for continuity.
 */
export interface AgentPromptResponse {
  response: string;
  sessionId: string;
}

/**
 * Send a prompt to the AI agent and get a response.
 * Supports session continuity by accepting and returning a session ID.
 *
 * @param repoPath - Path to the repository (null for current directory)
 * @param prompt - The prompt to send to the agent
 * @param sessionId - Optional session ID to resume an existing session
 * @returns The agent's response and session ID for future resumption
 */
export async function sendAgentPrompt(
  repoPath: string | null,
  prompt: string,
  sessionId?: string | null
): Promise<AgentPromptResponse> {
  return invoke<AgentPromptResponse>('send_agent_prompt', {
    repoPath,
    prompt,
    sessionId: sessionId ?? null,
  });
}
