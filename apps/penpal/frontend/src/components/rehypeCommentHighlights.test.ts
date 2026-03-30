import { describe, it, expect } from 'vitest';
import rehypeCommentHighlights, { nthIndexOf } from './rehypeCommentHighlights';
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

/** Build a <pre><code class="language-{lang}">...code...</code></pre> node with position */
function makePreCode(line: number, language: string, code: string): Element {
  return {
    type: 'element',
    tagName: 'pre',
    properties: {},
    children: [
      {
        type: 'element',
        tagName: 'code',
        properties: { className: [`language-${language}`] },
        children: [{ type: 'text', value: code } as Text],
        position: { start: { line, column: 1, offset: 0 }, end: { line: line + code.split('\n').length, column: 1, offset: code.length } },
      } as Element,
    ],
    position: { start: { line, column: 1, offset: 0 }, end: { line: line + code.split('\n').length + 1, column: 1, offset: code.length } },
  } as Element;
}

/** Build a <p data-source-line="N">...text...</p> element */
function makeParagraph(line: number, text: string): Element {
  return {
    type: 'element',
    tagName: 'p',
    properties: {},
    children: [{ type: 'text', value: text } as Text],
    position: { start: { line, column: 1, offset: 0 }, end: { line, column: 1 + text.length, offset: text.length } },
  } as Element;
}

/** Build a <p><img src="..."></p> block-level image paragraph */
function makeImgParagraph(line: number, src: string): Element {
  return {
    type: 'element',
    tagName: 'p',
    properties: {},
    children: [
      {
        type: 'element',
        tagName: 'img',
        properties: { src },
        children: [],
      } as Element,
    ],
    position: { start: { line, column: 1, offset: 0 }, end: { line, column: 1, offset: 0 } },
  } as Element;
}

