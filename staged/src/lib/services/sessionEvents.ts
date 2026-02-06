/**
 * Session event listener service.
 *
 * Listens to Tauri events emitted by the Rust backend during ACP sessions:
 * - "session-update": Real-time SessionNotification from the ACP SDK
 * - "session-complete": Finalized transcript when session ends
 * - "session-error": Error information if session fails
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SessionNotification, SessionCompleteEvent, SessionErrorEvent } from './ai';

type SessionUpdateHandler = (notification: SessionNotification) => void;
type SessionCompleteHandler = (event: SessionCompleteEvent) => void;
type SessionErrorHandler = (event: SessionErrorEvent) => void;

const updateHandlers = new Set<SessionUpdateHandler>();
const completeHandlers = new Set<SessionCompleteHandler>();
const errorHandlers = new Set<SessionErrorHandler>();

let unlistenUpdate: UnlistenFn | null = null;
let unlistenComplete: UnlistenFn | null = null;
let unlistenError: UnlistenFn | null = null;

/**
 * Start listening for session events from the backend.
 * Call this once when the app starts or when session streaming is needed.
 */
export async function startSessionEventListeners(): Promise<void> {
  if (!unlistenUpdate) {
    unlistenUpdate = await listen<SessionNotification>('session-update', (e) => {
      for (const handler of updateHandlers) {
        try {
          handler(e.payload);
        } catch (err) {
          console.error('Error in session update handler:', err);
        }
      }
    });
  }

  if (!unlistenComplete) {
    unlistenComplete = await listen<SessionCompleteEvent>('session-complete', (e) => {
      for (const handler of completeHandlers) {
        try {
          handler(e.payload);
        } catch (err) {
          console.error('Error in session complete handler:', err);
        }
      }
    });
  }

  if (!unlistenError) {
    unlistenError = await listen<SessionErrorEvent>('session-error', (e) => {
      for (const handler of errorHandlers) {
        try {
          handler(e.payload);
        } catch (err) {
          console.error('Error in session error handler:', err);
        }
      }
    });
  }
}

/**
 * Subscribe to session update events (streaming chunks, tool calls).
 * Returns an unsubscribe function.
 */
export function onSessionUpdate(handler: SessionUpdateHandler): () => void {
  updateHandlers.add(handler);
  return () => updateHandlers.delete(handler);
}

/**
 * Subscribe to session complete events.
 * Returns an unsubscribe function.
 */
export function onSessionComplete(handler: SessionCompleteHandler): () => void {
  completeHandlers.add(handler);
  return () => completeHandlers.delete(handler);
}

/**
 * Subscribe to session error events.
 * Returns an unsubscribe function.
 */
export function onSessionError(handler: SessionErrorHandler): () => void {
  errorHandlers.add(handler);
  return () => errorHandlers.delete(handler);
}

/**
 * Stop listening for session events and clear all handlers.
 * Call this when session streaming is no longer needed.
 */
export function stopSessionEventListeners(): void {
  unlistenUpdate?.();
  unlistenComplete?.();
  unlistenError?.();
  unlistenUpdate = null;
  unlistenComplete = null;
  unlistenError = null;
  updateHandlers.clear();
  completeHandlers.clear();
  errorHandlers.clear();
}
