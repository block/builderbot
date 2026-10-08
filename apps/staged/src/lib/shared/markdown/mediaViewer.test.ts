// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import { renderMarkdown } from './renderMarkdown';
import { getMarkdownMediaFromEvent } from './mediaViewer';
vi.mock('../../transport', () => ({ isTauri: false }));
const id = '3f2a9c1e-1234-4567-8123-123456789abc';

describe('note media viewer activation', () => {
  it('resolves an image and nested expand icon to the same media', () => {
    const host = document.createElement('div');
    host.innerHTML = renderMarkdown(`![Look & see](staged-media://${id}.png)`);
    const expected = { id, ext: 'png', kind: 'image', alt: 'Look & see' };
    expect(getMarkdownMediaFromEvent(host.querySelector('img'))).toEqual(expected);
    expect(getMarkdownMediaFromEvent(host.querySelector('path'))).toEqual(expected);
    expect(getMarkdownMediaFromEvent(host.querySelector('figcaption'))).toBeNull();
  });
  it('opens video from its button, leaving native playback clicks alone', () => {
    const host = document.createElement('div');
    host.innerHTML = renderMarkdown(`![Clip](staged-media://${id}.mp4)`);
    expect(getMarkdownMediaFromEvent(host.querySelector('button'))?.kind).toBe('video');
    expect(getMarkdownMediaFromEvent(host.querySelector('video'))).toBeNull();
    expect(getMarkdownMediaFromEvent(null)).toBeNull();
    expect(getMarkdownMediaFromEvent(document.createTextNode('text'))).toBeNull();
  });
});
