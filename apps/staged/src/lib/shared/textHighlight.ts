/**
 * textHighlight.ts — Text search and highlighting utilities
 *
 * Provides functions to search through DOM text nodes, highlight matches,
 * and navigate between them.
 */

import { revealWithin } from './revealWithin';

export interface HighlightResult {
  total: number;
  elements: HTMLElement[];
}

/**
 * Escape special regex characters in a search query
 */
function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Highlight all matches of a query string in a container element.
 * Wraps matches in <mark> tags with appropriate classes.
 *
 * @param container - The container element to search within
 * @param query - The search query string
 * @param currentIndex - The index of the current active match (0-based)
 * @returns Object with total match count and array of match elements
 */
export function highlightMatches(
  container: HTMLElement,
  query: string,
  currentIndex: number
): HighlightResult {
  if (!query.trim()) {
    return { total: 0, elements: [] };
  }

  const matchElements: HTMLElement[] = [];
  const regex = new RegExp(escapeRegex(query), 'gi');

  // Use TreeWalker to traverse text nodes only
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) => {
      // Skip text nodes inside <mark> tags (already highlighted)
      if (node.parentElement?.tagName === 'MARK') {
        return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });

  const textNodes: Text[] = [];
  let node: Node | null;
  while ((node = walker.nextNode())) {
    textNodes.push(node as Text);
  }

  // Process each text node
  for (const textNode of textNodes) {
    const text = textNode.textContent || '';
    const matches = Array.from(text.matchAll(regex));

    if (matches.length === 0) continue;

    const fragment = document.createDocumentFragment();
    let lastIndex = 0;

    for (const match of matches) {
      const matchStart = match.index!;
      const matchEnd = matchStart + match[0].length;

      // Add text before match
      if (matchStart > lastIndex) {
        fragment.appendChild(document.createTextNode(text.slice(lastIndex, matchStart)));
      }

      // Create mark element for match
      const mark = document.createElement('mark');
      mark.className = 'search-match';
      mark.setAttribute('data-match-index', String(matchElements.length));
      mark.textContent = match[0];

      // Add current class if this is the active match
      if (matchElements.length === currentIndex) {
        mark.classList.add('search-match-current');
      }

      matchElements.push(mark);
      fragment.appendChild(mark);

      lastIndex = matchEnd;
    }

    // Add remaining text after last match
    if (lastIndex < text.length) {
      fragment.appendChild(document.createTextNode(text.slice(lastIndex)));
    }

    // Replace original text node with fragment
    textNode.parentNode?.replaceChild(fragment, textNode);
  }

  return {
    total: matchElements.length,
    elements: matchElements,
  };
}

/**
 * Clear all search highlights from a container element.
 * Removes <mark> tags and restores original text.
 *
 * Each mark's text is merged back into the text `highlightMatches` split
 * off around it, by hand rather than with `normalize()`. `normalize()` also
 * deletes every empty text node in the container, and Svelte uses empty text
 * nodes as block anchors: a block whose anchor is gone renders nothing the
 * next time its contents change.
 *
 * @param container - The container element to clear highlights from
 */
export function clearHighlights(container: HTMLElement): void {
  const marks = container.querySelectorAll('mark.search-match');

  marks.forEach((mark) => {
    const parent = mark.parentNode;
    if (!parent) return;
    const text = mark.textContent || '';
    const prev = mark.previousSibling;
    const next = mark.nextSibling;

    if (isNonEmptyText(prev)) {
      prev.data += text;
      mark.remove();
      if (isNonEmptyText(next)) {
        prev.data += next.data;
        next.remove();
      }
    } else if (isNonEmptyText(next)) {
      next.data = text + next.data;
      mark.remove();
    } else {
      parent.replaceChild(document.createTextNode(text), mark);
    }
  });
}

/**
 * The pieces `highlightMatches` splits a text node into are never empty, so
 * only non-empty neighbours are merged: an empty one may be a Svelte anchor.
 */
function isNonEmptyText(node: Node | null): node is Text {
  return node !== null && node.nodeType === Node.TEXT_NODE && (node as Text).data.length > 0;
}

/**
 * Scroll an element into view with smooth scrolling.
 * Centers the element in the viewport.
 *
 * @param element - The element to scroll to
 * @param within - The box being searched. When given, only it and the
 *   scrollers inside it move (see revealWithin), not the boxes around it.
 */
export function scrollToMatch(element: HTMLElement, within?: HTMLElement): void {
  const options = { behavior: 'smooth', block: 'center', inline: 'nearest' } as const;
  if (within) revealWithin(element, within, options);
  else element.scrollIntoView(options);
}
