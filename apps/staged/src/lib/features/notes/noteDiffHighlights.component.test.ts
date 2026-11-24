import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { highlightMatches } from '../../shared/textHighlight';
import { computeNoteDiff, type NoteDiff } from './noteDiff';
import {
  applyNoteDiffMarks,
  clearNoteDiffMarks,
  NOTE_DIFF_ACTIVE_ATTRIBUTE,
  NOTE_DIFF_ADDED_HIGHLIGHT,
  NOTE_DIFF_ATTRIBUTE,
  NOTE_DIFF_HUNK_ATTRIBUTE,
  NOTE_DIFF_REMOVED_HIGHLIGHT,
  noteDiffHunkRanges,
  paintNoteDiffWords,
  setActiveNoteDiffHunk,
} from './noteDiffHighlights';
import { extractNoteDiffUnits, type NoteDiffDomUnit } from './noteDiffUnits';
import { renderNoteMarkdown } from './noteMarkdown';

const PREVIOUS = [
  '# Cache design',
  '',
  'The cache is **warmed** when the app starts.',
  '',
  'Entries are evicted oldest first, a hundred at a time.',
  '',
  'This paragraph will be dropped.',
  '',
  'Nothing is written to disk.',
  '',
  'See the [design doc](https://example.com/v1) for more.',
  '',
  'Logs are kept for a week.',
  '',
  '## Limits',
].join('\n');

const CURRENT = [
  '# Cache design',
  '',
  'The cache is **warmed** when the app starts.',
  '',
  'A new paragraph about thumbnails.',
  '',
  'Entries are evicted **newest** first,',
  'a thousand at a time.',
  '',
  'Nothing is written to disk.',
  '',
  'See the [design doc](https://example.com/v2) for more.',
  '',
  'Retention is set by policy, per workspace, and audited.',
  '',
  '## Limits',
].join('\n');

interface Scene {
  previousRoot: HTMLElement;
  currentRoot: HTMLElement;
  previous: NoteDiffDomUnit[];
  current: NoteDiffDomUnit[];
  diff: NoteDiff;
}

/** Both versions in the page, as the split view has them, and their diff. */
function scene(previousMarkdown = PREVIOUS, currentMarkdown = CURRENT): Scene {
  const pane = (markdown: string) => {
    const root = document.createElement('div');
    root.className = 'markdown-content';
    root.innerHTML = renderNoteMarkdown(markdown);
    return root;
  };
  const previousRoot = pane(previousMarkdown);
  const currentRoot = pane(currentMarkdown);
  document.body.replaceChildren(previousRoot, currentRoot);
  const previous = extractNoteDiffUnits(previousRoot);
  const current = extractNoteDiffUnits(currentRoot);
  return { previousRoot, currentRoot, previous, current, diff: computeNoteDiff(previous, current) };
}

/** Each marked block under `root`: its text's first word, its own change and its hunk. */
function marks(root: Element): [string, string | null, string | null][] {
  return [...root.querySelectorAll(`[${NOTE_DIFF_ATTRIBUTE}]`)].map((element) => [
    (element.textContent ?? '').trim().split(/\s+/)[0],
    element.getAttribute(NOTE_DIFF_ATTRIBUTE),
    element.getAttribute(NOTE_DIFF_HUNK_ATTRIBUTE),
  ]);
}

function active(root: Element): string[] {
  return [...root.querySelectorAll(`[${NOTE_DIFF_ACTIVE_ATTRIBUTE}]`)].map(
    (element) => (element.textContent ?? '').trim().split(/\s+/)[0]
  );
}

function texts(ranges: Range[]): string[] {
  return ranges.map((range) => range.toString());
}

