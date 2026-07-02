import { describe, expect, it, vi } from 'vitest';

import { loadPikchrRenderer, sanitizePikchrSvg } from './pikchrRendering';

describe('sanitizePikchrSvg', () => {
  it('keeps static Pikchr geometry and applies the themed default ink', () => {
    const svg = sanitizePikchrSvg(
      [
        '<svg xmlns="http://www.w3.org/2000/svg" class="markdown-pikchr-svg" viewBox="0 0 58 34" data-pikchr-date="20260403102956">',
        '<path d="M2,32L56,32L56,2L2,2Z" style="fill:none;stroke-width:2.16;stroke:rgb(0,0,0);" />',
        '<text x="29" y="17" text-anchor="middle" fill="rgb(0,0,0)" dominant-baseline="central">Start</text>',
        '</svg>',
      ].join('')
    );

    expect(svg).toContain('<svg');
    expect(svg).toContain('viewBox="0 0 58 34"');
    expect(svg).toContain('<path');
    expect(svg).toContain('style="fill:none;stroke-width:2.16;stroke:var(--pikchr-ink)"');
    expect(svg).toContain('<text');
    expect(svg).toContain('fill="var(--pikchr-ink)"');
    expect(svg).not.toContain('data-pikchr-date');
  });

  it('keeps safe direct SVG colors and strips unsafe direct SVG colors', () => {
    const svg = sanitizePikchrSvg(
      [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20">',
        '<path d="M1,1L19,19" fill="none" stroke="#123456" />',
        '<text x="10" y="10" fill="rgb(1, 2, 3)" stroke="url(https://example.com/stroke)">Label</text>',
        '<rect x="1" y="1" width="4" height="4" fill="url(https://example.com/fill)" stroke="rgba(12, 34, 56, 0.5)" />',
        '</svg>',
      ].join('')
    );

    expect(svg).toContain('fill="none"');
    expect(svg).toContain('stroke="#123456"');
    expect(svg).toContain('fill="rgb(1, 2, 3)"');
    expect(svg).toContain('stroke="rgba(12, 34, 56, 0.5)"');
    expect(svg).not.toContain('url(');
  });

  it('maps common Pikchr colors onto the themed palette', () => {
    const svg = sanitizePikchrSvg(
      [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 40">',
        '<path d="M1,1L119,1" style="stroke:rgb(255,0,0);fill:white" />',
        '<rect x="1" y="1" width="10" height="10" fill="yellow" stroke="green" />',
        '<text x="20" y="20" fill="blue">Label</text>',
        '</svg>',
      ].join('')
    );

    expect(svg).toContain('stroke:var(--pikchr-red)');
    expect(svg).toContain('fill:var(--pikchr-surface)');
    expect(svg).toContain('fill="var(--pikchr-yellow)"');
    expect(svg).toContain('stroke="var(--pikchr-green)"');
    expect(svg).toContain('fill="var(--pikchr-blue)"');
  });

  it('preserves numeric Pikchr colors from the source', () => {
    const svg = sanitizePikchrSvg(
      [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 40">',
        '<path d="M1,1L119,1" style="stroke:rgb(255,0,0);fill:rgb(255,255,255)" />',
        '<text x="20" y="20" fill="rgb(0,0,0)">Label</text>',
        '</svg>',
      ].join(''),
      {
        source: 'box "Numeric colors" color 0xff0000 fill 0xffffff',
      }
    );

    expect(svg).toContain('stroke:rgb(255,0,0)');
    expect(svg).toContain('fill:rgb(255,255,255)');
    expect(svg).toContain('fill="var(--pikchr-ink)"');
  });

  it('keeps fill none as no paint', () => {
    const svg = sanitizePikchrSvg(
      [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20">',
        '<path d="M1,1L19,19" fill="none" stroke="rgb(0,0,0)" />',
        '</svg>',
      ].join('')
    );

    expect(svg).toContain('fill="none"');
    expect(svg).toContain('stroke="var(--pikchr-ink)"');
  });

  it('adds breathing room to side-anchored Pikchr text labels', () => {
    const svg = sanitizePikchrSvg(
      [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 80">',
        '<text x="20" y="20" text-anchor="start">Right-side label</text>',
        '<text x="20" y="40" text-anchor="end">Left-side label</text>',
        '<text x="20" y="60" text-anchor="start" dx="1em">Custom label</text>',
        '<text x="60" y="20" text-anchor="middle">Centered label</text>',
        '</svg>',
      ].join('')
    );

    expect(svg).toMatch(
      /<text\b(?=[^>]*text-anchor="start")(?=[^>]*dx="0\.35em")[^>]*>Right-side label<\/text>/
    );
    expect(svg).toMatch(
      /<text\b(?=[^>]*text-anchor="end")(?=[^>]*dx="-0\.35em")[^>]*>Left-side label<\/text>/
    );
    expect(svg).toMatch(
      /<text\b(?=[^>]*text-anchor="start")(?=[^>]*dx="1em")[^>]*>Custom label<\/text>/
    );
    expect(svg).toMatch(
      /<text\b(?=[^>]*text-anchor="middle")(?![^>]*\bdx=)[^>]*>Centered label<\/text>/
    );
  });

  it('strips executable and external-resource SVG surface', () => {
    const svg = sanitizePikchrSvg(
      [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" onclick="alert(1)">',
        '<script>alert(1)</script>',
        '<foreignObject><iframe src="https://example.com"></iframe></foreignObject>',
        '<path d="M0,0L10,10" style="stroke:url(https://example.com/x);fill:none;" />',
        '<text x="1" y="1" href="https://example.com">Label</text>',
        '</svg>',
      ].join('')
    );

    expect(svg).toContain('<svg');
    expect(svg).not.toContain('<script');
    expect(svg).not.toContain('foreignObject');
    expect(svg).not.toContain('iframe');
    expect(svg).not.toContain('onclick');
    expect(svg).not.toContain('href');
    expect(svg).not.toContain('url(');
  });

  it('rejects non-SVG renderer output', () => {
    expect(sanitizePikchrSvg('<div><pre>ERROR</pre></div>')).toBeNull();
  });
});

