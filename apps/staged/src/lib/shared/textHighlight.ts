/**
 * textHighlight.ts — Text search and highlighting utilities
 *
 * Provides functions to search through DOM text nodes, highlight matches,
 * and navigate between them.
 */

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
 * @param container - The container element to clear highlights from
 */
export function clearHighlights(container: HTMLElement): void {
  const marks = container.querySelectorAll('mark.search-match');

  marks.forEach((mark) => {
    const text = mark.textContent || '';
    const textNode = document.createTextNode(text);
    mark.parentNode?.replaceChild(textNode, mark);
  });

  // Normalize to merge adjacent text nodes
  container.normalize();
}

/**
 * Scroll an element into view with smooth scrolling.
 * Centers the element in the viewport.
 *
 * @param element - The element to scroll to
 */
export function scrollToMatch(element: HTMLElement): void {
  element.scrollIntoView({
    behavior: 'smooth',
    block: 'center',
    inline: 'nearest',
  });
}
