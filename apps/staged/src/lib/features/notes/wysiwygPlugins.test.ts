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

describe('taskListInputRule', () => {
  it.each([
    ['[ ] ', false],
    ['[] ', false],
    ['[x] ', true],
    ['[X] ', true],
  ])('turns %j on a plain paragraph into a task item', async (typed, checked) => {
    const view = await createEditor('# Title\n\nseed');
    selectStartOf(view, 'paragraph');
    type(view, typed);

    const list = view.state.doc.child(1);
    expect(list.type.name).toBe('bullet_list');
    expect(list.firstChild?.type.name).toBe('list_item');
    expect(list.firstChild?.attrs.checked).toBe(checked);
    expect(list.firstChild?.textContent).toBe('seed');
  });

  it('serializes the converted item as task markdown, not escaped brackets', async () => {
    const view = await createEditor('# Title\n\nhello');
    selectStartOf(view, 'paragraph');
    type(view, '[ ] ');

    expect(currentMarkdown()).toMatch(/[-*] \[ \] hello/);
    expect(currentMarkdown()).not.toContain('\\[');
  });

  it('turns `[] ` inside an existing list item into a task item in place', async () => {
    const view = await createEditor('# Title\n\n- seed');
    selectStartOf(view, 'paragraph');
    type(view, '[] ');

    const item = view.state.doc.child(1).firstChild;
    expect(item?.type.name).toBe('list_item');
    expect(item?.attrs.checked).toBe(false);
    expect(item?.textContent).toBe('seed');
  });

  it('leaves an already-task item alone', async () => {
    const view = await createEditor('# Title\n\n- [x] seed');
    selectStartOf(view, 'paragraph');
    type(view, '[ ] ');

    const item = view.state.doc.child(1).firstChild;
    expect(item?.attrs.checked).toBe(true);
    expect(item?.textContent).toBe('[ ] seed');
  });
});

describe('firstLineTitlePlugin', () => {
  it('promotes the first line to the title H1 as it is typed', async () => {
    const view = await createEditor('');
    type(view, 'Hello');

    const first = view.state.doc.firstChild;
    expect(first?.type.name).toBe('heading');
    expect(first?.attrs.level).toBe(1);
    expect(currentMarkdown().startsWith('# Hello')).toBe(true);
  });

  // The first line is the note's title by design, so task syntax there stays
  // title text; checkboxes start on the second line.
  it('keeps task syntax on the first line as title text', async () => {
    const view = await createEditor('');
    type(view, '[ ] hello');

    const first = view.state.doc.firstChild;
    expect(first?.type.name).toBe('heading');
    expect(first?.textContent).toBe('[ ] hello');
  });

  it('does not re-level a heading the user demoted', async () => {
    const view = await createEditor('## Sub');
    const end = view.state.doc.firstChild!.nodeSize - 1;
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, end)));
    type(view, '!');

    const first = view.state.doc.firstChild;
    expect(first?.type.name).toBe('heading');
    expect(first?.attrs.level).toBe(2);
  });

  it('promotes body paragraphs only in first position', async () => {
    const view = await createEditor('# Title\n\nbody');
    selectStartOf(view, 'paragraph');
    type(view, 'more ');

    expect(view.state.doc.child(1).type.name).toBe('paragraph');
  });
});
