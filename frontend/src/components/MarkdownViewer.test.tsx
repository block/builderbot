import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import MarkdownViewer from './MarkdownViewer';

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

  it('adds data-source-line attributes to block elements', async () => {
    const md = '# Heading\n\nParagraph text here.\n\n- List item';
    const { container } = render(<MarkdownViewer content={md} rawMarkdown={md} />);
    // Wait for useEffect
    await new Promise((r) => setTimeout(r, 10));
    const withSourceLine = container.querySelectorAll('[data-source-line]');
    expect(withSourceLine.length).toBeGreaterThan(0);
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
});
