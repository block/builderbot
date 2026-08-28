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
 * The editor has no separate title field: on save `splitNoteMarkdown` takes the
 * first line as the title and stores the rest as the body, so that line leaves
 * the document either way. Promoting it as it's typed shows that contract
 * instead of hiding it — the title line looks like the title it becomes, and
 * like the H1 `noteMarkdownWithTitle` puts back when the note is reopened.
 *
 * A deeper heading on the first line is re-levelled for the same reason: `## Sub`
 * on line one is still the title, and the title is stored as plain text, so the
 * extra `#` would be dropped on save regardless. Doing it in the editor makes
 * that visible while it can still be undone.
 *
 * Only non-empty top-level text blocks are promoted: a fresh note's empty first
 * block stays a paragraph so the doc still counts as empty (the placeholder
 * shows, the trailing-paragraph plugin stays idle), and a document opening with
 * a list or a code fence keeps its structure — there is no in-place promotion
 * that wouldn't mangle it, and its first line simply becomes the title text as
 * written. Composition transactions are skipped — retyping the block under an
 * active IME session would break it — so a composed title is promoted on the
 * next ordinary edit instead.
 */
const firstLineTitlePlugin = $prose(() => {
  return new Plugin({
    key: new PluginKey('WRITTEN_NOTE_TITLE'),
    appendTransaction: (transactions, _oldState, state) => {
      if (!transactions.some((tr) => tr.docChanged && !tr.getMeta('composition'))) return null;
      const heading = state.schema.nodes.heading;
      const first = state.doc.firstChild;
      if (!heading || !first || first.content.size === 0) return null;
      const promotable =
        first.type.name === 'paragraph' || (first.type === heading && first.attrs.level !== 1);
      if (!promotable) return null;
      return state.tr.setBlockType(1, 1, heading, { level: 1 });
    },
  });
});

export const wysiwygPlugins = [firstLineTitlePlugin];
