import { describe, it, expect } from 'vitest';
import { orderComments, formatTime, truncateText } from './comments';
import type { Comment } from '../types';

describe('orderComments', () => {
  it('returns empty array for no comments', () => {
    expect(orderComments([])).toEqual([]);
  });

  it('returns single comment as-is', () => {
    const comments: Comment[] = [
      { id: '1', author: 'Alice', role: 'human', body: 'Hello', createdAt: '2026-01-01T00:00:00Z' },
    ];
    expect(orderComments(comments)).toEqual(comments);
  });

  it('sorts root comments by time', () => {
    const comments: Comment[] = [
      { id: '2', author: 'Bob', role: 'human', body: 'Second', createdAt: '2026-01-02T00:00:00Z' },
      { id: '1', author: 'Alice', role: 'human', body: 'First', createdAt: '2026-01-01T00:00:00Z' },
    ];
    const result = orderComments(comments);
    expect(result[0].id).toBe('1');
    expect(result[1].id).toBe('2');
  });

  it('nests replies under their parents', () => {
    const comments: Comment[] = [
      { id: '1', author: 'Alice', role: 'human', body: 'Root', createdAt: '2026-01-01T00:00:00Z' },
      { id: '2', author: 'Bob', role: 'agent', body: 'Reply', createdAt: '2026-01-02T00:00:00Z', inReplyTo: '1' },
      { id: '3', author: 'Alice', role: 'human', body: 'Reply to reply', createdAt: '2026-01-03T00:00:00Z', inReplyTo: '2' },
    ];
    const result = orderComments(comments);
    expect(result.map(c => c.id)).toEqual(['1', '2', '3']);
  });

  it('treats replies to non-existent parents as roots', () => {
    const comments: Comment[] = [
      { id: '1', author: 'Alice', role: 'human', body: 'Root', createdAt: '2026-01-01T00:00:00Z' },
      { id: '2', author: 'Bob', role: 'agent', body: 'Orphaned reply', createdAt: '2026-01-02T00:00:00Z', inReplyTo: 'nonexistent' },
    ];
    const result = orderComments(comments);
    expect(result.length).toBe(2);
  });
});

describe('formatTime', () => {
  it('returns empty string for falsy input', () => {
    expect(formatTime('')).toBe('');
  });

  it('returns "just now" for recent timestamps', () => {
    const now = new Date().toISOString();
    expect(formatTime(now)).toBe('just now');
  });

  it('returns minutes ago', () => {
    const fiveMinAgo = new Date(Date.now() - 5 * 60000).toISOString();
    expect(formatTime(fiveMinAgo)).toBe('5m ago');
  });

  it('returns hours ago', () => {
    const threeHoursAgo = new Date(Date.now() - 3 * 3600000).toISOString();
    expect(formatTime(threeHoursAgo)).toBe('3h ago');
  });

  it('returns days ago', () => {
    const twoDaysAgo = new Date(Date.now() - 2 * 86400000).toISOString();
    expect(formatTime(twoDaysAgo)).toBe('2d ago');
  });
});

describe('truncateText', () => {
  it('returns empty string for falsy input', () => {
    expect(truncateText('', 10)).toBe('');
  });

  it('returns text as-is when shorter than max', () => {
    expect(truncateText('hello', 10)).toBe('hello');
  });

  it('truncates with ellipsis', () => {
    expect(truncateText('hello world', 5)).toBe('hello...');
  });
});
