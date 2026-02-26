import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import TableOfContents from './TableOfContents';

describe('TableOfContents', () => {
  it('returns null for empty headings', () => {
    const { container } = render(<TableOfContents headings={[]} />);
    expect(container.innerHTML).toBe('');
  });

  it('renders heading links', () => {
    const headings = [
      { level: 1 as const, text: 'Introduction', id: 'introduction' },
      { level: 2 as const, text: 'Overview', id: 'overview' },
      { level: 3 as const, text: 'Details', id: 'details' },
    ];
    render(<TableOfContents headings={headings} />);
    expect(screen.getByText('On this page')).toBeDefined();
    expect(screen.getByText('Introduction')).toBeDefined();
    expect(screen.getByText('Overview')).toBeDefined();
    expect(screen.getByText('Details')).toBeDefined();
  });

  it('links to heading anchors', () => {
    const headings = [{ level: 1 as const, text: 'Title', id: 'title' }];
    render(<TableOfContents headings={headings} />);
    const link = screen.getByText('Title') as HTMLAnchorElement;
    expect(link.getAttribute('href')).toBe('#title');
  });

  it('applies correct level classes', () => {
    const headings = [
      { level: 1 as const, text: 'H1', id: 'h1' },
      { level: 2 as const, text: 'H2', id: 'h2' },
      { level: 3 as const, text: 'H3', id: 'h3' },
    ];
    const { container } = render(<TableOfContents headings={headings} />);
    expect(container.querySelector('.level-1')).toBeDefined();
    expect(container.querySelector('.level-2')).toBeDefined();
    expect(container.querySelector('.level-3')).toBeDefined();
  });
});
