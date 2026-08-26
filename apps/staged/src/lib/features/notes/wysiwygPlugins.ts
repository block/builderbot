/**
 * wysiwygPlugins.ts — Milkdown plugins the written-note editor adds on top of Crepe
 *
 * Imported lazily next to `@milkdown/crepe` (both pull in ProseMirror) and
 * registered on the underlying Milkdown editor before `create()`.
 */
import { InputRule } from '@milkdown/kit/prose/inputrules';
import { Plugin, PluginKey } from '@milkdown/kit/prose/state';
import { findWrapping } from '@milkdown/kit/prose/transform';
import { $inputRule, $prose } from '@milkdown/kit/utils';

/**
 * Convert `[ ] `, `[x] ` — and the lazier `[] ` — into a task checkbox.
 *
 * The GFM preset ships this conversion only for text already inside a list
 * item (typed after `- `); on a plain paragraph the brackets stay literal
 * text, which the markdown serializer then escapes to `\[ ]` on save and
 * copy. Wrap the paragraph into a fresh task list instead.
 */
const taskListInputRule = $inputRule(() => {
  return new InputRule(/^\[(?<checked>[ xX])?\]\s$/, (state, match, start, end) => {
    const { paragraph, list_item: listItem, bullet_list: bulletList } = state.schema.nodes;
    if (!paragraph || !listItem || !bulletList) return null;
    const $start = state.doc.resolve(start);
    if ($start.parent.type !== paragraph) return null;
    const checked = (match.groups?.checked ?? '').toLowerCase() === 'x';

    // Already in a list item: make it a task item in place. The preset's own
    // rule does this for `[ ]`/`[x]`; this also accepts `[]`.
    const item = $start.node(-1);
    if (item.type === listItem) {
      if (item.attrs.checked != null) return null;
      return state.tr
        .delete(start, end)
        .setNodeMarkup($start.before(-1), undefined, { ...item.attrs, checked });
    }

    const tr = state.tr.delete(start, end);
    const range = tr.doc.resolve(start).blockRange();
    const wrapping = range && findWrapping(range, bulletList);
    if (!range || !wrapping) return null;
    tr.wrap(range, wrapping);
    // wrap() opens the wrappers just before the paragraph: the bullet list at
    // range.start, the list item one token further in.
    const itemPos = range.start + wrapping.length - 1;
    const wrapped = tr.doc.nodeAt(itemPos);
    if (wrapped?.type !== listItem) return null;
    return tr.setNodeMarkup(itemPos, undefined, { ...wrapped.attrs, checked });
  });
});

/**
 * Keep a non-empty first line a level-1 heading — the note's title.
 *
 * Written notes have no separate title field: on save the leading H1 becomes
 * the title, and without one `splitNoteMarkdown` drafts the first line anyway.
 * Promoting the first block as it's typed shows that contract instead of
 * hiding it — the title line looks like the title it will become — and keeps
 * the round-trip through `noteMarkdownWithTitle` on the H1 path rather than
 * prepending a duplicate of the first line on the next edit.
 *
 * Only non-empty paragraphs are promoted: a fresh note's empty first block
 * stays a paragraph so the doc still counts as empty (the placeholder shows,
 * the trailing-paragraph plugin stays idle), and a heading the user demoted
 * to another level on purpose is their call. Composition transactions are
 * skipped — retyping the block under an active IME session would break it —
 * so a composed title is promoted on the next ordinary edit instead.
 */
const firstLineTitlePlugin = $prose(() => {
  return new Plugin({
    key: new PluginKey('WRITTEN_NOTE_TITLE'),
    appendTransaction: (transactions, _oldState, state) => {
      if (!transactions.some((tr) => tr.docChanged && !tr.getMeta('composition'))) return null;
      const heading = state.schema.nodes.heading;
      const first = state.doc.firstChild;
      if (!heading || !first) return null;
      if (first.type.name !== 'paragraph' || first.content.size === 0) return null;
      return state.tr.setBlockType(1, 1, heading, { level: 1 });
    },
  });
});

export const wysiwygPlugins = [taskListInputRule, firstLineTitlePlugin];
