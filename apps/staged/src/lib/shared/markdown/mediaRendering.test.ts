import { describe, expect, it, vi } from 'vitest';
import { renderMarkdown } from './renderMarkdown';
import { renderMarkdownMedia } from './mediaRendering';

vi.mock('../../transport', () => ({ isTauri: false }));
const id = '3f2a9c1e-1234-4567-8123-123456789abc';

describe('note media rendering', () => {
  it('renders an image with its caption, title, resolved URL, and expand button', () => {
    const html = renderMarkdown(`![Login](staged-media://${id}.png "Before")`);
    expect(html).toContain('class="markdown-media markdown-media-image"');
    expect(html).toContain(`src="/api/media/${id}.png"`);
    expect(html).toContain('loading="lazy" title="Before"');
    expect(html).toContain('<figcaption>Login</figcaption>');
    expect(html).toContain('aria-label="Open full screen"');
  });

  it.each(['mp4', 'mov', 'webm'])('renders %s with native video controls', (ext) => {
    const html = renderMarkdown(`![Recording](staged-media://${id}.${ext})`);
    expect(html).toContain(
      `<video src="/api/media/${id}.${ext}" controls preload="metadata" playsinline`
    );
    expect(html).not.toContain('autoplay');
  });

  it.each([
    ['![Step a\\_b]', 'Step a_b'],
    ['![**Before** fix]', 'Before fix'],
    ['![a & b `<c>`]', 'a &amp; b &lt;c&gt;'],
  ])('shows %s as plain caption text, escaped after flattening', (alt, caption) => {
    const html = renderMarkdown(`${alt}(staged-media://${id}.png)`);
    expect(html).toContain(`<figcaption>${caption}</figcaption>`);
    expect(html).toContain(`alt="${caption}"`);
    expect(html).toContain(`data-media-alt="${caption}"`);
    expect(html).not.toContain('<strong>');
    expect(html).not.toContain('<c>');
  });

  it('escapes captions and titles before bypassing sanitization', () => {
    const html = renderMarkdownMedia({
      type: 'image',
      raw: '',
      tokens: [],
      href: `staged-media://${id}.png`,
      text: '<img onerror="bad"> & caption',
      title: '" onload="bad',
    });
    expect(html).toContain('&lt;img onerror=&quot;bad&quot;&gt; &amp; caption');
    expect(html).toContain('title="&quot; onload=&quot;bad"');
    expect(html).not.toContain('<img onerror=');
  });

  it.each([
    `staged-media://bad.png`,
    `staged-media://${id}.svg`,
    `staged-media://${id}.png?x=1`,
    'javascript:alert(1)',
  ])('does not trust %s', (href) => {
    const html = renderMarkdown(`![Caption](${href})`);
    expect(html).not.toContain('<figure');
    expect(html).not.toContain('src=');
  });

  it('preserves HTTP images and keeps raw video HTML sanitized', () => {
    expect(renderMarkdown('![External](https://example.com/a.png)')).toContain(
      '<img src="https://example.com/a.png" alt="External"'
    );
    expect(
      renderMarkdown(
        '<video src="x" controls></video><button class="markdown-media-expand">Open</button>'
      )
    ).not.toMatch(/<video|<button|markdown-media-expand/);
  });
});
