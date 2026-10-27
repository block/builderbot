import { describe, expect, it } from 'vitest';
import type { HashtagItem } from '../../types';
import { reuseHashtagItems } from './hashtagItemsIdentity';

function item(overrides: Partial<HashtagItem> & Pick<HashtagItem, 'type' | 'id'>): HashtagItem {
  return { title: overrides.id, color: '#000', bgColor: '#fff', ...overrides };
}

describe('reuseHashtagItems', () => {
  it('returns the previous array when the items match', () => {
    const previous = [item({ type: 'project-note', id: 'p1' }), item({ type: 'commit', id: 'c1' })];
    const next = [item({ type: 'project-note', id: 'p1' }), item({ type: 'commit', id: 'c1' })];
    expect(reuseHashtagItems(previous, next)).toBe(previous);
  });

  it('returns the previous array for two empty lists', () => {
    const previous: HashtagItem[] = [];
    expect(reuseHashtagItems(previous, [])).toBe(previous);
  });

  it('ignores fields consumers cannot observe', () => {
    const previous = [item({ type: 'note', id: 'n1', noteContent: 'draft', branchName: 'a' })];
    const next = [item({ type: 'note', id: 'n1', noteContent: 'final', branchName: 'b' })];
    expect(reuseHashtagItems(previous, next)).toBe(previous);
  });

  it.each([
    ['a different id', item({ type: 'note', id: 'n2' })],
    ['a different type', item({ type: 'review', id: 'n1' })],
    ['a changed title', item({ type: 'note', id: 'n1', title: 'renamed' })],
    ['a changed subtitle', item({ type: 'note', id: 'n1', subtitle: 'edited' })],
    ['a newer note revision', item({ type: 'note', id: 'n1', noteUpdatedAt: 2 })],
  ])('returns the next array for %s', (_label, changed) => {
    const previous = [item({ type: 'note', id: 'n1', noteUpdatedAt: 1 })];
    const next = [changed];
    expect(reuseHashtagItems(previous, next)).toBe(next);
  });

  it('returns the next array when an item is added, removed, or reordered', () => {
    const a = item({ type: 'note', id: 'n1' });
    const b = item({ type: 'commit', id: 'c1' });
    expect(reuseHashtagItems([a], [a, b])).toHaveLength(2);
    expect(reuseHashtagItems([a, b], [a])).toHaveLength(1);
    const reordered = [b, a];
    expect(reuseHashtagItems([a, b], reordered)).toBe(reordered);
  });
});
