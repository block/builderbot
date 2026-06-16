import { describe, expect, it } from 'vitest';

import { renderHashtagTokens } from '../../features/sessions/hashtagItems';
import type { HashtagItem } from '../../types';
import { renderMarkdown } from './renderMarkdown';
import type { PikchrRenderer } from './pikchrRendering';

describe('renderMarkdown', () => {
  it('renders Pikchr fenced blocks as escaped source while the renderer is unavailable', () => {
    const html = renderMarkdown('```pikchr\nbox "Start" fit\n```');

    expect(html).toContain('<pre class="markdown-diagram-source markdown-diagram-source-pikchr">');
    expect(html).toContain('box "Start" fit');
    expect(html).not.toContain('<svg');
    expect(html).not.toContain('markdown-diagram-preview');
  });

  it('leaves non-diagram fenced blocks as normal code blocks', () => {
    const html = renderMarkdown('```ts\nconst value = 1;\n```');

    expect(html).toContain('<pre><code class="language-ts">');
    expect(html).not.toContain('markdown-diagram-source');
  });

  it('leaves Mermaid fenced blocks as normal code blocks', () => {
    const html = renderMarkdown('```mermaid\nflowchart TD\nA-->B\n```');

    expect(html).toContain('<pre><code class="language-mermaid">');
    expect(html).toContain('flowchart TD');
    expect(html).not.toContain('markdown-diagram-source');
    expect(html).not.toContain('markdown-diagram-preview');
  });

  it('renders Pikchr fenced blocks as sanitized SVG when a renderer is loaded', () => {
    const html = renderMarkdown('```pikchr\nbox "Start" fit\n```', {
      pikchrRenderer: safePikchrRenderer,
    });

    expect(html).toContain(
      '<figure class="markdown-diagram markdown-diagram-pikchr" role="button" tabindex="0" aria-label="Open diagram full screen" title="Open diagram full screen">'
    );
    expect(html).not.toContain('markdown-diagram-caption');
    expect(html).toContain(
      '<div class="markdown-diagram-preview markdown-diagram-preview-pikchr">'
    );
    expect(html).toContain('<svg');
    expect(html).toContain('class="markdown-pikchr-svg"');
    expect(html).toContain('<path');
    expect(html).toContain('stroke:var(--pikchr-ink)');
    expect(html).not.toContain('markdown-diagram-source-wrap');
    expect(html).not.toContain('markdown-diagram-source-pikchr');
    expect(html).not.toContain('box "Start" fit');
    expect(html).not.toContain('box &quot;Start&quot; fit');
    expect(html).not.toContain('STAGED_MARKDOWN_TRUSTED_HTML_');
  });

  it('preserves trusted inline hashtag badge HTML through Markdown sanitization', () => {
    const html = renderMarkdown('See #note:note-1 for context.', {
      renderInlineText: (text) => renderHashtagTokens(text, hashtagItems),
    });

    expect(html).toContain('See ');
    expect(html).toContain('class="hashtag-badge type-note stable-raster stable-raster-glyphs"');
    expect(html).toContain('role="button"');
    expect(html).toContain('tabindex="0"');
    expect(html).toContain('data-hashtag-ref="#note:note-1"');
    expect(html).toContain('data-hashtag-type="note"');
    expect(html).toContain('data-hashtag-id="note-1"');
    expect(html).not.toContain('style=');
    expect(html).toContain('<svg xmlns="http://www.w3.org/2000/svg"');
    expect(html).toContain('<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2');
    expect(html).toContain('Escaped &lt;title&gt; &amp; &quot;quotes&quot;');
    expect(html).not.toContain('STAGED_MARKDOWN_TRUSTED_HTML_');
  });

  it('preserves numeric Pikchr colors through the Markdown rendering path', () => {
    const html = renderMarkdown('```pikchr\nbox "Exact" color 0xff0000 fill 0xffffff\n```', {
      pikchrRenderer: numericColorPikchrRenderer,
    });

    expect(html).toContain('stroke:rgb(255,0,0)');
    expect(html).toContain('fill:rgb(255,255,255)');
  });

  it('does not allow renderer SVG through the generic Markdown sanitizer', () => {
    const html = renderMarkdown('<svg><script>alert(1)</script></svg>');

    expect(html).not.toContain('<svg');
    expect(html).not.toContain('<script>');
  });

  it('does not allow raw HTML to opt into markdown diagram classes', () => {
    const html = renderMarkdown(
      '<div class="markdown-diagram-preview"><figcaption class="markdown-diagram-caption">Pikchr</figcaption></div>'
    );

    expect(html).not.toContain('class="markdown-diagram-preview"');
    expect(html).not.toContain('class="markdown-diagram-caption"');
    expect(html).toContain('Pikchr');
  });

  it('falls back to escaped source when the Pikchr renderer rejects the SVG', () => {
    const html = renderMarkdown('```pikchr\nbox "Unsafe" fit\n```', {
      pikchrRenderer: unsafePikchrRenderer,
    });

    expect(html).toContain('<pre class="markdown-diagram-source markdown-diagram-source-pikchr">');
    expect(html).toContain('box "Unsafe" fit');
    expect(html).not.toContain('<svg');
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('onclick');
  });

  it('leaves raw SVG fenced blocks as normal escaped code blocks', () => {
    const html = renderMarkdown('```svg\n<svg><script>alert(1)</script></svg>\n```');

    expect(html).toContain('<pre><code class="language-svg">');
    expect(html).toContain('&lt;svg&gt;&lt;script&gt;alert(1)&lt;/script&gt;&lt;/svg&gt;');
    expect(html).not.toContain('markdown-diagram-source');
    expect(html).not.toContain('<script>');
  });
});

const safePikchrRenderer: PikchrRenderer = () => ({
  kind: 'svg',
  width: 58,
  height: 34,
  svg: [
    '<svg xmlns="http://www.w3.org/2000/svg" class="markdown-pikchr-svg" viewBox="0 0 58 34">',
    '<path d="M2,32L56,32L56,2L2,2Z" style="fill:none;stroke-width:2.16;stroke:rgb(0,0,0);" />',
    '<text x="29" y="17" text-anchor="middle" fill="rgb(0,0,0)" dominant-baseline="central">Start</text>',
    '</svg>',
  ].join(''),
});

const numericColorPikchrRenderer: PikchrRenderer = () => ({
  kind: 'svg',
  width: 58,
  height: 34,
  svg: [
    '<svg xmlns="http://www.w3.org/2000/svg" class="markdown-pikchr-svg" viewBox="0 0 58 34">',
    '<path d="M2,32L56,32L56,2L2,2Z" style="fill:rgb(255,255,255);stroke-width:2.16;stroke:rgb(255,0,0);" />',
    '<text x="29" y="17" text-anchor="middle" fill="rgb(255,0,0)" dominant-baseline="central">Exact</text>',
    '</svg>',
  ].join(''),
});

const unsafePikchrRenderer: PikchrRenderer = () => ({
  kind: 'error',
  message: 'Pikchr rendered unsafe SVG.',
});

const hashtagItems: HashtagItem[] = [
  {
    type: 'note',
    id: 'note-1',
    title: 'Escaped <title> & "quotes"',
    color: '--note-color',
    bgColor: '--note-bg',
  },
];
