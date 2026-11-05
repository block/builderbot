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
import { NodeSelection } from '@milkdown/kit/prose/state';
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
});
