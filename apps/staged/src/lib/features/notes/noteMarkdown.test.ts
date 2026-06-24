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
  it('marks Pikchr fenced blocks as note diagram sources', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Start" fit\n```');

    expect(html).toContain(
      '<pre class="note-diagram-source note-diagram-source-pikchr"><code class="language-pikchr">'
    );
    expect(html).toContain('box "Start" fit');
  });

  it('leaves non-diagram fenced blocks as normal code blocks', () => {
    const html = renderNoteMarkdown('```ts\nconst value = 1;\n```');

    expect(html).toContain('<pre><code class="language-ts">');
    expect(html).not.toContain('note-diagram-source');
  });

  it('keeps raw SVG fences escaped as source for now', () => {
    const html = renderNoteMarkdown('```svg\n<svg><script>alert(1)</script></svg>\n```');

    expect(html).toContain('note-diagram-source-svg');
    expect(html).toContain('&lt;svg&gt;&lt;script&gt;alert(1)&lt;/script&gt;&lt;/svg&gt;');
    expect(html).not.toContain('<script>');
  });
});
