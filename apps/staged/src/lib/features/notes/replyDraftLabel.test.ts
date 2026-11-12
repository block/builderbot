import { describe, expect, it } from 'vitest';
import { formatReplyDraftLabel } from './replyDraftLabel';

describe('formatReplyDraftLabel', () => {
  it('names the draft and counts its quotes', () => {
    expect(formatReplyDraftLabel(null)).toBeNull();
    expect(formatReplyDraftLabel({ quotes: 0, hasText: true })).toBe('Unsent reply draft');
    expect(formatReplyDraftLabel({ quotes: 1, hasText: false })).toBe(
      'Unsent reply draft · 1 quote'
    );
    expect(formatReplyDraftLabel({ quotes: 3, hasText: true })).toBe(
      'Unsent reply draft · 3 quotes'
    );
  });
});
