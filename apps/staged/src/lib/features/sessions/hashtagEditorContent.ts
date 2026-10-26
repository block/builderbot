/**
 * hashtagEditorContent.ts — The composer's DOM for a value with `#type:id` references
 *
 * HashtagInput shows its raw value with each resolvable reference as an inline
 * badge. The nodes are built here so a paste turns into the same shape as the
 * initial render, and so that shape can be tested without mounting the editor.
 */
import type { HashtagItem } from '../../types';
import { HASHTAG_TOKEN_RE, escapeHtml, hashtagTypeIconSvg } from './hashtagItems';

/** Follows every badge so the caret has somewhere to land after it. */
export const BADGE_CARET_ANCHOR = '\u200B';

export function createBadgeElement(item: HashtagItem): HTMLSpanElement {
  const badge = document.createElement('span');
  badge.className = 'hashtag-badge stable-raster stable-raster-glyphs';
  badge.contentEditable = 'false';
  badge.dataset.token = `#${item.type}:${item.id}`;
  badge.title = item.title;
  const iconSvg = hashtagTypeIconSvg[item.type] ?? '';
  badge.innerHTML = `${iconSvg}<span class="hashtag-badge-label">${escapeHtml(item.title)}</span>`;
  badge.style.cssText = `background: var(${item.bgColor}); color: var(${item.color});`;
  return badge;
}

/**
 * The editor nodes for `text`: a badge and caret anchor for each reference
 * found in `itemsByKey`, a `<br>` for each newline, and text for the rest. A
 * reference no item resolves stays as text; `unresolved` reports whether there
 * was one, so the editor can render again once items arrive.
 */
export function hashtagContentFragment(
  text: string,
  itemsByKey: ReadonlyMap<string, HashtagItem>
): { fragment: DocumentFragment; unresolved: boolean } {
  const fragment = document.createDocumentFragment();
  const regex = new RegExp(HASHTAG_TOKEN_RE.source, 'g');
  let lastIndex = 0;
  let unresolved = false;
  let match: RegExpExecArray | null;

  while ((match = regex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      appendTextNodes(fragment, text.slice(lastIndex, match.index));
    }

    const item = itemsByKey.get(`${match[1]}:${match[2]}`);
    if (item) {
      fragment.appendChild(createBadgeElement(item));
      fragment.appendChild(document.createTextNode(BADGE_CARET_ANCHOR));
    } else {
      fragment.appendChild(document.createTextNode(match[0]));
      unresolved = true;
    }

    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    appendTextNodes(fragment, text.slice(lastIndex));
  }

  return { fragment, unresolved };
}

function appendTextNodes(parent: ParentNode, text: string) {
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    if (i > 0) parent.appendChild(document.createElement('br'));
    if (lines[i]) parent.appendChild(document.createTextNode(lines[i]));
  }
}
