import { describe, expect, it } from 'vitest';
import { standaloneQueuedPullRowCopy } from './queuedPullRow';

describe('standaloneQueuedPullRowCopy', () => {
  it('leaves the badge on the origin-ahead row that already hosts it', () => {
    expect(standaloneQueuedPullRowCopy({ pullQueued: true, relation: 'originAhead' })).toBeNull();
  });

  it('keeps a queued pull visible once the branch diverges', () => {
    expect(standaloneQueuedPullRowCopy({ pullQueued: true, relation: 'diverged' })).toEqual({
      title: 'Pull from origin',
      meta: 'Pull queued',
    });
  });

  it('keeps a queued pull visible for relations that render no upstream row', () => {
    for (const relation of ['inSync', 'localAhead', 'missing'] as const) {
      expect(standaloneQueuedPullRowCopy({ pullQueued: true, relation })).not.toBeNull();
    }
  });

  it('keeps a queued pull visible when the timeline has no git state', () => {
    expect(standaloneQueuedPullRowCopy({ pullQueued: true, relation: null })).not.toBeNull();
  });

  it('renders nothing when no pull is queued', () => {
    for (const relation of ['originAhead', 'diverged', 'inSync', null] as const) {
      expect(standaloneQueuedPullRowCopy({ pullQueued: false, relation })).toBeNull();
    }
  });
});
