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
  it('renders Pikchr fenced blocks as inert diagram previews with escaped source', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Start" fit\n```');

    expect(html).toContain('<figure class="note-diagram note-diagram-pikchr">');
    expect(html).toContain('<figcaption class="note-diagram-caption">Pikchr</figcaption>');
    expect(html).toContain('<div class="note-diagram-preview note-diagram-preview-pikchr">');
    expect(html).toContain('<pre class="note-diagram-source note-diagram-source-pikchr">');
    expect(html).toContain('box "Start" fit');
  });

  it('leaves non-diagram fenced blocks as normal code blocks', () => {
    const html = renderNoteMarkdown('```ts\nconst value = 1;\n```');

    expect(html).toContain('<pre><code class="language-ts">');
    expect(html).not.toContain('note-diagram-source');
  });

  it('preserves Mermaid fences as escaped diagram source blocks', () => {
    const html = renderNoteMarkdown('```mermaid\nflowchart TD\nA-->B\n```');

    expect(html).toContain(
      '<pre class="note-diagram-source note-diagram-source-mermaid"><code class="language-mermaid">'
    );
    expect(html).toContain('flowchart TD');
    expect(html).not.toContain('note-diagram-preview');
  });

  it('does not emit executable Pikchr preview surfaces before an SVG renderer exists', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "<script>alert(1)</script>" fit\n```');

    expect(html).toContain('&lt;script&gt;alert(1)&lt;/script&gt;');
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('<svg');
    expect(html).not.toContain('<iframe');
    expect(html).not.toContain('srcdoc');
  });

  it('keeps raw SVG fences escaped as source for now', () => {
    const html = renderNoteMarkdown('```svg\n<svg><script>alert(1)</script></svg>\n```');

    expect(html).toContain('note-diagram-source-svg');
    expect(html).toContain('&lt;svg&gt;&lt;script&gt;alert(1)&lt;/script&gt;&lt;/svg&gt;');
    expect(html).not.toContain('<script>');
  });
});
