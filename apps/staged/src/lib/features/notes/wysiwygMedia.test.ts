// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';

// The desktop build: the viewer and the editor must resolve a reference to
// the same protocol URL (the web build's `/api/media` path is covered by the
// component test).
vi.mock('../../transport', () => ({ isTauri: true }));
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: vi.fn((path: string, protocol: string) => `${protocol}://localhost/${path}`),
}));
vi.mock('../../api/commands', () => ({ getMediaDimensions: vi.fn(async () => ({})) }));

import { Editor, defaultValueCtx, editorViewCtx, rootCtx } from '@milkdown/kit/core';
import { history, undoCommand } from '@milkdown/kit/plugin/history';
import { commonmark } from '@milkdown/kit/preset/commonmark';
import { gfm } from '@milkdown/kit/preset/gfm';
import { NodeSelection, TextSelection } from '@milkdown/kit/prose/state';
import type { EditorView } from '@milkdown/kit/prose/view';
import { callCommand, getMarkdown } from '@milkdown/kit/utils';

import { wysiwygPlugins } from './wysiwygPlugins';

const IMAGE = 'staged-media://3f2a9c1e-1234-4567-8123-123456789abc.png';
const VIDEO = 'staged-media://3f2a9c1e-1234-4567-8123-123456789abd.mp4';
const note = `# Note\n\n![Shot](${IMAGE})\n\nbetween\n\n![Clip](${VIDEO})`;

let editor: Editor | null = null;

async function createEditor(markdown: string): Promise<EditorView> {
  editor = await Editor.make()
    .config((ctx) => {
      ctx.set(rootCtx, document.body);
      ctx.set(defaultValueCtx, markdown);
    })
    .use(commonmark)
    .use(gfm)
    .use(history)
    .use(wysiwygPlugins)
    .create();
  return editor.ctx.get(editorViewCtx);
}

afterEach(async () => {
  await editor?.destroy();
  editor = null;
  document.body.innerHTML = '';
});

function currentMarkdown(): string {
  return editor?.action(getMarkdown()) ?? '';
}

function figures(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('.ProseMirror figure.markdown-media')];
}

/** Playback for a video, which jsdom does not implement. */
function stubPlayback(video: HTMLVideoElement) {
  let paused = true;
  Object.defineProperty(video, 'paused', { configurable: true, get: () => paused });
  video.play = vi.fn(async () => {
    paused = false;
  });
  video.pause = vi.fn(() => {
    paused = true;
  });
}

/**
 * A key as the browser delivers it: a keydown, then, unless that was
 * prevented, the keypress ProseMirror types a character from.
 */
function press(target: Element, key: string): KeyboardEvent {
  const keydown = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
  target.dispatchEvent(keydown);
  if (!keydown.defaultPrevented && key.length === 1) {
    target.dispatchEvent(
      new KeyboardEvent('keypress', {
        key,
        charCode: key.charCodeAt(0),
        bubbles: true,
        cancelable: true,
      })
    );
  }
  return keydown;
}

function leftClick(): MouseEvent {
  return new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 });
}

/** Position of the first image node whose src is `src`. */
function positionOf(view: EditorView, src: string): number {
  let found = -1;
  view.state.doc.descendants((node, pos) => {
    if (found === -1 && node.type.name === 'image' && node.attrs.src === src) found = pos;
    return found === -1;
  });
  expect(found).toBeGreaterThan(-1);
  return found;
}

