/**
 * streamingSession.svelte.ts - Persistent streaming state for AI sessions
 *
 * Keeps streaming state alive independent of component lifecycle.
 * A single pair of global event listeners accumulates chunks for all sessions.
 * Components connect/disconnect without losing data.
 *
 * Uses plain objects (not Map/Set) so Svelte 5's $state proxy tracks all
 * reads and writes deeply — components re-render when streaming data changes.
 */

import {
  listenToSessionUpdates,
  listenToSessionStatus,
  type SessionNotification,
  type SessionStatusEvent,
} from '../services/ai';
import type { DisplaySegment } from '../types/streaming';
import type { UnlistenFn } from '@tauri-apps/api/event';

// =============================================================================
// Types
// =============================================================================

export interface StreamingSessionState {
  /** Ordered segments for the current streaming turn */
  streamingSegments: DisplaySegment[];
  /** Tool calls indexed by ID for in-place updates */
  toolCalls: Record<string, DisplaySegment & { type: 'tool' }>;
  /** Whether the session is currently processing */
  isActive: boolean;
  /** Error message if the session errored */
  error: string | null;
}

export interface ConnectOptions {
  /** Called when the session transitions to idle */
  onIdle?: () => void;
  /** Called when the session encounters an error */
  onError?: (message: string) => void;
}

interface SessionEntry {
  state: StreamingSessionState;
  subscriberCount: number;
  callbacks: ConnectOptions[];
}

// =============================================================================
// Module-level state (survives component mount/unmount)
// =============================================================================

// Plain object so $state deeply proxies all nested reads/writes.
let sessions: Record<string, SessionEntry> = $state({});

let listenersInitialized = false;
let unlistenUpdate: UnlistenFn | null = null;
let unlistenStatus: UnlistenFn | null = null;

// =============================================================================
// Global event handlers
// =============================================================================

function processUpdate(state: StreamingSessionState, update: SessionNotification['update']) {
  if (update.sessionUpdate === 'agent_message_chunk') {
    if ('content' in update && update.content.type === 'text') {
      const lastSegment = state.streamingSegments[state.streamingSegments.length - 1];
      if (lastSegment && lastSegment.type === 'text') {
        lastSegment.text += update.content.text;
        state.streamingSegments = [...state.streamingSegments];
      } else {
        state.streamingSegments = [
          ...state.streamingSegments,
          { type: 'text', text: update.content.text },
        ];
      }
    }
  } else if (update.sessionUpdate === 'tool_call') {
    if ('toolCallId' in update) {
      const toolSegment: DisplaySegment & { type: 'tool' } = {
        type: 'tool',
        id: update.toolCallId,
        title: update.title,
        status: update.status,
      };
      state.toolCalls[update.toolCallId] = toolSegment;
      state.streamingSegments = [...state.streamingSegments, toolSegment];
    }
  } else if (update.sessionUpdate === 'tool_call_update') {
    if ('toolCallId' in update) {
      const existing = state.toolCalls[update.toolCallId];
      if (existing && update.fields) {
        if (update.fields.title) existing.title = update.fields.title;
        if (update.fields.status) existing.status = update.fields.status;
        state.streamingSegments = [...state.streamingSegments];
      }
    }
  }
}

function handleSessionUpdate(notification: SessionNotification) {
  // The backend stamps the internal session ID onto all update events,
  // so we can do a direct lookup. This works correctly with multiple
  // concurrent sessions.
  const entry = sessions[notification.sessionId];
  if (entry?.state.isActive) {
    processUpdate(entry.state, notification.update);
  }
}

function handleSessionStatus(event: SessionStatusEvent) {
  // Status events carry the internal session ID.
  const entry = sessions[event.sessionId];
  if (!entry) return;

  if (event.status.status === 'idle' || event.status.status === 'cancelled') {
    entry.state.isActive = false;
    entry.state.error = null;

    // Notify all subscribers
    for (const cb of entry.callbacks) {
      cb.onIdle?.();
    }

    // Clean up if no subscribers remain
    if (entry.subscriberCount <= 0) {
      cleanupSession(event.sessionId);
    }
  } else if (event.status.status === 'processing') {
    entry.state.isActive = true;
    entry.state.error = null;
  } else if (event.status.status === 'error') {
    const message = event.status.message;
    entry.state.isActive = false;
    entry.state.error = message;
    entry.state.streamingSegments = [];
    entry.state.toolCalls = {};

    // Notify all subscribers
    for (const cb of entry.callbacks) {
      cb.onError?.(message);
    }

    // Clean up if no subscribers remain
    if (entry.subscriberCount <= 0) {
      cleanupSession(event.sessionId);
    }
  }
}

async function ensureListeners() {
  if (listenersInitialized) return;
  listenersInitialized = true;

  unlistenUpdate = await listenToSessionUpdates(handleSessionUpdate);
  unlistenStatus = await listenToSessionStatus(handleSessionStatus);
}

function cleanupSession(sessionId: string) {
  delete sessions[sessionId];
}

// =============================================================================
// Public API
// =============================================================================

/**
 * Connect to a streaming session. Returns the reactive state handle.
 * If the session already has accumulated streaming data, it's immediately available.
 * Call disconnectFromSession when the component unmounts.
 */
export function connectToSession(
  sessionId: string,
  options: ConnectOptions = {}
): StreamingSessionState {
  // Lazily initialize global listeners
  ensureListeners();

  const existing = sessions[sessionId];
  if (existing) {
    // Existing entry — increment subscriber count and add callbacks
    existing.subscriberCount++;
    existing.callbacks.push(options);
    return existing.state;
  }

  // New entry — assign through the proxy so nested objects are reactive
  sessions[sessionId] = {
    state: {
      streamingSegments: [],
      toolCalls: {},
      isActive: true,
      error: null,
    },
    subscriberCount: 1,
    callbacks: [options],
  };

  // Read back through the proxy to return the reactive version
  return sessions[sessionId].state;
}

/**
 * Disconnect from a streaming session.
 * State is preserved if the session is still active (streaming).
 * State is cleaned up only when idle AND no subscribers remain.
 */
export function disconnectFromSession(sessionId: string, options?: ConnectOptions) {
  const entry = sessions[sessionId];
  if (!entry) return;

  entry.subscriberCount--;

  // Remove the specific callbacks instance
  if (options) {
    const idx = entry.callbacks.indexOf(options);
    if (idx !== -1) {
      entry.callbacks.splice(idx, 1);
    }
  }

  // Only clean up if idle/errored AND no subscribers
  if (!entry.state.isActive && entry.subscriberCount <= 0) {
    cleanupSession(sessionId);
  }
}

/**
 * Clear streaming state for a session (e.g., after refreshing from database).
 */
export function clearStreamingState(sessionId: string) {
  const entry = sessions[sessionId];
  if (!entry) return;

  entry.state.streamingSegments = [];
  entry.state.toolCalls = {};
}

/**
 * Get the current state for a session without connecting.
 * Returns undefined if no entry exists.
 */
export function getStreamingState(sessionId: string): StreamingSessionState | undefined {
  return sessions[sessionId]?.state;
}
