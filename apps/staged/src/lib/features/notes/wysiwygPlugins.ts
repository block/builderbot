/**
 * wysiwygPlugins.ts — Milkdown plugins the written-note editor adds on top of Crepe
 *
 * Imported lazily next to `@milkdown/crepe` (both pull in ProseMirror) and
 * registered on the underlying Milkdown editor before `create()`.
 */
import { Plugin, PluginKey } from '@milkdown/kit/prose/state';
import { $prose } from '@milkdown/kit/utils';

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

export const wysiwygPlugins = [firstLineTitlePlugin];
