import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import MarkdownViewer from './MarkdownViewer';

// E-PENPAL-MD-RENDER: verifies data-source-line, heading IDs, mermaid containers, GFM tables.
// E-PENPAL-HIGHLIGHT-REHYPE: verifies comment highlights and pending highlights via rehype plugin.
describe('MarkdownViewer', () => {
  it('renders markdown content', () => {
    const md = '# Hello World\n\nThis is a paragraph.';
    render(<MarkdownViewer content={md} rawMarkdown={md} />);
    expect(screen.getByText('Hello World')).toBeDefined();
    expect(screen.getByText('This is a paragraph.')).toBeDefined();
  });

  it('renders with content div id', () => {
    const md = 'Test content';
    const { container } = render(<MarkdownViewer content={md} rawMarkdown={md} />);
    expect(container.querySelector('#content')).toBeDefined();
  });

  it('generates heading IDs', () => {
    const md = '# My Heading\n\n## Sub Heading';
    const { container } = render(<MarkdownViewer content={md} rawMarkdown={md} />);
    const h1 = container.querySelector('h1');
    expect(h1?.id).toBe('penpal-md-my-heading');
    const h2 = container.querySelector('h2');
    expect(h2?.id).toBe('penpal-md-sub-heading');
  });

  it('calls onHeadingsExtracted with headings', async () => {
    const onHeadings = vi.fn();
    const md = '# Title\n\n## Section\n\n### Subsection';
    render(
      <MarkdownViewer content={md} rawMarkdown={md} onHeadingsExtracted={onHeadings} />,
    );
    // Wait for useEffect
    await new Promise((r) => setTimeout(r, 10));
    expect(onHeadings).toHaveBeenCalled();
    const headings = onHeadings.mock.calls[0][0];
    expect(headings.length).toBe(3);
    expect(headings[0]).toMatchObject({ level: 1, text: 'Title' });
    expect(headings[1]).toMatchObject({ level: 2, text: 'Section' });
    expect(headings[2]).toMatchObject({ level: 3, text: 'Subsection' });
  });

  it('adds data-source-line attributes to block elements from AST positions', () => {
    const md = '# Heading\n\nParagraph text here.\n\n- List item';
    const { container } = render(<MarkdownViewer content={md} rawMarkdown={md} />);
    const withSourceLine = container.querySelectorAll('[data-source-line]');
    expect(withSourceLine.length).toBeGreaterThan(0);
    // Verify specific line numbers match remark AST positions
    const heading = container.querySelector('h1');
    expect(heading?.getAttribute('data-source-line')).toBe('1');
    const paragraph = container.querySelector('p');
    expect(paragraph?.getAttribute('data-source-line')).toBe('3');
    const listItem = container.querySelector('li');
    expect(listItem?.getAttribute('data-source-line')).toBe('5');
  });

  it('renders GFM tables', () => {
    const md = '| Col1 | Col2 |\n|------|------|\n| A | B |';
    const { container } = render(<MarkdownViewer content={md} rawMarkdown={md} />);
    expect(container.querySelector('table')).toBeDefined();
    expect(container.querySelector('th')?.textContent).toBe('Col1');
  });

  it('renders code blocks', () => {
    const md = '```js\nconsole.log("hello")\n```';
    const { container } = render(<MarkdownViewer content={md} rawMarkdown={md} />);
    expect(container.querySelector('pre')).toBeDefined();
    expect(container.querySelector('code')).toBeDefined();
  });

  it('creates mermaid containers for mermaid code blocks', () => {
    const md = '```mermaid\ngraph TD\n  A --> B\n```';
    const { container } = render(<MarkdownViewer content={md} rawMarkdown={md} />);
    const mermaidContainer = container.querySelector('.mermaid-container');
    expect(mermaidContainer).toBeDefined();
    expect(mermaidContainer?.getAttribute('data-mermaid-source')).toContain('graph TD');
  });

  it('renders comment highlights via rehype plugin', () => {
    const md = 'Hello world';
    const highlights = [
      { threadId: 't1', selectedText: 'world', startLine: 1 },
    ];
    const { container } = render(
      <MarkdownViewer content={md} rawMarkdown={md} highlights={highlights} />,
    );
    const mark = container.querySelector('mark.comment-highlight');
    expect(mark).not.toBeNull();
    expect(mark?.textContent).toBe('world');
    expect(mark?.getAttribute('data-thread-id')).toBe('t1');
  });

  it('renders highlights inside fenced code blocks via custom renderer', () => {
    // ``` fence is line 1, code content starts at line 2
    const md = '```go\nfunc main() {}\n```';
    const highlights = [
      { threadId: 't-code', selectedText: 'func main', startLine: 2 },
    ];
    const { container } = render(
      <MarkdownViewer content={md} rawMarkdown={md} highlights={highlights} />,
    );
    // SyntaxHighlighter tokenizes "func" and "main" separately, so there may
    // be multiple <mark> elements covering the full match.
    const marks = container.querySelectorAll('mark.comment-highlight[data-thread-id="t-code"]');
    expect(marks.length).toBeGreaterThan(0);
    const combinedText = Array.from(marks).map(m => m.textContent).join('');
    expect(combinedText).toBe('func main');
  });

  it('renders highlights when startLine equals the fence line', () => {
    // Backend may resolve anchor to the opening ``` fence (line 1)
    const md = '```go\nfunc main() {}\n```';
    const highlights = [
      { threadId: 't-fence', selectedText: 'func main', startLine: 1 },
    ];
    const { container } = render(
      <MarkdownViewer content={md} rawMarkdown={md} highlights={highlights} />,
    );
    const marks = container.querySelectorAll('mark.comment-highlight[data-thread-id="t-fence"]');
    expect(marks.length).toBeGreaterThan(0);
    const combinedText = Array.from(marks).map(m => m.textContent).join('');
    expect(combinedText).toBe('func main');
  });

  // E-PENPAL-HIGHLIGHT-CROSS: cross-boundary highlight renders in code block
  it('renders cross-boundary highlight spanning prose and code block', () => {
    const md = 'Some text\n\n```js\nreturn 1;\n```';
    const highlights = [
      { threadId: 't-cross', selectedText: 'Some text return 1;', startLine: 1 },
    ];
    const { container } = render(
      <MarkdownViewer content={md} rawMarkdown={md} highlights={highlights} />,
    );
    // Prose portion should be highlighted
    const proseMarks = container.querySelectorAll('p mark.comment-highlight');
    expect(proseMarks.length).toBeGreaterThan(0);
    // Code portion should also be highlighted (via dataCrossHighlights → renderer)
    const codeMarks = container.querySelectorAll('pre mark.comment-highlight[data-thread-id="t-cross"]');
    expect(codeMarks.length).toBeGreaterThan(0);
  });

  // E-PENPAL-MD-STABLE-COMPONENTS: mermaid containers survive highlight changes
  it('preserves mermaid containers when highlights change', () => {
    const md = '```mermaid\ngraph TD\n  A --> B\n```\n\nSome text here';
    const { container, rerender } = render(
      <MarkdownViewer content={md} rawMarkdown={md} />,
    );
    const mermaidBefore = container.querySelector('.mermaid-container');
    expect(mermaidBefore).not.toBeNull();

    // Re-render with highlights — mermaid container must be the same DOM node
    const highlights = [
      { threadId: 't1', selectedText: 'Some text', startLine: 6 },
    ];
    rerender(
      <MarkdownViewer content={md} rawMarkdown={md} highlights={highlights} />,
    );
    const mermaidAfter = container.querySelector('.mermaid-container');
    expect(mermaidAfter).not.toBeNull();
    // Same DOM element reference means React reconciled in place (not unmounted/remounted)
    expect(mermaidAfter).toBe(mermaidBefore);
  });

  // E-PENPAL-HIGHLIGHT-MEDIA: mermaid container gets highlight class via annotation
  it('applies comment-highlight class to mermaid container when highlight spans into it', () => {
    const md = 'Before text\n\n```mermaid\ngraph TD\n  A --> B\n```';
    const highlights = [
      { threadId: 't-media', selectedText: 'Before text A B', startLine: 1 },
    ];
    const { container } = render(
      <MarkdownViewer content={md} rawMarkdown={md} highlights={highlights} />,
    );
    const mermaidContainer = container.querySelector('.mermaid-container.comment-highlight');
    expect(mermaidContainer).not.toBeNull();
    expect(mermaidContainer?.getAttribute('data-thread-id')).toBe('t-media');
  });

  it('renders pending highlights with pending-highlight class', () => {
    const md = 'Hello world';
    const highlights = [
      { threadId: 'pending', selectedText: 'world', startLine: 1, pending: true },
    ];
    const { container } = render(
      <MarkdownViewer content={md} rawMarkdown={md} highlights={highlights} />,
    );
    const mark = container.querySelector('mark.pending-highlight');
    expect(mark).not.toBeNull();
    expect(mark?.classList.contains('comment-highlight')).toBe(true);
  });
});
