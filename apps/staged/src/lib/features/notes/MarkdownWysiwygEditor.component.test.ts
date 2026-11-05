import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ImageDimensions } from '../../types';

// A copied branch note is stored as a written note (note_transfer.rs), so
// clicking it opens WriteNoteModal, i.e. this editor, not NoteModal. Its body
// still carries the media the transfer copied, as `staged-media://` refs.
const IMAGE_ID = '9f1c0da7-0000-4000-8000-000000000001';
const VIDEO_ID = '9f1c0da7-0000-4000-8000-000000000002';
const copiedNote = [
  '# Shots',
  '',
  `![The fixed row](staged-media://${IMAGE_ID}.png)`,
  '',
  `![Recording](staged-media://${VIDEO_ID}.mp4)`,
  '',
].join('\n');

const { getMediaDimensions } = vi.hoisted(() => ({
  getMediaDimensions: vi.fn(async (): Promise<Record<string, ImageDimensions>> => ({
    // A Retina capture: 2x pixels, laid out at logical size.
    [IMAGE_ID]: { width: 1320, height: 960, density: 2 },
  })),
}));
vi.mock('../../api/commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../api/commands')>()),
  getMediaDimensions,
}));

import { resolveStagedMediaUrl } from '../../shared/markdown/mediaUrl';
import MarkdownWysiwygEditor from './MarkdownWysiwygEditor.svelte';

let component: ReturnType<typeof mount> | null = null;

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

async function mountEditor(value: string) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  let ready!: (error: Error | null) => void;
  const loaded = new Promise<Error | null>((resolve) => (ready = resolve));
  component = mount(MarkdownWysiwygEditor, { target, props: { value, onReady: ready } });
  flushSync();
  expect(await loaded).toBeNull();
  return target;
}

describe('MarkdownWysiwygEditor with a copied note’s media', () => {
  it('loads embedded images from the media server, as the viewer does', async () => {
    const root = await mountEditor(copiedNote);
    const image = await vi.waitFor(() => {
      const img = root.querySelector<HTMLImageElement>(`img[alt="The fixed row"]`);
      expect(img).not.toBeNull();
      return img!;
    });
    expect(image.getAttribute('src')).toBe(resolveStagedMediaUrl(IMAGE_ID, 'png'));
    // The viewer's figure, inside the editor's document, so the shared media
    // styles reach it.
    const figure = image.closest('figure.markdown-media.markdown-media-image');
    expect(figure?.closest('.markdown-content .ProseMirror')).not.toBeNull();
    expect(figure?.querySelector('figcaption')?.textContent).toBe('The fixed row');
  });

  it('lays images out at their logical size once their dimensions are known', async () => {
    const root = await mountEditor(copiedNote);
    await vi.waitFor(() => {
      const img = root.querySelector<HTMLImageElement>(`img[alt="The fixed row"]`);
      expect(img?.getAttribute('width')).toBe('660');
      expect(img?.getAttribute('height')).toBe('480');
      expect(img?.getAttribute('srcset')).toBe(`${resolveStagedMediaUrl(IMAGE_ID, 'png')} 2x`);
      expect(img?.style.getPropertyValue('--media-width')).toBe('660px');
    });
  });

  it('plays embedded videos rather than showing them as images', async () => {
    const root = await mountEditor(copiedNote);
    await vi.waitFor(() => expect(root.querySelector('.ProseMirror')).not.toBeNull());
    const video = root.querySelector<HTMLVideoElement>('video');
    expect(video?.getAttribute('src')).toBe(resolveStagedMediaUrl(VIDEO_ID, 'mp4'));
    expect(video?.hasAttribute('controls')).toBe(true);
    expect(video?.closest('figure.markdown-media-video')?.getAttribute('contenteditable')).toBe(
      'false'
    );
    expect(root.querySelector(`img[alt="Recording"]`)).toBeNull();
  });

  it('round-trips the stored references unchanged', async () => {
    await mountEditor(copiedNote);
    const editor = component as unknown as { getMarkdown(): string };
    const markdown = editor.getMarkdown();
    expect(markdown).toContain(`staged-media://${IMAGE_ID}.png`);
    expect(markdown).toContain(`staged-media://${VIDEO_ID}.mp4`);
  });
});
