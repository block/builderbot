/**
 * wysiwygPlugins.ts — Milkdown plugins the written-note editor adds on top of Crepe
 *
 * Imported lazily next to `@milkdown/crepe` (both pull in ProseMirror) and
 * registered on the underlying Milkdown editor before `create()`.
 */
import type { Node } from '@milkdown/kit/prose/model';
import { Plugin, PluginKey } from '@milkdown/kit/prose/state';
import { $prose } from '@milkdown/kit/utils';

import { canBeNoteTitleText } from './noteMarkdown';

/**
 * Keep the first line a level-1 heading exactly when it can be the note's title.
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
 * The reverse holds too. A line holding a link or an image can't be the title —
 * it would be stored as raw syntax, so `splitNoteMarkdown` leaves it in the body
 * and names the note Untitled — and dressing it as an H1 would promise otherwise.
 * Such a first block is left as a paragraph, and a title that stops qualifying
 * (a link pasted into it) is demoted back to one, so what reads as a title on
 * screen is what gets saved as the title.
 *
 * Only non-empty top-level text blocks are touched: a fresh note's empty first
 * block stays a paragraph so the doc still counts as empty (the placeholder
 * shows, the trailing-paragraph plugin stays idle), and a document opening with
 * a list or a code fence keeps its structure — those can't be titles either, and
 * `splitNoteMarkdown` treats them the same way. Composition transactions are
 * skipped — retyping the block under an active IME session would break it — so a
 * composed title is promoted on the next ordinary edit instead.
 */
const firstLineTitlePlugin = $prose(() => {
  return new Plugin({
    key: new PluginKey('WRITTEN_NOTE_TITLE'),
    appendTransaction: (transactions, _oldState, state) => {
      if (!transactions.some((tr) => tr.docChanged && !tr.getMeta('composition'))) return null;
      const { heading, paragraph } = state.schema.nodes;
      const first = state.doc.firstChild;
      if (!heading || !paragraph || !first) return null;

      const isHeading = first.type === heading;
      if (!isHeading && first.type !== paragraph) return null;

      if (canHoldTitle(first)) {
        if (isHeading && first.attrs.level === 1) return null;
        return state.tr.setBlockType(1, 1, heading, { level: 1 });
      }
      if (!isHeading || first.attrs.level !== 1) return null;
      return state.tr.setBlockType(1, 1, paragraph);
    },
  });
});

/**
 * Whether this block would survive the trip through the title column.
 *
 * The markdown rule (`canBeNoteTitleLine`) runs on the serialized line, so it
 * sees `[docs](url)` as text. Here the same content is already parsed — a link is
 * a mark and an image is a node with no text at all — so the structure is checked
 * directly and the text is passed through the shared rule for what it can still
 * catch, such as a bare URL typed out.
 *
 * The text form of that rule is the one to call: `textContent` is plain already,
 * so the serializer's escapes have never been added to it and there is nothing to
 * undo.
 */
function canHoldTitle(node: Node): boolean {
  if (node.content.size === 0) return false;
  let holdsLinkOrImage = false;
  node.descendants((child) => {
    if (holdsLinkOrImage) return false;
    if (child.type.name === 'image' || child.marks.some((mark) => mark.type.name === 'link')) {
      holdsLinkOrImage = true;
    }
    return !holdsLinkOrImage;
  });
  return !holdsLinkOrImage && canBeNoteTitleText(node.textContent);
}

export const wysiwygPlugins = [firstLineTitlePlugin];