beforeEach(() => {
  document.body.replaceChildren();
  vi.spyOn(console, 'info').mockImplementation(() => {});
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('the fixture', () => {
  it('holds two hunks: an addition, a rewording and a removal, then a new link and a replaced paragraph', () => {
    const { diff } = scene();

    expect(diff.hunks.map((hunk) => [hunk.id, hunk.kind])).toEqual([
      ['hunk-0', 'modified'],
      ['hunk-1', 'modified'],
    ]);
    expect(diff.hunks[0].ops.map((op) => op.type)).toEqual(['added', 'modified', 'removed']);
    expect(diff.hunks[1].ops.map((op) => op.type)).toEqual(['modified', 'removed', 'added']);
  });
});

describe('applyNoteDiffMarks', () => {
  // Both hunks are modified ones, and their markers say so. A block is
  // marked as the code diff viewer classes a line: by its own change.
  it('marks the current version’s changed blocks with their own change and their hunk’s id', () => {
    const { currentRoot, current, diff } = scene();

    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');

    expect(marks(currentRoot)).toEqual([
      ['A', 'added', 'hunk-0'],
      ['Entries', 'modified', 'hunk-0'],
      ['See', 'modified', 'hunk-1'],
      ['Retention', 'added', 'hunk-1'],
    ]);
  });

  it('marks the previous version’s changed blocks on its side', () => {
    const { previousRoot, previous, diff } = scene();

    applyNoteDiffMarks(previousRoot, previous, diff.hunks, 'previous');

    expect(marks(previousRoot)).toEqual([
      ['Entries', 'modified', 'hunk-0'],
      ['This', 'removed', 'hunk-0'],
      ['See', 'modified', 'hunk-1'],
      ['Logs', 'removed', 'hunk-1'],
    ]);
  });

  it('never marks a block as added in the previous version or removed in the note', () => {
    const { previousRoot, currentRoot, previous, current, diff } = scene();

    applyNoteDiffMarks(previousRoot, previous, diff.hunks, 'previous');
    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');

    expect(previousRoot.querySelector(`[${NOTE_DIFF_ATTRIBUTE}='added']`)).toBeNull();
    expect(currentRoot.querySelector(`[${NOTE_DIFF_ATTRIBUTE}='removed']`)).toBeNull();
  });

  it('marks additions and removals in their own colours, and nothing for the side without them', () => {
    const { previousRoot, currentRoot, previous, current, diff } = scene(
      'One.\n\nTwo will go.\n\nThree.',
      'One.\n\nThree.\n\nFour is new.'
    );

    applyNoteDiffMarks(previousRoot, previous, diff.hunks, 'previous');
    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');

    expect(marks(previousRoot)).toEqual([['Two', 'removed', 'hunk-0']]);
    expect(marks(currentRoot)).toEqual([['Four', 'added', 'hunk-1']]);
  });

  it('marks a list item and a table row, not the list or the table', () => {
    const { currentRoot, current, diff } = scene(
      '- alpha one\n- beta two\n\n| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |',
      '- alpha one\n- beta three\n\n| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 5 |'
    );

    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');

    expect(
      [...currentRoot.querySelectorAll(`[${NOTE_DIFF_ATTRIBUTE}]`)].map(
        (element) => element.tagName
      )
    ).toEqual(['LI', 'TR']);
  });

  it('leaves the note’s nodes as they were', () => {
    const { currentRoot, current, diff } = scene();
    const before = currentRoot.innerHTML;

    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');
    setActiveNoteDiffHunk(currentRoot, 'hunk-0');
    clearNoteDiffMarks(currentRoot);

    expect(currentRoot.innerHTML).toBe(before);
  });

  it('replaces the marks of an earlier diff', () => {
    const { currentRoot, current, diff } = scene();
    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');
    setActiveNoteDiffHunk(currentRoot, 'hunk-0');

    applyNoteDiffMarks(currentRoot, current, diff.hunks.slice(1), 'current');

    expect(marks(currentRoot)).toEqual([
      ['See', 'modified', 'hunk-1'],
      ['Retention', 'added', 'hunk-1'],
    ]);
    expect(active(currentRoot)).toEqual([]);
  });
});

describe('clearNoteDiffMarks', () => {
  it('takes every diff attribute off', () => {
    const { currentRoot, current, diff } = scene();
    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');
    setActiveNoteDiffHunk(currentRoot, 'hunk-1');

    clearNoteDiffMarks(currentRoot);

    expect(marks(currentRoot)).toEqual([]);
    expect(currentRoot.querySelector(`[${NOTE_DIFF_HUNK_ATTRIBUTE}]`)).toBeNull();
    expect(active(currentRoot)).toEqual([]);
  });
});

describe('setActiveNoteDiffHunk', () => {
  it('lights one hunk’s blocks at a time', () => {
    const { currentRoot, current, diff } = scene();
    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');

    setActiveNoteDiffHunk(currentRoot, 'hunk-0');
    expect(active(currentRoot)).toEqual(['A', 'Entries']);

    setActiveNoteDiffHunk(currentRoot, 'hunk-1');
    expect(active(currentRoot)).toEqual(['See', 'Retention']);

    setActiveNoteDiffHunk(currentRoot, null);
    expect(active(currentRoot)).toEqual([]);
  });

  it('lights nothing for a hunk the note does not hold', () => {
    const { currentRoot, current, diff } = scene();
    applyNoteDiffMarks(currentRoot, current, diff.hunks, 'current');

    setActiveNoteDiffHunk(currentRoot, 'hunk-9');

    expect(active(currentRoot)).toEqual([]);
  });
});

describe('noteDiffHunkRanges', () => {
  it('covers the words that differ in a modified block, and nothing of a block only one side has', () => {
    const { previous, current, diff } = scene();
    const [hunk] = diff.hunks;

    // The reworded paragraph's words run through bold text and a line break.
    // The new paragraph before it and the dropped one after are whole
    // changes, coloured as blocks: no words are picked out of them.
    expect(texts(noteDiffHunkRanges(current, hunk, 'current'))).toEqual(['newest', 'thousand']);
    expect(texts(noteDiffHunkRanges(previous, hunk, 'previous'))).toEqual(['oldest', 'hundred']);
  });

  it('covers the whole text of a block that changed in its link alone', () => {
    const { previous, current, diff } = scene();
    const hunk = diff.hunks[1];

    expect(texts(noteDiffHunkRanges(current, hunk, 'current'))).toEqual([
      'See the design doc for more.',
    ]);
    expect(texts(noteDiffHunkRanges(previous, hunk, 'previous'))).toEqual([
      'See the design doc for more.',
    ]);
  });

  it('starts and ends its ranges inside the text nodes that hold the words', () => {
    const { current, diff } = scene();
    const [newest] = noteDiffHunkRanges(current, diff.hunks[0], 'current');

    expect(newest.startContainer.nodeType).toBe(Node.TEXT_NODE);
    expect(newest.startContainer.parentElement?.tagName).toBe('STRONG');
    expect(newest.endContainer).toBe(newest.startContainer);
  });

  it('covers nothing in a hunk of additions alone', () => {
    const { current, diff } = scene('One.\n\nThree.', 'One.\n\nTwo is new.\n\nThree.');

    expect(diff.hunks[0].kind).toBe('added');
    expect(noteDiffHunkRanges(current, diff.hunks[0], 'current')).toEqual([]);
  });

  it('covers nothing for a block with no text', () => {
    const { current, diff } = scene('One.\n\nTwo.\n\nThree.', 'One.\n\n---\n\nThree.');

    expect(diff.hunks[0].ops.map((op) => op.type)).toEqual(['removed', 'added']);
    expect(noteDiffHunkRanges(current, diff.hunks[0], 'current')).toEqual([]);
  });

  it('finds the same words after marks have been put around the note’s text', () => {
    const { currentRoot, current, diff } = scene();

    // Find-in-note splits text nodes around its matches, one of them a changed word.
    expect(highlightMatches(currentRoot, 'new', 0).total).toBe(2);

    expect(texts(noteDiffHunkRanges(current, diff.hunks[0], 'current'))).toEqual([
      'newest',
      'thousand',
    ]);
  });

  it('covers nothing in a block whose text is no longer what was compared', () => {
    const { current, diff } = scene();
    const reworded = current.find((unit) => unit.text.startsWith('Entries'));
    reworded!.element.textContent = 'Entries are evicted at random.';

    expect(noteDiffHunkRanges(current, diff.hunks[0], 'current')).toEqual([]);
  });

  it('ends a range at the last word before a line break, not at the break', () => {
    const { current, diff } = scene(
      'Alpha beta gamma delta epsilon zeta eta theta.',
      'Alpha beta gamma delta epsilon\nomega zeta eta theta.'
    );
    const ranges = noteDiffHunkRanges(current, diff.hunks[0], 'current');

    // The added text is the break's space and a word; only the word is in a text node.
    expect(texts(ranges)).toEqual(['omega']);
    expect(ranges[0].startContainer.nodeType).toBe(Node.TEXT_NODE);
  });
});

describe('paintNoteDiffWords', () => {
  class FakeHighlight {
    ranges: Range[];
    constructor(...ranges: Range[]) {
      this.ranges = ranges;
    }
  }

  function stubHighlights() {
    const registry = new Map<string, FakeHighlight>();
    vi.stubGlobal('CSS', { highlights: registry });
    vi.stubGlobal('Highlight', FakeHighlight);
    return registry;
  }

  it('registers the current version’s words as added and the previous one’s as removed', () => {
    const registry = stubHighlights();
    const { previous, current, diff } = scene();
    const added = noteDiffHunkRanges(current, diff.hunks[0], 'current');
    const removed = noteDiffHunkRanges(previous, diff.hunks[0], 'previous');

    expect(paintNoteDiffWords('current', added)).toBe(true);
    expect(paintNoteDiffWords('previous', removed)).toBe(true);

    expect([...registry.keys()].sort()).toEqual([
      NOTE_DIFF_ADDED_HIGHLIGHT,
      NOTE_DIFF_REMOVED_HIGHLIGHT,
    ]);
    expect(registry.get(NOTE_DIFF_ADDED_HIGHLIGHT)?.ranges).toEqual(added);
    expect(registry.get(NOTE_DIFF_REMOVED_HIGHLIGHT)?.ranges).toEqual(removed);
  });

  it('replaces what a side had painted, and clears it when given nothing', () => {
    const registry = stubHighlights();
    const { previous, current, diff } = scene();
    paintNoteDiffWords('previous', noteDiffHunkRanges(previous, diff.hunks[0], 'previous'));
    paintNoteDiffWords('current', noteDiffHunkRanges(current, diff.hunks[0], 'current'));

    const next = noteDiffHunkRanges(current, diff.hunks[1], 'current');
    paintNoteDiffWords('current', next);
    expect(registry.get(NOTE_DIFF_ADDED_HIGHLIGHT)?.ranges).toEqual(next);

    paintNoteDiffWords('current', []);
    expect(registry.has(NOTE_DIFF_ADDED_HIGHLIGHT)).toBe(false);
    // The other side is left alone.
    expect(registry.has(NOTE_DIFF_REMOVED_HIGHLIGHT)).toBe(true);
  });

  it('reports that it cannot paint in a runtime without the highlight API', () => {
    vi.stubGlobal('CSS', undefined);
    const { current, diff } = scene();

    expect(
      paintNoteDiffWords('current', noteDiffHunkRanges(current, diff.hunks[0], 'current'))
    ).toBe(false);

    vi.stubGlobal('CSS', { highlights: new Map() });
    vi.stubGlobal('Highlight', undefined);
    expect(paintNoteDiffWords('current', [])).toBe(false);
  });
});
