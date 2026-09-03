import { describe, expect, it } from 'vitest';
import { nextActiveSessionIds } from './activeSessionIds';

describe('nextActiveSessionIds', () => {
  it('tracks a project session from its running event', () => {
    const next = nextActiveSessionIds(new Set(), { sessionId: 'parent', status: 'running' });

    expect(next).toEqual(new Set(['parent']));
  });

  it('keeps a queue-drained repo session out of the query set', () => {
    const next = nextActiveSessionIds(new Set(['parent']), {
      sessionId: 'child',
      status: 'running',
      branchId: 'branch-1',
      sessionType: 'commit',
    });

    expect(next).toEqual(new Set(['parent']));
  });

  it('keeps tracking a resumed project session despite its stamped type', () => {
    // The resume path stamps `sessionType: "note"` on a project-note session's
    // running event; only the missing branch marks it as a parent.
    const next = nextActiveSessionIds(new Set(), {
      sessionId: 'parent',
      status: 'running',
      branchId: null,
      sessionType: 'note',
    });

    expect(next).toEqual(new Set(['parent']));
  });

  it('drops a finished session', () => {
    const next = nextActiveSessionIds(new Set(['parent']), {
      sessionId: 'parent',
      status: 'completed',
    });

    expect(next).toEqual(new Set());
  });

  it('returns a fresh Set even when the contents are unchanged', () => {
    // Reference identity is the refresh trigger: a child finishing in a window
    // that never saw it start leaves the contents untouched, and only the new
    // Set re-runs the badge $effect.
    const current = new Set(['parent']);

    const afterChildRunning = nextActiveSessionIds(current, {
      sessionId: 'child',
      status: 'running',
      branchId: 'branch-1',
      sessionType: 'note',
    });
    const afterChildDone = nextActiveSessionIds(current, {
      sessionId: 'child',
      status: 'completed',
    });

    expect(afterChildRunning).not.toBe(current);
    expect(afterChildDone).not.toBe(current);
    expect(afterChildRunning).toEqual(current);
    expect(afterChildDone).toEqual(current);
  });
});
