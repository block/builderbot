// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';

vi.mock('../../commands', () => ({
  getBranchTimeline: vi.fn(),
  listProjectNotes: vi.fn(),
}));

import type { HashtagItem } from '../../types';
import { BADGE_CARET_ANCHOR, hashtagContentFragment } from './hashtagEditorContent';

const note: HashtagItem = {
  type: 'note',
  id: 'abc',
  title: 'Porting <notes>',
  color: '--note-color',
  bgColor: '--note-bg',
};
const itemsByKey = new Map([['note:abc', note]]);

function shape(fragment: DocumentFragment): string[] {
  return Array.from(fragment.childNodes, (node) =>
    node instanceof HTMLElement
      ? `<${node.tagName.toLowerCase()}${node.dataset.token ? ` ${node.dataset.token}` : ''}>`
      : (node.textContent ?? '')
  );
}

describe('hashtagContentFragment', () => {
  it('renders resolved references as badges followed by a caret anchor', () => {
    const { fragment, unresolved } = hashtagContentFragment('see #note:abc now', itemsByKey);
    expect(shape(fragment)).toEqual(['see ', '<span #note:abc>', BADGE_CARET_ANCHOR, ' now']);
    expect(unresolved).toBe(false);

    const badge = fragment.querySelector('span')!;
    expect(badge.contentEditable).toBe('false');
    expect(badge.textContent).toBe('Porting <notes>');
    expect(badge.querySelector('svg')).not.toBeNull();
  });

  it('keeps unresolved references as text and says so', () => {
    const { fragment, unresolved } = hashtagContentFragment('#commit:deadbeef x', itemsByKey);
    expect(shape(fragment)).toEqual(['#commit:deadbeef', ' x']);
    expect(unresolved).toBe(true);
  });

  it('turns newlines into line breaks', () => {
    const { fragment } = hashtagContentFragment('a\n\n#note:abc\nb', itemsByKey);
    expect(shape(fragment)).toEqual([
      'a',
      '<br>',
      '<br>',
      '<span #note:abc>',
      BADGE_CARET_ANCHOR,
      '<br>',
      'b',
    ]);
  });
});
