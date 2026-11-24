// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../transport', () => ({ isTauri: false }));

import { missingMedia, watchForMissingMedia } from './missingMedia';
import { renderMarkdown } from './renderMarkdown';

const IMAGE_ID = '3f2a9c1e-1234-4567-8123-123456789abc';
const OTHER_IMAGE_ID = '7b1d4e2a-9876-4abc-8def-0123456789ab';
const VIDEO_ID = '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5d';
const NOTE = [
  'Before',
  `![The settings page](staged-media://${IMAGE_ID}.png)`,
  `![Another](staged-media://${OTHER_IMAGE_ID}.png)`,
  `![Demo clip](staged-media://${VIDEO_ID}.mp4)`,
  '![Elsewhere](https://example.com/picture.png)',
  'After',
].join('\n\n');

/** The note as a viewer shows it, in the page. */
function mount(markdown = NOTE): HTMLElement {
  const root = document.createElement('div');
  root.className = 'markdown-content';
  root.innerHTML = renderMarkdown(markdown);
  document.body.replaceChildren(root);
  return root;
}

const figure = (root: Element, id: string) =>
  root.querySelector<HTMLElement>(`figure[data-media-id="${id}"]`)!;
const media = (root: Element, id: string) =>
  figure(root, id).querySelector<HTMLImageElement | HTMLVideoElement>('img, video')!;

/** The load failed. `error` does not bubble, from an image or from a video. */
function fail(element: Element) {
  element.dispatchEvent(new Event('error'));
}

beforeEach(() => {
  document.body.innerHTML = '';
});

describe('watchForMissingMedia', () => {
  it('marks nothing while the media loads', () => {
    const root = mount();
    watchForMissingMedia(root);

    media(root, IMAGE_ID).dispatchEvent(new Event('load'));

    expect(root.querySelector('[data-media-missing]')).toBeNull();
  });

  it('marks the figure of an image that fails to load, and no other', () => {
    const root = mount();
    watchForMissingMedia(root);

    fail(media(root, IMAGE_ID));

    expect(figure(root, IMAGE_ID).getAttribute('data-media-missing')).toBe('image');
    expect(figure(root, IMAGE_ID).getAttribute('data-media-missing-label')).toBe(
      'Image no longer available'
    );
    expect(root.querySelectorAll('[data-media-missing]')).toHaveLength(1);
  });

  it('marks the figure of a video that fails to load', () => {
    const root = mount();
    watchForMissingMedia(root);

    fail(media(root, VIDEO_ID));

    expect(figure(root, VIDEO_ID).getAttribute('data-media-missing')).toBe('video');
    expect(figure(root, VIDEO_ID).getAttribute('data-media-missing-label')).toBe(
      'Video no longer available'
    );
  });

  it('leaves a video that was given another source to try, until that fails too', () => {
    const root = mount();
    const video = media(root, VIDEO_ID) as HTMLVideoElement;
    const stream = `http://127.0.0.1:49152/${'a'.repeat(64)}/${VIDEO_ID}.mp4`;
    const protocol = `staged-media://localhost/${VIDEO_ID}.mp4`;
    video.setAttribute('src', stream);
    // As `leaveFailedStream` does, from the document, ahead of the watch.
    const leaveStream = () => {
      if (video.getAttribute('src') === stream) video.setAttribute('src', protocol);
    };
    document.addEventListener('error', leaveStream, true);
    watchForMissingMedia(root);

    fail(video);
    expect(video.getAttribute('src')).toBe(protocol);
    expect(figure(root, VIDEO_ID).hasAttribute('data-media-missing')).toBe(false);

    // The protocol could not load it either.
    video.dispatchEvent(new Event('loadstart'));
    fail(video);
    expect(figure(root, VIDEO_ID).getAttribute('data-media-missing')).toBe('video');
    document.removeEventListener('error', leaveStream, true);
  });

  it('follows a video that replaced the one the markup came with', () => {
    const root = mount();
    watchForMissingMedia(root);
    // As pageVideos does, whichever of the two runs first.
    const copy = media(root, VIDEO_ID).cloneNode(true) as HTMLVideoElement;
    media(root, VIDEO_ID).replaceWith(copy);

    fail(copy);

    expect(figure(root, VIDEO_ID).getAttribute('data-media-missing')).toBe('video');
  });

  it('leaves media that is known to be kept as the browser shows it', () => {
    const root = mount();
    watchForMissingMedia(root, (id) => id === IMAGE_ID);

    fail(media(root, IMAGE_ID));
    fail(media(root, OTHER_IMAGE_ID));

    expect(figure(root, IMAGE_ID).hasAttribute('data-media-missing')).toBe(false);
    expect(figure(root, OTHER_IMAGE_ID).getAttribute('data-media-missing')).toBe('image');
  });

  it('leaves an image that is not stored media alone', () => {
    const root = mount();
    watchForMissingMedia(root);

    fail(root.querySelector('img[src^="https://example.com"]')!);

    expect(root.querySelector('[data-media-missing]')).toBeNull();
  });

  it('asks for every stored image at once, so none fails later under the reader', () => {
    const root = mount();
    const elsewhere = root.querySelector('img[src^="https://example.com"]')!;
    const asWritten = elsewhere.getAttribute('loading');
    expect(media(root, IMAGE_ID).getAttribute('loading')).toBe('lazy');

    watchForMissingMedia(root);

    expect(media(root, IMAGE_ID).getAttribute('loading')).toBe('eager');
    expect(media(root, OTHER_IMAGE_ID).getAttribute('loading')).toBe('eager');
    expect(elsewhere.getAttribute('loading')).toBe(asWritten);
  });

  it('marks an image that had already failed when the watch began', () => {
    const root = mount();
    const image = media(root, IMAGE_ID) as HTMLImageElement;
    Object.defineProperty(image, 'complete', { value: true });
    Object.defineProperty(image, 'naturalWidth', { value: 0 });

    watchForMissingMedia(root);

    expect(figure(root, IMAGE_ID).getAttribute('data-media-missing')).toBe('image');
    expect(figure(root, OTHER_IMAGE_ID).hasAttribute('data-media-missing')).toBe(false);
  });

  it('changes nothing of the figure but its attributes', () => {
    const root = mount();
    const before = figure(root, IMAGE_ID).innerHTML.replace('loading="lazy"', 'loading="eager"');
    watchForMissingMedia(root);

    fail(media(root, IMAGE_ID));

    expect(figure(root, IMAGE_ID).innerHTML).toBe(before);
    expect(figure(root, IMAGE_ID).textContent).toBe('The settings page');
  });

  it('stops marking once the watch is undone', () => {
    const root = mount();
    const stop = watchForMissingMedia(root);

    stop();
    fail(media(root, IMAGE_ID));

    expect(root.querySelector('[data-media-missing]')).toBeNull();
  });
});

describe('missingMedia', () => {
  it('watches the element it is attached to until it is detached', () => {
    const root = mount();
    const detach = missingMedia(root.innerHTML)(root);

    fail(media(root, IMAGE_ID));
    expect(figure(root, IMAGE_ID).getAttribute('data-media-missing')).toBe('image');

    if (typeof detach === 'function') detach();
    fail(media(root, OTHER_IMAGE_ID));
    expect(figure(root, OTHER_IMAGE_ID).hasAttribute('data-media-missing')).toBe(false);
  });
});
