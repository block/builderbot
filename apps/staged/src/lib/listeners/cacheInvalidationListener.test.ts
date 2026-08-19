import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { BranchChangedEvent } from '../types';

/**
 * The branch-changed leg of the cache listener, including the store change
 * feed's lag recovery: a null branchId means the feed dropped changes it can
 * no longer name, so invalidation widens from one branch to all of them.
 */

type EventCallback = (payload: unknown) => void;

let eventCallbacks: Map<string, EventCallback>;
let invalidateCache: ReturnType<typeof vi.fn>;
let invalidateCacheByArgs: ReturnType<typeof vi.fn>;
let invalidateCacheByCommand: ReturnType<typeof vi.fn>;
let invalidateBranchTimeline: ReturnType<typeof vi.fn>;
let invalidateAllBranchTimelines: ReturnType<typeof vi.fn>;

function emitBranchChanged(payload: BranchChangedEvent): void {
  eventCallbacks.get('branch-changed')?.(payload);
}

async function startListening() {
  const { listenForCacheInvalidation } = await import('./cacheInvalidationListener');
  return listenForCacheInvalidation();
}

beforeEach(() => {
  vi.resetModules();
  eventCallbacks = new Map();
  invalidateCache = vi.fn();
  invalidateCacheByArgs = vi.fn();
  invalidateCacheByCommand = vi.fn();
  invalidateBranchTimeline = vi.fn();
  invalidateAllBranchTimelines = vi.fn();

  vi.doMock('../transport', () => ({
    listenToEvent: (event: string, callback: EventCallback) => {
      eventCallbacks.set(event, callback);
      return () => eventCallbacks.delete(event);
    },
  }));
  vi.doMock('../cache', () => ({
    invalidateCache,
    invalidateCacheByArgs,
    invalidateCacheByCommand,
  }));
  vi.doMock('../commands', () => ({
    invalidateBranchTimeline,
    invalidateAllBranchTimelines,
  }));
});

afterEach(() => {
  vi.doUnmock('../transport');
  vi.doUnmock('../cache');
  vi.doUnmock('../commands');
});

describe('branch-changed cache invalidation', () => {
  it('invalidates just the named branch’s timeline and diffs, and just the named project’s list', async () => {
    await startListening();

    emitBranchChanged({ branchId: 'b1', projectId: 'p1' });

    expect(invalidateBranchTimeline).toHaveBeenCalledWith('b1');
    expect(invalidateAllBranchTimelines).not.toHaveBeenCalled();
    expect(invalidateCacheByArgs).toHaveBeenCalledWith('list_branches_for_project', {
      projectId: 'p1',
    });
    expect(invalidateCacheByArgs).toHaveBeenCalledWith('get_diff_files', { branchId: 'b1' });
    expect(invalidateCacheByArgs).toHaveBeenCalledWith('get_file_diff', { branchId: 'b1' });
    // Nothing widens: another project's cached branch list keeps its instant paint.
    expect(invalidateCacheByCommand).not.toHaveBeenCalled();
  });

  it('widens the branch-list drop when the backend couldn’t resolve the project', async () => {
    await startListening();

    emitBranchChanged({ branchId: 'b1', projectId: null });

    expect(invalidateCacheByCommand.mock.calls).toEqual([['list_branches_for_project']]);
    // The branch-scoped half is unaffected by an unresolved project.
    expect(invalidateBranchTimeline).toHaveBeenCalledWith('b1');
    expect(invalidateAllBranchTimelines).not.toHaveBeenCalled();
    expect(invalidateCacheByArgs.mock.calls).toEqual([
      ['get_diff_files', { branchId: 'b1' }],
      ['get_file_diff', { branchId: 'b1' }],
    ]);
  });

  it('widens to every branch when the lag flush names none', async () => {
    await startListening();

    emitBranchChanged({ branchId: null, projectId: null });

    expect(invalidateAllBranchTimelines).toHaveBeenCalledTimes(1);
    expect(invalidateBranchTimeline).not.toHaveBeenCalled();
    // Command-wide, since there's no branch id to match cached args against.
    expect(invalidateCacheByArgs).not.toHaveBeenCalled();
    expect(invalidateCacheByCommand.mock.calls.map(([command]) => command)).toEqual([
      'list_branches_for_project',
      'get_diff_files',
      'get_file_diff',
    ]);
  });
});
