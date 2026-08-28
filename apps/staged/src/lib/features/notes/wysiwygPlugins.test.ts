// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';

import { Editor, defaultValueCtx, editorViewCtx, rootCtx } from '@milkdown/kit/core';
import { commonmark } from '@milkdown/kit/preset/commonmark';
import { gfm } from '@milkdown/kit/preset/gfm';
import { TextSelection } from '@milkdown/kit/prose/state';
import type { EditorView } from '@milkdown/kit/prose/view';
import { getMarkdown } from '@milkdown/kit/utils';

import { wysiwygPlugins } from './wysiwygPlugins';

// The editor under test is Milkdown with the same presets Crepe layers its
// features over; the plugins only touch the schema and input pipeline, which
// these presets define.
let editor: Editor | null = null;

async function createEditor(markdown: string): Promise<EditorView> {
  editor = await Editor.make()
    .config((ctx) => {
      ctx.set(rootCtx, document.body);
      ctx.set(defaultValueCtx, markdown);
    })
    .use(commonmark)
    .use(gfm)
    .use(wysiwygPlugins)
    .create();
  return editor.ctx.get(editorViewCtx);
}

afterEach(async () => {
  await editor?.destroy();
  editor = null;
  document.body.innerHTML = '';
});

/** Feed text through the same path the DOM does: input rules first. */
function type(view: EditorView, text: string) {
  for (const char of text) {
    const { from, to } = view.state.selection;
    const insert = () => view.state.tr.insertText(char, from, to);
    const handled = view.someProp('handleTextInput', (handler) =>
      handler(view, from, to, char, insert)
    );
    if (!handled) view.dispatch(insert());
  }
}

/** Place the cursor at the start of the first block of the given type. */
function selectStartOf(view: EditorView, typeName: string) {
  let inside = -1;
  view.state.doc.descendants((node, pos) => {
    if (inside === -1 && node.type.name === typeName) inside = pos + 1;
    return inside === -1;
  });
  expect(inside).toBeGreaterThan(-1);
  view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, inside)));
}

function currentMarkdown(): string {
  return editor?.action(getMarkdown()) ?? '';
}

describe('firstLineTitlePlugin', () => {
  it('promotes the first line to the title H1 as it is typed', async () => {
    const view = await createEditor('');
    type(view, 'Hello');

    const first = view.state.doc.firstChild;
    expect(first?.type.name).toBe('heading');
    expect(first?.attrs.level).toBe(1);
    expect(currentMarkdown().startsWith('# Hello')).toBe(true);
  });

  it('re-levels a deeper heading on the first line', async () => {
    const view = await createEditor('## Sub');
    const end = view.state.doc.firstChild!.nodeSize - 1;
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, end)));
    type(view, '!');

    const first = view.state.doc.firstChild;
    expect(first?.type.name).toBe('heading');
    expect(first?.attrs.level).toBe(1);
    expect(currentMarkdown().startsWith('# Sub!')).toBe(true);
  });

  it('leaves deeper headings alone below the first line', async () => {
    const view = await createEditor('# Title\n\n## Sub\n\nbody');
    selectStartOf(view, 'paragraph');
    type(view, 'more ');

    expect(view.state.doc.child(1).attrs.level).toBe(2);
  });

  it('leaves a document that opens with a list alone', async () => {
    const view = await createEditor('- item');
    selectStartOf(view, 'paragraph');
    type(view, 'more ');

    expect(view.state.doc.firstChild?.type.name).toBe('bullet_list');
    expect(currentMarkdown().includes('more item')).toBe(true);
  });

  it('leaves a first line that is only an image as a paragraph', async () => {
    const view = await createEditor('![Screenshot](shot.png)');
    selectStartOf(view, 'paragraph');
    type(view, 'x');

    // An H1 here would promise a title the save path won't store.
    expect(view.state.doc.firstChild?.type.name).toBe('paragraph');
  });

  it('leaves a first line holding a link as a paragraph', async () => {
    const view = await createEditor('See [the docs](https://example.com)');
    selectStartOf(view, 'paragraph');
    type(view, 'x ');

    expect(view.state.doc.firstChild?.type.name).toBe('paragraph');
  });

  it('demotes the title when a URL is typed into it', async () => {
    const view = await createEditor('# Read this');
    const end = view.state.doc.firstChild!.nodeSize - 1;
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, end)));
    type(view, ' https://example.com');

    expect(view.state.doc.firstChild?.type.name).toBe('paragraph');
    // The serializer escapes the colon so the line can't autolink; the saved
    // markdown still opens with the demoted paragraph rather than an H1.
    expect(currentMarkdown().startsWith('Read this https\\://example.com')).toBe(true);
  });

  it('promotes body paragraphs only in first position', async () => {
    const view = await createEditor('# Title\n\nbody');
    selectStartOf(view, 'paragraph');
    type(view, 'more ');

    expect(view.state.doc.child(1).type.name).toBe('paragraph');
  });
});