/** Build a <p>text <img src="..."> text</p> with inline image */
function makeParagraphWithImg(line: number, textBefore: string, imgSrc: string, textAfter: string): Element {
  return {
    type: 'element',
    tagName: 'p',
    properties: {},
    children: [
      { type: 'text', value: textBefore } as Text,
      {
        type: 'element',
        tagName: 'img',
        properties: { src: imgSrc },
        children: [],
      } as Element,
      { type: 'text', value: textAfter } as Text,
    ],
    position: { start: { line, column: 1, offset: 0 }, end: { line, column: 1 + textBefore.length + textAfter.length, offset: textBefore.length + textAfter.length } },
  } as Element;
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

  it('does not insert marks inside fenced code blocks', () => {
    // Fenced code: <pre><code>...</code></pre>
    const tree: Root = {
      type: 'root',
      children: [
        {
          type: 'element',
          tagName: 'pre',
          properties: {},
          children: [
            {
              type: 'element',
              tagName: 'code',
              properties: { className: ['language-go'] },
              children: [{ type: 'text', value: 'func main() {}' } as Text],
              position: { start: { line: 2, column: 1, offset: 4 }, end: { line: 2, column: 16, offset: 19 } },
            } as Element,
          ],
          position: { start: { line: 1, column: 1, offset: 0 }, end: { line: 3, column: 4, offset: 23 } },
        } as Element,
      ],
    };
    const transform = rehypeCommentHighlights({
      highlights: [{ threadId: 't1', selectedText: 'func main', startLine: 1 }],
    });
    transform(tree);
    expect(findMarks(tree)).toHaveLength(0);
    // Text content should be unchanged
    const code = (tree.children[0] as Element).children[0] as Element;
    expect((code.children[0] as Text).value).toBe('func main() {}');
  });

  it('does not false-positive short prefix match on long elements', () => {
    // A long element ending with "H" should NOT trigger a cross-element match
    // for a highlight starting with "Hello"
    const tree = makeTree(1, 'Something that ends with H');
    const transform = rehypeCommentHighlights({
      highlights: [{ threadId: 't1', selectedText: 'Hello World', startLine: 1 }],
    });
    transform(tree);
    expect(findMarks(tree)).toHaveLength(0);
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

  // E-PENPAL-HIGHLIGHT-CROSS: cross-boundary code block tests
  describe('cross-boundary code block highlights', () => {
    it('prose → code: continuation stores dataCrossHighlights on <code> element', () => {
      // Paragraph followed by a fenced code block. Highlight starts in the
      // paragraph and continues into the code block.
      const tree: Root = {
        type: 'root',
        children: [
          makeParagraph(1, 'Some prose text'),
          makePreCode(3, 'js', 'function main() {\n  return 1;\n}\n'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'Some prose text function main() {',
          startLine: 1,
        }],
      });
      transform(tree);

      // Paragraph should have a <mark> for the prose portion
      const marks = findMarks(tree);
      expect(marks.length).toBeGreaterThanOrEqual(1);
      expect((marks[0].children[0] as Text).value).toBe('Some prose text');

      // Code element should have dataCrossHighlights with the code portion
      const pre = tree.children[1] as Element;
      const code = pre.children[0] as Element;
      const crossRaw = code.properties?.dataCrossHighlights;
      expect(crossRaw).toBeDefined();
      const crossHighlights = JSON.parse(String(crossRaw)) as { threadId: string; selectedText: string }[];
      expect(crossHighlights).toHaveLength(1);
      expect(crossHighlights[0].threadId).toBe('t1');
      expect(crossHighlights[0].selectedText).toBe('function main() {');
    });

    it('code → prose: highlight starting in code block continues into paragraph', () => {
      // Fenced code block followed by a paragraph. Highlight starts at the
      // code block's line and extends into the paragraph.
      const tree: Root = {
        type: 'root',
        children: [
          makePreCode(1, 'js', 'return 1;\n'),
          makeParagraph(4, 'Some following text'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'return 1; Some following text',
          startLine: 1,
        }],
      });
      transform(tree);

      // Code element should have dataCrossHighlights for the code portion
      const pre = tree.children[0] as Element;
      const code = pre.children[0] as Element;
      const crossRaw = code.properties?.dataCrossHighlights;
      expect(crossRaw).toBeDefined();
      const crossHighlights = JSON.parse(String(crossRaw)) as { threadId: string; selectedText: string }[];
      expect(crossHighlights).toHaveLength(1);
      expect(crossHighlights[0].threadId).toBe('t1');
      expect(crossHighlights[0].selectedText).toContain('return 1;');

      // Paragraph should have a <mark> for the prose portion
      const marks = findMarks(tree);
      expect(marks.length).toBeGreaterThanOrEqual(1);
      const proseMarks = marks.filter(m =>
        (m.children[0] as Text).value.includes('Some following text')
      );
      expect(proseMarks).toHaveLength(1);
    });

    it('code only: full match does not store dataCrossHighlights (MarkdownViewer handles it)', () => {
      const tree: Root = {
        type: 'root',
        children: [
          makePreCode(1, 'go', 'func main() {}\n'),
          makeParagraph(4, 'Unrelated text'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'func main',
          startLine: 1,
        }],
      });
      transform(tree);

      // Full code-only match: dataCrossHighlights should NOT be set.
      // MarkdownViewer's existing startLine filter handles these directly.
      const pre = tree.children[0] as Element;
      const code = pre.children[0] as Element;
      expect(code.properties?.dataCrossHighlights).toBeUndefined();

      // No marks in the HAST (SyntaxHighlighter handles rendering)
      expect(findMarks(tree)).toHaveLength(0);

      // Paragraph should NOT have any marks (no continuation)
      const p = tree.children[1] as Element;
      expect(findMarks(p)).toHaveLength(0);
    });

    it('mermaid blocks are not treated as code for cross-highlights', () => {
      const tree: Root = {
        type: 'root',
        children: [
          makeParagraph(1, 'Some text before'),
          makePreCode(3, 'mermaid', 'graph TD\n  A-->B\n'),
          makeParagraph(7, 'Some text after'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'Some text before Some text after',
          startLine: 1,
        }],
      });
      transform(tree);

      // Mermaid code element should NOT have dataCrossHighlights
      const pre = tree.children[1] as Element;
      const code = pre.children[0] as Element;
      expect(code.properties?.dataCrossHighlights).toBeUndefined();

      // Prose before should be highlighted
      const marks = findMarks(tree);
      expect(marks.length).toBeGreaterThanOrEqual(1);
    });

    it('does not insert marks inside syntax-highlighted code blocks', () => {
      // Existing behavior: <mark> elements in <pre><code class="language-*">
      // would break SyntaxHighlighter. Verify no marks are inserted.
      // Full code-only matches are handled by MarkdownViewer's startLine filter,
      // not by dataCrossHighlights.
      const tree: Root = {
        type: 'root',
        children: [makePreCode(1, 'go', 'func main() {}')],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{ threadId: 't1', selectedText: 'func main', startLine: 1 }],
      });
      transform(tree);
      expect(findMarks(tree)).toHaveLength(0);
    });

    it('language-less <pre> falls through to normal mark insertion', () => {
      // Language-less code blocks don't use SyntaxHighlighter, so <mark>
      // elements can be inserted directly.
      const pre: Element = {
        type: 'element',
        tagName: 'pre',
        properties: {},
        children: [
          {
            type: 'element',
            tagName: 'code',
            properties: {},
            children: [{ type: 'text', value: 'plain code block' } as Text],
            position: { start: { line: 1, column: 1, offset: 0 }, end: { line: 1, column: 17, offset: 16 } },
          } as Element,
        ],
        position: { start: { line: 1, column: 1, offset: 0 }, end: { line: 2, column: 1, offset: 17 } },
      } as Element;
      const tree: Root = { type: 'root', children: [pre] };
      const transform = rehypeCommentHighlights({
        highlights: [{ threadId: 't1', selectedText: 'plain code', startLine: 1 }],
      });
      transform(tree);
      const marks = findMarks(tree);
      expect(marks).toHaveLength(1);
      expect((marks[0].children[0] as Text).value).toBe('plain code');
    });

    it('code→prose prefix fallback stores partial cross-highlight', () => {
      // Highlight starts in code and the full selectedText only partially
      // matches the code (prefix match). The code portion should get a
      // dataCrossHighlights entry and remaining text continues into prose.
      const tree: Root = {
        type: 'root',
        children: [
          makePreCode(1, 'js', 'const x = 1;\n'),
          makeParagraph(4, 'And then we use x'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'const x = 1; And then we use x',
          startLine: 1,
        }],
      });
      transform(tree);

      // Code gets a cross-highlight for partial match
      const code = (tree.children[0] as Element).children[0] as Element;
      const crossRaw = code.properties?.dataCrossHighlights;
      expect(crossRaw).toBeDefined();
      const crossHighlights = JSON.parse(String(crossRaw)) as { threadId: string; selectedText: string }[];
      expect(crossHighlights).toHaveLength(1);
      expect(crossHighlights[0].selectedText).toContain('const x = 1;');

      // Prose gets a mark for the continuation
      const marks = findMarks(tree);
      expect(marks.length).toBeGreaterThanOrEqual(1);
    });

    it('occurrenceIndex targets correct match in code block', () => {
      // Code has "val" twice. occurrenceIndex=1 should match the second.
      const tree: Root = {
        type: 'root',
        children: [
          makePreCode(1, 'js', 'val = 1;\nval = 2;\n'),
          makeParagraph(5, 'After code'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'val = 2; After code',
          startLine: 2,
          occurrenceIndex: 1,
        }],
      });
      transform(tree);

      // Code cross-highlight should contain the second "val = 2;"
      const code = (tree.children[0] as Element).children[0] as Element;
      const crossRaw = code.properties?.dataCrossHighlights;
      expect(crossRaw).toBeDefined();
      const crossHighlights = JSON.parse(String(crossRaw)) as { threadId: string; selectedText: string }[];
      expect(crossHighlights[0].selectedText).toContain('val = 2;');

      // Prose continuation
      const marks = findMarks(tree);
      expect(marks.length).toBeGreaterThanOrEqual(1);
    });

    it('unmatched highlight in code block is not marked as applied (can match later)', () => {
      // A highlight whose selectedText doesn't match the code block at all
      // should NOT be marked as applied — it may match a subsequent element.
      const tree: Root = {
        type: 'root',
        children: [
          makePreCode(1, 'js', 'unrelated code\n'),
          makeParagraph(4, 'The actual matching text'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'The actual matching text',
          startLine: 1,
        }],
      });
      transform(tree);

      // The highlight should still render in the paragraph
      const marks = findMarks(tree);
      expect(marks.length).toBeGreaterThanOrEqual(1);
      expect((marks[0].children[0] as Text).value).toContain('The actual matching text');
    });
  });

  // E-PENPAL-HIGHLIGHT-MEDIA: media wrapping tests
  describe('media highlight wrapping', () => {
    it('inline image wrapped when between marks with same threadId', () => {
      // <p>Hello <img> world</p> — highlight covers "Hello" and "world"
      const tree: Root = {
        type: 'root',
        children: [makeParagraphWithImg(1, 'Hello ', 'test.png', ' world')],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{ threadId: 't1', selectedText: 'Hello  world', startLine: 1 }],
      });
      transform(tree);

      const marks = findMarks(tree);
      // Should have 3 marks: "Hello ", <img> wrapper, " world"
      expect(marks).toHaveLength(3);
      // The middle mark should wrap the img
      const imgMark = marks.find(m =>
        m.children.some(c => c.type === 'element' && (c as Element).tagName === 'img')
      );
      expect(imgMark).toBeDefined();
      expect((imgMark!.properties as Record<string, unknown>).dataThreadId).toBe('t1');
    });

    it('block-level image wrapped during continuation', () => {
      // <p>First</p> <p><img></p> <p>Last</p>
      const tree: Root = {
        type: 'root',
        children: [
          makeParagraph(1, 'First paragraph text'),
          makeImgParagraph(3, 'diagram.png'),
          makeParagraph(5, 'Last paragraph text'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'First paragraph text Last paragraph text',
          startLine: 1,
        }],
      });
      transform(tree);

      // The image paragraph should be wrapped in a <mark>
      const rootChildren = tree.children;
      const imgContainer = rootChildren[1];
      expect(imgContainer.type).toBe('element');
      expect((imgContainer as Element).tagName).toBe('mark');
      expect((imgContainer as Element).properties?.dataThreadId).toBe('t1');
      // The <p><img></p> should be inside the mark
      const innerP = (imgContainer as Element).children[0] as Element;
      expect(innerP.tagName).toBe('p');
      expect((innerP.children[0] as Element).tagName).toBe('img');

      // First and last paragraphs should also have marks
      const marks = findMarks(tree);
      expect(marks.length).toBeGreaterThanOrEqual(2);
    });

    it('mermaid block annotated (not wrapped) during continuation', () => {
      // <p>Before</p> <pre><code class="language-mermaid">...</code></pre> <p>After</p>
      // Mermaid blocks can't be wrapped in <mark> because it changes tree structure,
      // causing React to recreate DOM nodes and lose imperatively-rendered SVG.
      // Instead, the <code> element gets a dataMermaidHighlight annotation.
      const tree: Root = {
        type: 'root',
        children: [
          makeParagraph(1, 'Before mermaid text'),
          makePreCode(3, 'mermaid', 'graph TD\n  A-->B\n'),
          makeParagraph(7, 'After mermaid text'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'Before mermaid text After mermaid text',
          startLine: 1,
        }],
      });
      transform(tree);

      // The mermaid <pre> should NOT be wrapped — tree structure unchanged
      const pre = tree.children[1] as Element;
      expect(pre.tagName).toBe('pre');

      // The <code> element should have dataMermaidHighlight annotation
      const code = pre.children[0] as Element;
      const raw = code.properties?.dataMermaidHighlight;
      expect(raw).toBeDefined();
      const parsed = JSON.parse(String(raw)) as { threadId: string; pending?: boolean };
      expect(parsed.threadId).toBe('t1');

      // Before and after paragraphs should also have marks
      const proseMarks = findMarks(tree).filter(m =>
        m.children.some(c => c.type === 'text')
      );
      expect(proseMarks.length).toBeGreaterThanOrEqual(2);
    });

    it('image NOT wrapped when highlight is only on one side', () => {
      // <p>Hello <img> world</p> — highlight covers only "Hello"
      const tree: Root = {
        type: 'root',
        children: [makeParagraphWithImg(1, 'Hello ', 'test.png', ' world')],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{ threadId: 't1', selectedText: 'Hello', startLine: 1 }],
      });
      transform(tree);

      const marks = findMarks(tree);
      // Should have 1 mark for "Hello" only — img should NOT be wrapped
      expect(marks).toHaveLength(1);
      expect((marks[0].children[0] as Text).value).toBe('Hello');

      // The img should still be a direct child of the <p>, not wrapped in <mark>
      const p = tree.children[0] as Element;
      const hasDirectImg = p.children.some(
        c => c.type === 'element' && (c as Element).tagName === 'img'
      );
      expect(hasDirectImg).toBe(true);
    });

    it('mermaid annotation carries pending flag', () => {
      const tree: Root = {
        type: 'root',
        children: [
          makeParagraph(1, 'Before text'),
          makePreCode(3, 'mermaid', 'graph TD\n  A-->B\n'),
          makeParagraph(7, 'After text'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'Before text After text',
          startLine: 1,
          pending: true,
        }],
      });
      transform(tree);

      const pre = tree.children[1] as Element;
      const code = pre.children[0] as Element;
      const parsed = JSON.parse(String(code.properties?.dataMermaidHighlight)) as { threadId: string; pending?: boolean };
      expect(parsed.pending).toBe(true);
    });

    it('continuation works past mermaid when selectedText has SVG text', () => {
      // Real scenario: user selects from prose through a mermaid diagram to prose
      // after. Selection captures SVG node labels (not mermaid source), so the
      // remaining after matching the first paragraph contains junk SVG text
      // before the post-mermaid paragraph text.
      const tree: Root = {
        type: 'root',
        children: [
          makeParagraph(1, 'Before mermaid text'),
          makePreCode(3, 'mermaid', 'graph TD\n  A[Start]-->B[End]\n'),
          makeParagraph(7, 'After mermaid text here'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          // Simulates sel.toString(): prose + SVG labels + post-prose
          selectedText: 'Before mermaid text Start End After mermaid text here',
          startLine: 1,
        }],
      });
      transform(tree);

      // First paragraph should be highlighted
      const firstPMarks = findMarks(tree.children[0] as Element);
      expect(firstPMarks.length).toBeGreaterThanOrEqual(1);

      // Mermaid block should be annotated
      const pre = tree.children[1] as Element;
      const code = pre.children[0] as Element;
      expect(code.properties?.dataMermaidHighlight).toBeDefined();

      // Last paragraph should also be highlighted via continuation
      const lastP = tree.children[2];
      // lastP might be wrapped in <mark> (block-level) or have inner marks
      const lastPMarks = lastP.type === 'element' && (lastP as Element).tagName === 'mark'
        ? [lastP as Element]
        : findMarks(lastP as Element);
      expect(lastPMarks.length).toBeGreaterThanOrEqual(1);
    });

    it('short text after mermaid is highlighted via mermaidCrossed flag', () => {
      // When SVG text is long relative to post-mermaid text, the normal
      // Strategy 3 thresholds reject the match. mermaidCrossed lowers them.
      const tree: Root = {
        type: 'root',
        children: [
          makeParagraph(1, 'Intro text'),
          makePreCode(3, 'mermaid', 'graph TD\n  A[Login]-->B[Dashboard]-->C[Settings]-->D[Profile]\n'),
          makeParagraph(8, 'Done'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          // SVG labels are long, post-mermaid text is short
          selectedText: 'Intro text Login Dashboard Settings Profile Done',
          startLine: 1,
        }],
      });
      transform(tree);

      const lastP = tree.children[2];
      const lastPMarks = lastP.type === 'element' && (lastP as Element).tagName === 'mark'
        ? [lastP as Element]
        : findMarks(lastP as Element);
      expect(lastPMarks.length).toBeGreaterThanOrEqual(1);
    });

    it('mermaid block NOT annotated when no continuation is active', () => {
      // Mermaid block as first element — no highlight spans into it
      const tree: Root = {
        type: 'root',
        children: [
          makePreCode(1, 'mermaid', 'graph TD\n  A-->B\n'),
          makeParagraph(5, 'Some text after'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{ threadId: 't1', selectedText: 'Some text after', startLine: 5 }],
      });
      transform(tree);

      // Mermaid should NOT have annotation
      const pre = tree.children[0] as Element;
      expect(pre.tagName).toBe('pre');
      const code = pre.children[0] as Element;
      expect(code.properties?.dataMermaidHighlight).toBeUndefined();

      // Paragraph should have a mark
      const marks = findMarks(tree);
      expect(marks).toHaveLength(1);
    });

    it('pending class propagated to media wrapping marks', () => {
      const tree: Root = {
        type: 'root',
        children: [
          makeParagraph(1, 'Before text'),
          makeImgParagraph(3, 'photo.png'),
          makeParagraph(5, 'After text'),
        ],
      };
      const transform = rehypeCommentHighlights({
        highlights: [{
          threadId: 't1',
          selectedText: 'Before text After text',
          startLine: 1,
          pending: true,
        }],
      });
      transform(tree);

      // The image paragraph wrapper should have pending class
      const imgContainer = tree.children[1] as Element;
      expect(imgContainer.tagName).toBe('mark');
      expect((imgContainer.properties?.className as string[])).toContain('pending-highlight');
    });
  });

  describe('nthIndexOf', () => {
    it('returns first occurrence for index 0', () => {
      expect(nthIndexOf('foo bar foo baz foo', 'foo', 0)).toBe(0);
    });

    it('returns second occurrence for index 1', () => {
      expect(nthIndexOf('foo bar foo baz foo', 'foo', 1)).toBe(8);
    });

    it('returns third occurrence for index 2', () => {
      expect(nthIndexOf('foo bar foo baz foo', 'foo', 2)).toBe(16);
    });

    it('falls back to first occurrence when target index exceeds count', () => {
      expect(nthIndexOf('foo bar foo', 'foo', 5)).toBe(0);
    });

    it('returns -1 when search text not found', () => {
      expect(nthIndexOf('foo bar', 'baz', 0)).toBe(-1);
    });
  });
});
