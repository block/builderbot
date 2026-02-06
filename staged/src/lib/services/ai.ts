import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  SmartDiffResult,
  ChangesetSummary,
  ChangesetAnalysis,
  DiffSpec,
  SmartDiffAnnotation,
  Comment,
} from '../types';

// =============================================================================
// Types - Sessions
// =============================================================================

/** A session (persisted in SQLite) */
export interface Session {
  id: string;
  workingDir: string;
  agentId: string;
  title: string | null;
  createdAt: number;
  updatedAt: number;
}

/** Message role */
export type MessageRole = 'user' | 'assistant';

/** A message in a session */
export interface Message {
  id: number;
  sessionId: string;
  role: MessageRole;
  /** For user: plain text. For assistant: JSON array of ContentSegment */
  content: string;
  createdAt: number;
}

/** A segment of assistant content (text or tool call), stored in order */
export type ContentSegment =
  | { type: 'text'; text: string }
  | { type: 'toolCall'; id: string; title: string; status: string; locations?: string[] };

/** Full session with all messages */
export interface SessionFull {
  session: Session;
  messages: Message[];
}

/** Parse assistant message content into segments */
export function parseAssistantContent(content: string): ContentSegment[] {
  try {
    return JSON.parse(content) as ContentSegment[];
  } catch {
    // Fallback for plain text (shouldn't happen with new format)
    return [{ type: 'text', text: content }];
  }
}

/** Session status (live state) */
export type SessionStatus =
  | { status: 'idle' }
  | { status: 'processing' }
  | { status: 'error'; message: string }
  | { status: 'cancelled' };

/** Session status event payload */
export interface SessionStatusEvent {
  sessionId: string;
  status: SessionStatus;
}

// =============================================================================
// Types - ACP SDK (streaming events)
// =============================================================================

/** Content block types from ACP */
export type ContentBlock =
  | { type: 'text'; text: string }
  | { type: 'image'; data: string; mimeType: string }
  | { type: 'resource'; uri: string; mimeType?: string; text?: string };

/** Session update types from ACP */
export type SessionUpdate =
  | { sessionUpdate: 'agent_message_chunk'; content: ContentBlock }
  | {
      sessionUpdate: 'tool_call';
      toolCallId: string;
      title: string;
      status: string;
      locations?: Array<{ path: string }>;
    }
  | {
      sessionUpdate: 'tool_call_update';
      toolCallId: string;
      fields: { status?: string; title?: string; content?: unknown[] };
    }
  | { sessionUpdate: 'user_message_chunk'; content: ContentBlock }
  | { sessionUpdate: 'agent_thought_chunk'; content: ContentBlock }
  | { sessionUpdate: string }; // Catch-all

/** Session notification from ACP */
export interface SessionNotification {
  sessionId: string;
  update: SessionUpdate;
}

// =============================================================================
// Types - Legacy (for backward compatibility)
// =============================================================================

/** Available ACP provider info */
export interface AcpProviderInfo {
  id: string;
  label: string;
}

/** Response from legacy send_agent_prompt */
export interface AgentPromptResponse {
  response: string;
  sessionId: string;
}

/** Tool call summary (legacy) */
export interface ToolCallSummary {
  id: string;
  title: string;
  status: string;
  locations?: string[];
  resultPreview?: string;
}

/** Finalized message (legacy) */
export type FinalizedMessage =
  | { role: 'user'; content: string }
  | { role: 'assistant'; content: string; toolCalls?: ToolCallSummary[] };

/** Session complete event (legacy) */
export interface SessionCompleteEvent {
  sessionId: string;
  transcript: FinalizedMessage[];
}

/** Session error event (legacy) */
export interface SessionErrorEvent {
  sessionId: string;
  error: string;
}

// =============================================================================
// Session Commands
// =============================================================================

/**
 * Create a new session.
 * Returns the session ID.
 */
