/**
 * Shared branch event subscriptions.
 *
 * Branch cards mount in bulk during project navigation. This service keeps one
 * Tauri listener per backend event and fans events out to lightweight local
 * subscribers so navigation does not repeatedly register/unregister listeners.
 */

import { listenToEvent, type UnlistenFn } from '../transport';
import type { ActionStatusEvent, RunPhaseChangedEvent } from '../features/actions/actions';
import type { BranchGitState, PrStatusChangedEvent, SessionStatusPayload } from '../types';

type EventCallback<T> = (payload: T) => void;

interface SharedEventEntry<T> {
  callbacks: Set<EventCallback<T>>;
  unlisten: UnlistenFn | null;
}

export interface BranchSetupProgressEvent {
  branchId: string;
  phase: string;
  detail: string | null;
}

export interface BranchGitStateUpdatedEvent {
  branchId: string;
  gitState: BranchGitState;
}

const sharedEvents = new Map<string, SharedEventEntry<unknown>>();

function getSharedEventEntry<T>(eventName: string): SharedEventEntry<T> {
  let entry = sharedEvents.get(eventName) as SharedEventEntry<T> | undefined;
  if (!entry) {
    entry = {
      callbacks: new Set<EventCallback<T>>(),
      unlisten: null,
    };
    sharedEvents.set(eventName, entry as SharedEventEntry<unknown>);
  }
  return entry;
}

function subscribeSharedEvent<T>(eventName: string, callback: EventCallback<T>): UnlistenFn {
  const entry = getSharedEventEntry<T>(eventName);
  entry.callbacks.add(callback);

  if (!entry.unlisten) {
    entry.unlisten = listenToEvent<T>(eventName, (payload) => {
      for (const subscriber of [...entry.callbacks]) {
        try {
          subscriber(payload);
        } catch (error) {
          console.error(`[branchEventService] subscriber failed for ${eventName}:`, error);
        }
      }
    });
  }

  let active = true;
  return () => {
    if (!active) return;
    active = false;
    entry.callbacks.delete(callback);
  };
}

function combineUnlisteners(unlisteners: UnlistenFn[]): UnlistenFn {
  return () => {
    for (const unlisten of unlisteners) {
      unlisten();
    }
  };
}

function subscribeBranchPayload<T extends { branchId: string }>(
  eventName: string,
  branchId: string,
  callback: EventCallback<T>
): UnlistenFn {
  return subscribeSharedEvent<T>(eventName, (payload) => {
    if (payload.branchId === branchId) {
      callback(payload);
    }
  });
}

export function onBranchSetupProgress(
  branchId: string,
  callback: EventCallback<BranchSetupProgressEvent>
): UnlistenFn {
  return combineUnlisteners([
    subscribeBranchPayload('worktree-setup-progress', branchId, callback),
    subscribeBranchPayload('workspace-setup-progress', branchId, callback),
  ]);
}

export function onSessionStatusChanged(callback: EventCallback<SessionStatusPayload>): UnlistenFn {
  return subscribeSharedEvent('session-status-changed', callback);
}

export function onBranchSessionStatus(
  branchId: string,
  callback: EventCallback<SessionStatusPayload>
): UnlistenFn {
  return subscribeSharedEvent<SessionStatusPayload>('session-status-changed', (payload) => {
    if (payload.branchId === branchId) {
      callback(payload);
    }
  });
}

export function onBranchGitStateUpdated(
  branchId: string,
  callback: EventCallback<BranchGitStateUpdatedEvent>
): UnlistenFn {
  return subscribeBranchPayload('git-state-updated', branchId, callback);
}

export function onBranchPrStatusChanged(
  branchId: string,
  callback: EventCallback<PrStatusChangedEvent>
): UnlistenFn {
  return subscribeBranchPayload('pr-status-changed', branchId, callback);
}

export function onBranchPrStatusCleared(
  branchId: string,
  callback: EventCallback<string>
): UnlistenFn {
  return subscribeSharedEvent<string>('pr-status-cleared', (clearedBranchId) => {
    if (clearedBranchId === branchId) {
      callback(clearedBranchId);
    }
  });
}

export function onBranchActionStatus(
  branchId: string,
  callback: EventCallback<ActionStatusEvent>
): UnlistenFn {
  return subscribeBranchPayload('action_status', branchId, callback);
}

export function onBranchRunPhaseChanged(
  branchId: string,
  callback: EventCallback<RunPhaseChangedEvent>
): UnlistenFn {
  return subscribeBranchPayload('action:run-phase-changed', branchId, callback);
}