describe('stagedMediaView', () => {
  it('resolves stored media through the desktop protocol', async () => {
    await createEditor(note);
    const [image, video] = figures();
    expect(image.querySelector('img')?.getAttribute('src')).toBe(
      'staged-media://localhost/3f2a9c1e-1234-4567-8123-123456789abc.png'
    );
    expect(image.querySelector('figcaption')?.textContent).toBe('Shot');
    expect(video.querySelector('video')?.getAttribute('src')).toBe(
      'staged-media://localhost/3f2a9c1e-1234-4567-8123-123456789abd.mp4'
    );
    expect(video.querySelector('video')?.hasAttribute('controls')).toBe(true);
  });

  it('leaves other images on the plain img the preset draws', async () => {
    await createEditor('# Note\n\n![Screenshot](shot.png)');
    expect(figures()).toHaveLength(0);
    expect(document.querySelector('.ProseMirror img')?.getAttribute('src')).toBe('shot.png');
  });

  it('keeps media in place while text is edited around it', async () => {
    const view = await createEditor(note);
    // After the image's paragraph, inside "between", and the title.
    view.dispatch(view.state.tr.insertText(' more', positionOf(view, IMAGE) + 1));
    view.dispatch(view.state.tr.insertText('Edited ', 1));

    expect(currentMarkdown()).toContain(`![Shot](${IMAGE}) more`);
    expect(currentMarkdown()).toContain(`![Clip](${VIDEO})`);
    expect(currentMarkdown().startsWith('# Edited Note')).toBe(true);
    expect(figures()).toHaveLength(2);
  });

  it('deletes a selected media node and brings it back on undo', async () => {
    const view = await createEditor(note);
    view.dispatch(
      view.state.tr.setSelection(NodeSelection.create(view.state.doc, positionOf(view, VIDEO)))
    );
    expect(document.querySelector('.markdown-media-video.ProseMirror-selectednode')).not.toBeNull();

    view.dispatch(view.state.tr.deleteSelection());
    expect(currentMarkdown()).not.toContain(VIDEO);
    expect(currentMarkdown()).toContain(IMAGE);
    expect(figures()).toHaveLength(1);

    editor!.action(callCommand(undoCommand.key));
    expect(currentMarkdown()).toContain(`![Clip](${VIDEO})`);
    expect(figures()).toHaveLength(2);
    expect(figures()[1].querySelector('video')).not.toBeNull();
  });

  // A caption-less video's figure is all video, whose events ProseMirror never
  // sees, so the node view has to select it on a click itself.
  describe('a video without a caption', () => {
    const bare = `# Note\n\n![](${VIDEO})\n\nafter`;

    function backspace(target: Element): KeyboardEvent {
      const event = new KeyboardEvent('keydown', {
        key: 'Backspace',
        bubbles: true,
        cancelable: true,
      });
      target.dispatchEvent(event);
      return event;
    }

    async function clickVideo(): Promise<{ view: EditorView; video: HTMLVideoElement }> {
      const view = await createEditor(bare);
      const video = document.querySelector<HTMLVideoElement>('.ProseMirror video')!;
      expect(video.closest('figure')?.querySelector('figcaption')).toBeNull();
      const click = leftClick();
      video.dispatchEvent(click);
      // The browser still gets the click, for the controls.
      expect(click.defaultPrevented).toBe(false);
      expect(view.state.selection).toBeInstanceOf(NodeSelection);
      expect(view.state.selection.from).toBe(positionOf(view, VIDEO));
      expect(
        document.querySelector('.markdown-media-video.ProseMirror-selectednode')
      ).not.toBeNull();
      return { view, video };
    }

    it('is selected by a click and deleted by Backspace while it has focus', async () => {
      const { video } = await clickVideo();

      expect(backspace(video).defaultPrevented).toBe(true);
      expect(currentMarkdown()).not.toContain(VIDEO);
      expect(currentMarkdown()).toContain('after');

      editor!.action(callCommand(undoCommand.key));
      expect(currentMarkdown()).toContain(`![](${VIDEO})`);
      expect(figures()[0].querySelector('video')).not.toBeNull();
    });

    it('is deleted by Backspace in the editor once selected', async () => {
      const { view } = await clickVideo();

      backspace(view.dom);
      expect(currentMarkdown()).not.toContain(VIDEO);
      expect(currentMarkdown()).toContain('after');
    });

    it('keeps its keys for the browser when it is not selected', async () => {
      const { view, video } = await clickVideo();
      view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 1)));

      expect(backspace(video).defaultPrevented).toBe(false);
      expect(press(video, ' ').defaultPrevented).toBe(false);
      expect(currentMarkdown()).toContain(VIDEO);
    });

    // Focusing the editor after a click on play would hand it the next key.
    it('leaves focus alone when a click on its controls starts playback', async () => {
      const view = await createEditor(bare);
      const video = document.querySelector<HTMLVideoElement>('.ProseMirror video')!;
      stubPlayback(video);
      // The controls sit in the video's shadow tree, below its own listeners;
      // a child element stands in for them.
      const playButton = document.createElement('span');
      video.append(playButton);
      playButton.addEventListener('click', () => {
        if (video.paused) void video.play();
        else video.pause();
      });
      const focus = vi.spyOn(view, 'focus');

      playButton.dispatchEvent(leftClick());
      expect(video.paused).toBe(false);
      expect(view.state.selection).toBeInstanceOf(NodeSelection);
      expect(focus).not.toHaveBeenCalled();

      // Pausing is an ordinary click.
      playButton.dispatchEvent(leftClick());
      expect(video.paused).toBe(true);
      expect(focus).toHaveBeenCalled();
    });
  });

  // Over a selected node, Space and Enter would replace it (Enter by splitting
  // the block), and clicking play selects the video.
  describe('a selected video', () => {
    async function selectVideo(): Promise<{ view: EditorView; video: HTMLVideoElement }> {
      const view = await createEditor(note);
      const video = figures()[1].querySelector('video')!;
      stubPlayback(video);
      view.dispatch(
        view.state.tr.setSelection(NodeSelection.create(view.state.doc, positionOf(view, VIDEO)))
      );
      return { view, video };
    }

    it.each([' ', 'Enter'])(
      'plays and pauses on %j, in the editor or on the video, without editing',
      async (key) => {
        const { view, video } = await selectVideo();
        const before = currentMarkdown();

        for (const target of [view.dom, video]) {
          expect(press(target, key).defaultPrevented).toBe(true);
          expect(video.paused).toBe(false);
          expect(press(target, key).defaultPrevented).toBe(true);
          expect(video.paused).toBe(true);
        }
        expect(currentMarkdown()).toBe(before);
        expect(view.state.selection).toBeInstanceOf(NodeSelection);
        expect(view.state.selection.from).toBe(positionOf(view, VIDEO));
      }
    );

    it('is typed over by a letter, as a selected image is', async () => {
      const { view, video } = await selectVideo();

      expect(press(view.dom, 'a').defaultPrevented).toBe(false);
      expect(currentMarkdown()).not.toContain(VIDEO);
      expect(currentMarkdown()).toContain('between\n\na');
      expect(video.play).not.toHaveBeenCalled();

      editor!.action(callCommand(undoCommand.key));
      expect(currentMarkdown()).toContain(`![Clip](${VIDEO})`);
    });

    it('leaves Space to the text once something else is selected', async () => {
      const { view, video } = await selectVideo();
      view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 1)));

      expect(press(view.dom, ' ').defaultPrevented).toBe(false);
      expect(video.play).not.toHaveBeenCalled();
      expect(currentMarkdown()).toContain(`![Clip](${VIDEO})`);
    });
  });
});
