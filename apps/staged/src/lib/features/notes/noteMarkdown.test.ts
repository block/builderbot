import { describe, expect, it } from 'vitest';

import { noteMarkdownWithTitle, renderNoteMarkdown } from './noteMarkdown';

describe('noteMarkdownWithTitle', () => {
  it('prepends the note title as a markdown H1', () => {
    expect(noteMarkdownWithTitle('Investigation notes', 'Body text.')).toBe(
      '# Investigation notes\n\nBody text.'
    );
  });

  it('renders a title-only note as a markdown H1', () => {
    expect(noteMarkdownWithTitle('Standalone title', '')).toBe('# Standalone title');
  });

  it('does not duplicate content that already starts with an H1', () => {
    expect(noteMarkdownWithTitle('Stored title', '# Existing title\n\nBody text.')).toBe(
      '# Existing title\n\nBody text.'
    );
  });

  it('leaves untitled note content unchanged', () => {
    expect(noteMarkdownWithTitle('', 'Body text.')).toBe('Body text.');
  });
});

describe('renderNoteMarkdown', () => {
  it('uses the shared markdown renderer', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Start" fit\n```');

    expect(html).toContain('<pre class="markdown-diagram-source markdown-diagram-source-pikchr">');
    expect(html).toContain('box "Start" fit');
  });
});
