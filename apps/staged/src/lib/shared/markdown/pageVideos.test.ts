// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';

vi.mock('../../transport', () => ({ isTauri: false }));

import { pageVideos, remakeVideosInPage } from './pageVideos';
import { renderMarkdown } from './renderMarkdown';

const id = '3f2a9c1e-1234-4567-8123-123456789abc';
const NOTE = `Before\n\n![Demo clip](staged-media://${id}.mp4 "The demo")\n\nAfter`;

/*
 * What can be checked here is which document made the element the page ends
 * up with. That WebKit draws one and not the other is checked in a capture of
 * the real window (see pageVideos.ts): jsdom has no media pipeline.
 */
describe('a note video rendered with {@html}', () => {
  let load: MockInstance<() => void>;
  let importNode: MockInstance<Document['importNode']>;

  beforeEach(() => {
    document.body.innerHTML = '';
    // jsdom only reports that it has no `load`.
    load = vi.spyOn(HTMLMediaElement.prototype, 'load').mockImplementation(() => {});
    importNode = vi.spyOn(document, 'importNode');
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  /** The elements the page's own document has made, by `importNode`. */
  const madeByPage = () => importNode.mock.results.map((result) => result.value as Node);

  /** The same elements, not merely equal ones: `toEqual` compares nodes by their markup. */
  function expectSame(actual: unknown[], expected: Node[]) {
    expect(actual).toHaveLength(expected.length);
    actual.forEach((node, i) => expect(node).toBe(expected[i]));
  }

  /**
   * The note's markup, mounted the way Svelte's first `{@html}` render mounts
   * it: parsed in a template's document, then moved into the page.
   */
  function mountLikeSvelte(markdown = NOTE): HTMLElement {
    const template = document.createElement('template');
    template.innerHTML = `<div class="markdown-content">${renderMarkdown(markdown)}</div>`;
    const root = template.content.firstElementChild as HTMLElement;
    expect(root.ownerDocument).not.toBe(document);
    document.body.append(root);
    return root;
  }

  it('is replaced by an element the page made, with the same markup', () => {
    const root = mountLikeSvelte();
    const adopted = root.querySelector('video')!;
    const markup = adopted.outerHTML;

    remakeVideosInPage(root);

    const video = root.querySelector('video')!;
    expect(video).not.toBe(adopted);
    expectSame(madeByPage(), [video]);
    expect(video.ownerDocument).toBe(document);
    expect(video.outerHTML).toBe(markup);
    expect(video.getAttribute('src')).toBe(`/api/media/${id}.mp4#t=0.001`);
    // In the adopted one's place: still the figure's video, above its caption.
    expect(root.querySelectorAll('video')).toHaveLength(1);
    expect(video.parentElement?.className).toBe('markdown-media markdown-media-video');
    expect(video.nextElementSibling?.tagName).toBe('FIGCAPTION');
  });

  it('leaves the adopted element with no source to go on loading', () => {
    const root = mountLikeSvelte();
    const adopted = root.querySelector('video')!;

    remakeVideosInPage(root);

    expect(adopted.isConnected).toBe(false);
    expect(adopted.hasAttribute('src')).toBe(false);
    expectSame(load.mock.contexts, [adopted]);
  });

  it('replaces every video in the note, and nothing else', () => {
    const root = mountLikeSvelte(
      `![One](staged-media://${id}.mp4)\n\n![Still](staged-media://${id}.png)\n\n![Two](staged-media://${id}.webm)`
    );
    const image = root.querySelector('img')!;
    const adopted = [...root.querySelectorAll('video')];

    remakeVideosInPage(root);

    const videos = [...root.querySelectorAll('video')];
    expect(videos.map((video) => video.getAttribute('aria-label'))).toEqual(['One', 'Two']);
    expectSame(madeByPage(), videos);
    expect(videos).not.toContain(adopted[0]);
    expect(videos).not.toContain(adopted[1]);
    expect(root.querySelector('img')).toBe(image);
  });

  it('keeps the element it made when it runs again', () => {
    const root = mountLikeSvelte();
    remakeVideosInPage(root);
    const video = root.querySelector('video')!;

    remakeVideosInPage(root);
    pageVideos('same render')(root);

    // The reader may have started it: a second copy would start it over.
    expect(root.querySelector('video')).toBe(video);
    expect(importNode).toHaveBeenCalledTimes(1);
    expect(video.getAttribute('src')).toBe(`/api/media/${id}.mp4#t=0.001`);
  });

  it('only touches videos under the element it is given', () => {
    const root = mountLikeSvelte();
    // The editor's video, which its node view holds on to (wysiwygMedia.ts).
    const editor = document.createElement('div');
    editor.innerHTML = renderMarkdown(NOTE);
    document.body.append(editor);
    const editorVideo = editor.querySelector('video')!;

    remakeVideosInPage(root);

    expect(editor.querySelector('video')).toBe(editorVideo);
    expect(editorVideo.hasAttribute('src')).toBe(true);
  });

  it('leaves a video that is not in a page yet alone, and says so', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const template = document.createElement('template');
    template.innerHTML = `<div>${renderMarkdown(NOTE)}</div>`;
    const root = template.content.firstElementChild as HTMLElement;
    const adopted = root.querySelector('video')!;

    remakeVideosInPage(root);

    expect(root.querySelector('video')).toBe(adopted);
    expect(adopted.hasAttribute('src')).toBe(true);
    expect(importNode).not.toHaveBeenCalled();
    expect(load).not.toHaveBeenCalled();
    // Nothing runs it again for this render, so this is the only sign.
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0][0]).toContain('[pageVideos]');
  });

  it('says nothing when there is no video to leave alone, or when it remakes one', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const template = document.createElement('template');
    template.innerHTML = `<div>${renderMarkdown('Just text')}</div>`;

    remakeVideosInPage(template.content.firstElementChild as HTMLElement);
    remakeVideosInPage(mountLikeSvelte());

    expect(importNode).toHaveBeenCalledTimes(1);
    expect(warn).not.toHaveBeenCalled();
  });

  it('gives each render its own attachment, so Svelte runs it again', () => {
    expect(pageVideos('<p>one</p>')).not.toBe(pageVideos('<p>one</p>'));
  });
});
