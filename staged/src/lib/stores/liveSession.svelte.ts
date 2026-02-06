/**
 * Live session store for real-time session streaming.
 *
 * Tracks streaming state for active sessions, accumulating text chunks
 * and tool call updates as they arrive from the backend.
 */

import type {
  SessionNotification,
  FinalizedMessage,
  SessionUpdate,
  ContentBlock,
} from '../services/ai';
import {
  onSessionUpdate,
  onSessionComplete,
  onSessionError,
  startSessionEventListeners,
} from '../services/sessionEvents';

// Types used locally by this store

export type ToolCallStatus = 'pending' | 'in_progress' | 'completed' | 'failed';

export interface ToolCallLocation {
  path: string;
  line?: number;
}

export interface LiveToolCall {
  id: string;
  title: string;
  status: ToolCallStatus;
  kind: string;
  locations: string[];
  preview?: string;
}

export interface LiveSession {
  sessionId: string;
  isStreaming: boolean;
  currentText: string;
  toolCalls: Map<string, LiveToolCall>;
  error?: string;
  finalTranscript: FinalizedMessage[] | null;
}

// Narrow types for session update variants
interface SessionUpdateAgentMessageChunk {
  sessionUpdate: 'agent_message_chunk';
  content: ContentBlock;
}

interface SessionUpdateToolCall {
  sessionUpdate: 'tool_call';
  toolCallId: string;
  title: string;
  status: string;
  kind?: string;
  locations?: ToolCallLocation[];
}

interface SessionUpdateToolCallUpdate {
  sessionUpdate: 'tool_call_update';
  toolCallId: string;
  fields: {
    status?: ToolCallStatus;
    title?: string;
    content?: Array<{ type: string; [key: string]: unknown }>;
    locations?: ToolCallLocation[];
  };
}

class LiveSessionStore {
  sessions = $state<Map<string, LiveSession>>(new Map());
  /** The most recently created/updated session ID (for finding active streams) */
  mostRecentSessionId = $state<string | null>(null);
  #initialized = false;

