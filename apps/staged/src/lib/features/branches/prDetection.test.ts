import { describe, expect, it } from 'vitest';
import { describePrRejectionReason } from './prDetection';

describe('describePrRejectionReason', () => {
  it('explains a closed or merged PR', () => {
    expect(describePrRejectionReason({ kind: 'notOpen', state: 'merged' })).toBe(
      'It is merged; only an open pull request is linked.'
    );
  });

  it('names both the PR base and the branch base', () => {
    expect(
      describePrRejectionReason({ kind: 'wrongBase', expected: 'main', actual: 'develop' })
    ).toBe('It targets develop, but this branch is based on main.');
  });

  it('names the fork the head lives in', () => {
    expect(
      describePrRejectionReason({
        kind: 'wrongHeadRepo',
        expected: ['acme/widgets'],
        actual: 'fork-owner/widgets',
      })
    ).toBe('Its head branch lives in fork-owner/widgets, not acme/widgets.');
  });

  it('lists every acceptable head repository for a fork setup', () => {
    expect(
      describePrRejectionReason({
        kind: 'wrongHeadRepo',
        expected: ['acme/widgets', 'fork-owner/widgets'],
        actual: 'someone-else/widgets',
      })
    ).toBe(
      'Its head branch lives in someone-else/widgets, not acme/widgets or fork-owner/widgets.'
    );
  });

  it('handles a deleted head repository', () => {
    expect(
      describePrRejectionReason({ kind: 'wrongHeadRepo', expected: ['acme/widgets'], actual: null })
    ).toBe('Its head repository no longer exists (this branch pushes to acme/widgets).');
  });
});
