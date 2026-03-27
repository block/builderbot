import { describe, it, expect } from 'vitest';
import rehypeCommentHighlights from './rehypeCommentHighlights';
import type { Root, Element, Text } from 'hast';

/** Build a minimal HAST tree: <p data-source-line="N">...text...</p> */
function makeTree(line: number, text: string): Root {
  return {
    type: 'root',
    children: [
      {
        type: 'element',
        tagName: 'p',
        properties: {},
        children: [{ type: 'text', value: text } as Text],
        position: { start: { line, column: 1, offset: 0 }, end: { line, column: 1 + text.length, offset: text.length } },
      } as Element,
    ],
  };
}

/** Find all <mark> elements in a tree */
function findMarks(node: Root | Element): Element[] {
  const marks: Element[] = [];
  for (const child of ('children' in node ? node.children : [])) {
    if (child.type === 'element') {
      if (child.tagName === 'mark') marks.push(child);
      marks.push(...findMarks(child));
    }
  }
  return marks;
}

// E-PENPAL-HIGHLIGHT-REHYPE: verifies <mark> injection, pending class, line matching, and text splitting.
describe('rehypeCommentHighlights', () => {
  it('wraps matching text in a <mark> with comment-highlight class', () => {
    const tree = makeTree(1, 'Hello world');
    const transform = rehypeCommentHighlights({
      highlights: [{ threadId: 't1', selectedText: 'world', startLine: 1 }],
    });
    transform(tree);
    const marks = findMarks(tree);
    expect(marks).toHaveLength(1);
    expect((marks[0].properties as Record<string, unknown>).className).toEqual(['comment-highlight']);
    expect((marks[0].properties as Record<string, unknown>).dataThreadId).toBe('t1');
    expect((marks[0].children[0] as Text).value).toBe('world');
  });

  it('does not wrap text when selectedText is not found', () => {
    const tree = makeTree(1, 'Hello world');
    const transform = rehypeCommentHighlights({
      highlights: [{ threadId: 't1', selectedText: 'missing', startLine: 1 }],
    });
    transform(tree);
    expect(findMarks(tree)).toHaveLength(0);
  });

  it('does not wrap text when startLine does not match', () => {
    const tree = makeTree(1, 'Hello world');
    const transform = rehypeCommentHighlights({
      highlights: [{ threadId: 't1', selectedText: 'world', startLine: 99 }],
    });
    transform(tree);
    expect(findMarks(tree)).toHaveLength(0);
  });

  it('adds pending-highlight class when pending is true', () => {
    const tree = makeTree(1, 'Hello world');
    const transform = rehypeCommentHighlights({
      highlights: [{ threadId: 'pending', selectedText: 'world', startLine: 1, pending: true }],
    });
    transform(tree);
    const marks = findMarks(tree);
    expect(marks).toHaveLength(1);
    expect((marks[0].properties as Record<string, unknown>).className).toEqual(['comment-highlight', 'pending-highlight']);
  });

  it('does not add pending-highlight class when pending is false/undefined', () => {
    const tree = makeTree(1, 'Hello world');
    const transform = rehypeCommentHighlights({
      highlights: [{ threadId: 't1', selectedText: 'world', startLine: 1 }],
    });
    transform(tree);
    const marks = findMarks(tree);
    expect(marks).toHaveLength(1);
    expect((marks[0].properties as Record<string, unknown>).className).toEqual(['comment-highlight']);
  });

  it('splits text node correctly around the match', () => {
    const tree = makeTree(1, 'The quick brown fox');
    const transform = rehypeCommentHighlights({
      highlights: [{ threadId: 't1', selectedText: 'quick', startLine: 1 }],
    });
    transform(tree);
    const p = tree.children[0] as Element;
    // Should be: "The " + <mark>quick</mark> + " brown fox"
    expect(p.children).toHaveLength(3);
    expect((p.children[0] as Text).value).toBe('The ');
    expect((p.children[1] as Element).tagName).toBe('mark');
    expect((p.children[2] as Text).value).toBe(' brown fox');
  });

  it('returns tree unchanged when highlights array is empty', () => {
    const tree = makeTree(1, 'Hello world');
    const original = JSON.stringify(tree);
    const transform = rehypeCommentHighlights({ highlights: [] });
    transform(tree);
    expect(JSON.stringify(tree)).toBe(original);
  });

  it('handles multiple highlights on different lines', () => {
    const tree: Root = {
      type: 'root',
      children: [
        {
          type: 'element',
          tagName: 'p',
          properties: {},
          children: [{ type: 'text', value: 'First line' } as Text],
          position: { start: { line: 1, column: 1, offset: 0 }, end: { line: 1, column: 11, offset: 10 } },
        } as Element,
        {
          type: 'element',
          tagName: 'p',
          properties: {},
          children: [{ type: 'text', value: 'Second line' } as Text],
          position: { start: { line: 3, column: 1, offset: 12 }, end: { line: 3, column: 12, offset: 23 } },
        } as Element,
      ],
    };
    const transform = rehypeCommentHighlights({
      highlights: [
        { threadId: 't1', selectedText: 'First', startLine: 1 },
        { threadId: 't2', selectedText: 'Second', startLine: 3 },
      ],
    });
    transform(tree);
    const marks = findMarks(tree);
    expect(marks).toHaveLength(2);
    expect((marks[0].properties as Record<string, unknown>).dataThreadId).toBe('t1');
    expect((marks[1].properties as Record<string, unknown>).dataThreadId).toBe('t2');
  });
});