  /**
   * Initialize the store and start listening for events.
   * Safe to call multiple times.
   */
  async init(): Promise<void> {
    if (this.#initialized) return;
    this.#initialized = true;

    await startSessionEventListeners();
    onSessionUpdate((n) => this.handleUpdate(n));
    onSessionComplete((e) => this.handleComplete(e.sessionId, e.transcript));
    onSessionError((e) => this.handleError(e.sessionId, e.error));
  }

  private getOrCreate(sessionId: string): LiveSession {
    let session = this.sessions.get(sessionId);
    if (!session) {
      session = {
        sessionId,
        isStreaming: true,
        currentText: '',
        toolCalls: new Map(),
        finalTranscript: null,
      };
      // Create a new Map to trigger reactivity
      const newSessions = new Map(this.sessions);
      newSessions.set(sessionId, session);
      this.sessions = newSessions;
    }
    // Track most recent session for easy lookup
    this.mostRecentSessionId = sessionId;
    return session;
  }

  private handleUpdate(notification: SessionNotification): void {
    // Handle both camelCase (our types) and snake_case (SDK) for sessionId
    const sessionId =
      notification.sessionId ?? (notification as unknown as { session_id: string }).session_id;
    if (!sessionId) {
      console.warn('[LiveSession] Received notification without session ID:', notification);
      return;
    }
    const session = this.getOrCreate(sessionId);
    const update = notification.update;

    // Handle both camelCase and snake_case discriminator
    const updateType =
      update.sessionUpdate ?? (update as unknown as { session_update: string }).session_update;

    switch (updateType) {
      case 'agent_message_chunk': {
        const chunk = update as SessionUpdateAgentMessageChunk;
        if (chunk.content?.type === 'text') {
          session.currentText += chunk.content.text;
          this.triggerUpdate(notification.sessionId);
        }
        break;
      }

      case 'tool_call': {
        const tc = update as SessionUpdateToolCall;
        // Handle both camelCase (our types) and snake_case (SDK) field names
        const toolCallId =
          tc.toolCallId ?? (tc as unknown as { tool_call_id: string }).tool_call_id;
        if (toolCallId) {
          session.toolCalls.set(toolCallId, {
            id: toolCallId,
            title: tc.title ?? 'Tool Call',
            status: (tc.status as ToolCallStatus) ?? 'pending',
            kind: tc.kind ?? 'other',
            locations: tc.locations?.map((l: ToolCallLocation) => l.path) ?? [],
          });
          this.triggerUpdate(notification.sessionId);
        }
        break;
      }

      case 'tool_call_update': {
        const tcu = update as SessionUpdateToolCallUpdate;
        // Handle both camelCase (our types) and snake_case (SDK) field names
        const toolCallId =
          tcu.toolCallId ?? (tcu as unknown as { tool_call_id: string }).tool_call_id;
        const tc = toolCallId ? session.toolCalls.get(toolCallId) : undefined;
        // fields might be undefined or named differently in SDK
        const fields = tcu.fields ?? (tcu as unknown as Record<string, unknown>);
        if (tc && fields) {
          const status = (fields as { status?: ToolCallStatus }).status;
          const title = (fields as { title?: string }).title;
          const locations = (fields as { locations?: ToolCallLocation[] }).locations;
          const content = (fields as { content?: Array<{ type: string; [key: string]: unknown }> })
            .content;

          if (status) tc.status = status;
          if (title) tc.title = title;
          if (locations) {
            tc.locations = locations.map((l: ToolCallLocation) => l.path);
          }
          // Extract preview from content if available
          if (content?.length) {
            tc.preview = this.extractPreview(content);
          }
          this.triggerUpdate(sessionId);
        }
        break;
      }

      case 'agent_thought_chunk':
        // Could display thoughts differently, for now ignore
        break;

      default:
        // Ignore unknown update types
        break;
    }
  }

  private extractPreview(
    content: Array<{ type: string; [key: string]: unknown }>
  ): string | undefined {
    for (const item of content) {
      if (item.type === 'content' && item.content) {
        const c = item.content as { type: string; text?: string };
        if (c.type === 'text' && c.text) {
          return c.text.slice(0, 200) + (c.text.length > 200 ? '...' : '');
        }
      }
      if (item.type === 'diff') {
        return `${item.path}${item.oldText ? ' (modified)' : ' (new)'}`;
      }
      if (item.type === 'terminal') {
        return `Terminal: ${item.terminalId}`;
      }
    }
    return undefined;
  }

  private handleComplete(sessionId: string, transcript: FinalizedMessage[]): void {
    const session = this.sessions.get(sessionId);
    if (session) {
      session.isStreaming = false;
      session.finalTranscript = transcript;
      session.currentText = '';
      session.toolCalls.clear();
      this.triggerUpdate(sessionId);
    }
  }

  private handleError(sessionId: string, error: string): void {
    const session = this.sessions.get(sessionId);
    if (session) {
      session.isStreaming = false;
      session.error = error;
      this.triggerUpdate(sessionId);
    }
  }

  /**
   * Trigger reactivity by creating a new Map reference.
   */
  private triggerUpdate(sessionId: string): void {
    // Svelte 5 should track the Map mutations, but to be safe we can
    // reassign the map. For now, rely on $state tracking.
    // If reactivity issues occur, uncomment:
    // this.sessions = new Map(this.sessions);
  }

  /**
   * Get a live session by ID.
   */
  get(sessionId: string): LiveSession | undefined {
    return this.sessions.get(sessionId);
  }

  /**
   * Get the most recently active streaming session (if any).
   * Useful when you don't know the session ID yet but want to show streaming state.
   */
  getMostRecentStreaming(): LiveSession | undefined {
    if (!this.mostRecentSessionId) return undefined;
    const session = this.sessions.get(this.mostRecentSessionId);
    return session?.isStreaming ? session : undefined;
  }

  /**
   * Check if a session is currently streaming.
   */
  isStreaming(sessionId: string): boolean {
    return this.sessions.get(sessionId)?.isStreaming ?? false;
  }

  /**
   * Clear a session from the store.
   */
  clear(sessionId: string): void {
    const newSessions = new Map(this.sessions);
    newSessions.delete(sessionId);
    this.sessions = newSessions;
  }

  /**
   * Clear all sessions.
   */
  clearAll(): void {
    this.sessions = new Map();
  }
}

export const liveSessionStore = new LiveSessionStore();