export async function createSession(workingDir: string, agentId?: string): Promise<string> {
  return invoke<string>('create_session', {
    workingDir,
    agentId: agentId ?? null,
  });
}

/**
 * Get full session with all messages.
 */
export async function getSession(sessionId: string): Promise<SessionFull | null> {
  return invoke<SessionFull | null>('get_session', { sessionId });
}

/**
 * Get session status (idle, processing, error).
 */
export async function getSessionStatus(sessionId: string): Promise<SessionStatus> {
  return invoke<SessionStatus>('get_session_status', { sessionId });
}

/**
 * Send a prompt to a session.
 * Streams response via events, persists on completion.
 */
export async function sendPrompt(sessionId: string, prompt: string): Promise<void> {
  return invoke<void>('send_prompt', { sessionId, prompt });
}

/**
 * Get buffered streaming segments for a session (before DB persistence).
 * Returns null if no buffered segments exist.
 */
export async function getBufferedSegments(sessionId: string): Promise<ContentSegment[] | null> {
  return invoke<ContentSegment[] | null>('get_buffered_segments', { sessionId });
}

/**
 * Update session title.
 */
export async function updateSessionTitle(sessionId: string, title: string): Promise<void> {
  return invoke<void>('update_session_title', { sessionId, title });
}

// =============================================================================
// Legacy AI Analysis Commands
// =============================================================================

/**
 * Check if an AI agent is available.
 */
export async function checkAiAvailable(): Promise<string> {
  return invoke<string>('check_ai_available');
}

/**
 * Discover available ACP providers on the system.
 */
export async function discoverAcpProviders(): Promise<AcpProviderInfo[]> {
  return invoke<AcpProviderInfo[]>('discover_acp_providers');
}

/**
 * Analyze a diff using AI.
 */
export async function analyzeDiff(
  repoPath: string | null,
  spec: DiffSpec,
  provider?: string | null
): Promise<ChangesetAnalysis> {
  return invoke<ChangesetAnalysis>('analyze_diff', { repoPath, spec, provider: provider ?? null });
}

/**
 * Send a prompt to the AI agent (non-streaming, legacy).
 */
export async function sendAgentPrompt(
  repoPath: string | null,
  prompt: string,
  sessionId?: string | null,
  provider?: string | null
): Promise<AgentPromptResponse> {
  return invoke<AgentPromptResponse>('send_agent_prompt', {
    repoPath,
    prompt,
    sessionId: sessionId ?? null,
    provider: provider ?? null,
  });
}

/**
 * Send a prompt with streaming (legacy).
 */
export async function sendAgentPromptStreaming(
  prompt: string,
  options?: {
    repoPath?: string;
    sessionId?: string;
    provider?: string;
  }
): Promise<AgentPromptResponse> {
  return invoke<AgentPromptResponse>('send_agent_prompt_streaming', {
    repoPath: options?.repoPath ?? null,
    prompt,
    sessionId: options?.sessionId ?? null,
    provider: options?.provider ?? null,
  });
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

/**
 * Convert AI annotations to comments and save them to the database.
 */
export async function saveAiComments(
  repoPath: string | null,
  spec: DiffSpec,
  annotations: SmartDiffAnnotation[]
): Promise<Comment[]> {
  return invoke<Comment[]>('save_ai_comments', { repoPath, spec, annotations });
}

// =============================================================================
// Event Listeners
// =============================================================================

/**
 * Listen for session update events (streaming chunks, tool calls).
 */
export async function listenToSessionUpdates(
  callback: (notification: SessionNotification) => void
): Promise<UnlistenFn> {
  return listen<SessionNotification>('session-update', (event) => {
    callback(event.payload);
  });
}

/**
 * Listen for session status changes.
 */
export async function listenToSessionStatus(
  callback: (event: SessionStatusEvent) => void
): Promise<UnlistenFn> {
  return listen<SessionStatusEvent>('session-status', (event) => {
    callback(event.payload);
  });
}
