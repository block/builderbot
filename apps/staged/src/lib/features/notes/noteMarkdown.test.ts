import { describe, expect, it } from 'vitest';

import { noteMarkdownWithTitle, renderNoteMarkdown } from './noteMarkdown';
import type { NotePikchrRenderer } from './pikchrRendering';

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
  it('renders Pikchr fenced blocks as escaped source while the renderer is unavailable', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Start" fit\n```');

    expect(html).toContain('<pre class="note-diagram-source note-diagram-source-pikchr">');
    expect(html).toContain('box "Start" fit');
    expect(html).not.toContain('<svg');
    expect(html).not.toContain('note-diagram-preview');
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

  it('renders Pikchr fenced blocks as sanitized SVG when a renderer is loaded', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Start" fit\n```', {
      pikchrRenderer: safePikchrRenderer,
    });

    expect(html).toContain('<figure class="note-diagram note-diagram-pikchr">');
    expect(html).toContain('<figcaption class="note-diagram-caption">Pikchr</figcaption>');
    expect(html).toContain('<div class="note-diagram-preview note-diagram-preview-pikchr">');
    expect(html).toContain('<svg');
    expect(html).toContain('class="note-pikchr-svg"');
    expect(html).toContain('<path');
    expect(html).toContain('stroke:rgb(0,0,0)');
    expect(html).toContain('<div class="note-diagram-source-wrap">');
    expect(html).toContain('<pre class="note-diagram-source note-diagram-source-pikchr">');
    expect(html).toContain('box "Start" fit');
    expect(html).not.toContain('STAGED_NOTE_TRUSTED_DIAGRAM_');
  });

  it('does not allow renderer SVG through the generic Markdown sanitizer', () => {
    const html = renderNoteMarkdown('<svg><script>alert(1)</script></svg>');

    expect(html).not.toContain('<svg');
    expect(html).not.toContain('<script>');
  });

  it('does not allow raw HTML to opt into note diagram classes', () => {
    const html = renderNoteMarkdown(
      '<div class="note-diagram-preview"><figcaption class="note-diagram-caption">Pikchr</figcaption></div>'
    );

    expect(html).not.toContain('class="note-diagram-preview"');
    expect(html).not.toContain('class="note-diagram-caption"');
    expect(html).toContain('Pikchr');
  });

  it('falls back to escaped source when the Pikchr renderer rejects the SVG', () => {
    const html = renderNoteMarkdown('```pikchr\nbox "Unsafe" fit\n```', {
      pikchrRenderer: unsafePikchrRenderer,
    });

    expect(html).toContain('<pre class="note-diagram-source note-diagram-source-pikchr">');
    expect(html).toContain('box "Unsafe" fit');
    expect(html).not.toContain('<svg');
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('onclick');
  });

  it('keeps raw SVG fences escaped as source for now', () => {
    const html = renderNoteMarkdown('```svg\n<svg><script>alert(1)</script></svg>\n```');

    expect(html).toContain('note-diagram-source-svg');
    expect(html).toContain('&lt;svg&gt;&lt;script&gt;alert(1)&lt;/script&gt;&lt;/svg&gt;');
    expect(html).not.toContain('<script>');
  });
});

const safePikchrRenderer: NotePikchrRenderer = () => ({
  kind: 'svg',
  width: 58,
  height: 34,
  svg: [
    '<svg xmlns="http://www.w3.org/2000/svg" class="note-pikchr-svg" viewBox="0 0 58 34">',
    '<path d="M2,32L56,32L56,2L2,2Z" style="fill:none;stroke-width:2.16;stroke:rgb(0,0,0);" />',
    '<text x="29" y="17" text-anchor="middle" fill="rgb(0,0,0)" dominant-baseline="central">Start</text>',
    '</svg>',
  ].join(''),
});

const unsafePikchrRenderer: NotePikchrRenderer = () => ({
  kind: 'error',
  message: 'Pikchr rendered unsafe SVG.',
});
