import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionReconcileEvent } from '../types';

// The store is a rune module; under plain vitest `$state` is stubbed as
// identity, which is all a Map-and-counter store needs.

async function freshStore() {
  const { disconnectedSessionsStore } = await import('./disconnectedSessions.svelte');
  return disconnectedSessionsStore;
}

function event(
  sessionId: string,
  phase: SessionReconcileEvent['phase'],
  overrides: Partial<SessionReconcileEvent> = {}
): SessionReconcileEvent {
  return {
    sessionId,
    branchId: null,
    projectId: null,
    phase,
    reason: null,
    attempts: 0,
    nextAttemptAt: null,
    ...overrides,
  };
}

const never = () => false;

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (value: unknown) => value);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('disconnectedSessionsStore', () => {
  it('tracks disconnected sessions per project, in the order they were learnt', async () => {
    const store = await freshStore();
    store.markDisconnected('s1', 'p1', 'b1');
    store.markDisconnected('s2', 'p2', 'b2');
    store.markDisconnected('s3', 'p1', 'b3');

    expect(store.sessionsForProject('p1')).toEqual(['s1', 's3']);
    expect(store.hasDisconnectedSessions('p2')).toBe(true);

    store.clear('s2');
    expect(store.hasDisconnectedSessions('p2')).toBe(false);
    expect(store.isDisconnected('s2')).toBe(false);
  });

  it('keeps a known project when a later event names none', async () => {
    const store = await freshStore();
    store.markDisconnected('s1', 'p1', 'b1');
    store.markDisconnected('s1', null, null);
    expect(store.sessionsForProject('p1')).toEqual(['s1']);
  });

  it('adopts a snapshot as the whole set, except for skipped sessions', async () => {
    const store = await freshStore();
    store.markDisconnected('gone', 'p1', null);
    store.markDisconnected('raced', 'p1', null);

    store.applySnapshot(
      [
        { sessionId: 'new', projectId: 'p2', branchId: 'b2' },
        { sessionId: 'stale', projectId: 'p2', branchId: 'b3' },
      ],
      (id) => id === 'raced' || id === 'stale'
    );

    expect(store.isDisconnected('gone')).toBe(false);
    expect(store.isDisconnected('raced')).toBe(true);
    expect(store.isDisconnected('new')).toBe(true);
    expect(store.isDisconnected('stale')).toBe(false);
  });

  it('follows reconcile events, dropping a session’s state on idle', async () => {
    const store = await freshStore();
    const checking = event('s1', 'checking');
    store.applyReconcileEvent(checking);
    expect(store.getReconcileState('s1')).toBe(checking);

    store.applyReconcileEvent(event('s1', 'idle'));
    expect(store.getReconcileState('s1')).toBeNull();
    expect(store.getReconcileState(null)).toBeNull();
  });

  it('gives every reconcile event a new state, so pending checks can tell one arrived', async () => {
    const store = await freshStore();
    const first = event('s1', 'waiting');
    store.applyReconcileEvent(first);
    store.applyReconcileEvent(event('s1', 'waiting'));
    expect(store.getReconcileState('s1')).not.toBe(first);
  });

  it('learns a disconnected session’s project from its reconcile event', async () => {
    const store = await freshStore();
    store.markDisconnected('s1', null, null);
    store.applyReconcileEvent(event('s1', 'checking', { projectId: 'p1', branchId: 'b1' }));
    expect(store.sessionsForProject('p1')).toEqual(['s1']);

    // An event alone doesn't make a session disconnected.
    store.applyReconcileEvent(event('s2', 'checking', { projectId: 'p1' }));
    expect(store.isDisconnected('s2')).toBe(false);
  });

  it('adopts a reconcile snapshot, keeping skipped sessions as they are', async () => {
    const store = await freshStore();
    store.applyReconcileEvent(event('settled', 'waiting'));
    const raced = event('raced', 'checking');
    store.applyReconcileEvent(raced);

    store.applyReconcileSnapshot(
      [event('raced', 'waiting'), event('fresh', 'stopping')],
      (id) => id === 'raced'
    );

    expect(store.getReconcileState('settled')).toBeNull();
    expect(store.getReconcileState('raced')).toBe(raced);
    expect(store.getReconcileState('fresh')?.phase).toBe('stopping');
  });

  it('speaks for a project with its most telling session’s state', async () => {
    const store = await freshStore();
    store.markDisconnected('quiet', 'p1', null);
    store.markDisconnected('busy', 'p1', null);
    store.markDisconnected('elsewhere', 'p2', null);
    store.applyReconcileSnapshot(
      [event('quiet', 'waiting'), event('busy', 'checking'), event('elsewhere', 'stopping')],
      never
    );

    expect(store.projectReconcileState('p1')?.sessionId).toBe('busy');
    expect(store.projectReconcileState('p3')).toBeNull();
  });
});