describe('loadPikchrRenderer', () => {
  it('loads the bundled renderer and returns sanitized SVG', async () => {
    const renderPikchr = await loadPikchrRenderer();
    const rendered = renderPikchr('box "Start" fit');

    expect(rendered.kind).toBe('svg');
    if (rendered.kind !== 'svg') return;

    expect(rendered.width).toBeGreaterThan(0);
    expect(rendered.height).toBeGreaterThan(0);
    expect(rendered.svg).toContain('<svg');
    expect(rendered.svg).toContain('class="markdown-pikchr-svg"');
    expect(rendered.svg).toContain('<path');
    expect(rendered.svg).toContain('Start');
    expect(rendered.svg).toContain('var(--pikchr-ink)');
    expect(rendered.svg).not.toContain('<script');
    expect(rendered.svg).not.toContain('data-pikchr-date');
  });

  it('retries loading after renderer initialization fails', async () => {
    const loadPikchr = vi
      .fn()
      .mockRejectedValueOnce(new Error('initialization failed'))
      .mockResolvedValueOnce({
        render: () => ({
          width: 10,
          height: 10,
          svg: '<svg xmlns="http://www.w3.org/2000/svg" class="markdown-pikchr-svg" viewBox="0 0 10 10"><path d="M0,0L10,10" /></svg>',
        }),
      });

    vi.resetModules();
    vi.doMock('pikchr-js', () => ({ default: loadPikchr }));

    try {
      const { loadPikchrRenderer } = await import('./pikchrRendering');

      await expect(loadPikchrRenderer()).rejects.toThrow('initialization failed');

      const renderPikchr = await loadPikchrRenderer();
      const rendered = renderPikchr('box "Retry" fit');

      expect(loadPikchr).toHaveBeenCalledTimes(2);
      expect(rendered).toMatchObject({ kind: 'svg', width: 10, height: 10 });
    } finally {
      vi.doUnmock('pikchr-js');
      vi.resetModules();
    }
  });
});
