import { visit } from 'unist-util-visit';
import type { Root, Element, Text, ElementContent } from 'hast';

export interface ThreadHighlight {
  threadId: string;
  selectedText: string;
  startLine: number;
  occurrenceIndex?: number;
  pending?: boolean;
}

interface Options {
  highlights: ThreadHighlight[];
}

// E-PENPAL-HIGHLIGHT-REHYPE: rehype plugin injecting <mark> elements for comment highlights.
/**
 * Rehype plugin that inserts <mark> elements into the hast AST for comment highlights.
 * Replaces the previous DOM mutation approach (addCommentHighlights) so that React
 * owns the marks and reconciliation works correctly on re-render.
 *
 * Supports cross-element selections: when selectedText spans multiple block elements,
 * the plugin highlights the matching prefix in the start element and continues
 * highlighting in subsequent sibling elements until the full text is covered.
 */
export default function rehypeCommentHighlights(options: Options) {
  const { highlights } = options;
  if (!highlights || highlights.length === 0) return (tree: Root) => tree;

  // Group highlights by startLine
  const byLine = new Map<number, ThreadHighlight[]>();
  for (const h of highlights) {
    const list = byLine.get(h.startLine) || [];
    list.push(h);
    byLine.set(h.startLine, list);
  }

  return (tree: Root) => {
    // Track which highlights have been started to avoid double-applying
    const applied = new Set<string>();
    // Cross-element highlights needing continuation: threadId → remaining normalized text
    const continuing = new Map<string, { highlight: ThreadHighlight; remaining: string }>();

    visit(tree, 'element', (node: Element, index, parent) => {
      if (index === undefined || !parent) return;

      const sourceLine = node.position?.start?.line;
      if (!sourceLine) return;

      // Continue cross-element highlights into subsequent elements.
      // Only try elements on lines AFTER the highlight's startLine to avoid
      // double-matching in child elements of the start element (whose text
      // was already covered by collectTextNodes on the parent).
      for (const [threadId, state] of continuing) {
        if (sourceLine <= state.highlight.startLine) continue;
        const matched = applyContinuation(node, state.highlight, state.remaining);
        if (matched > 0) {
          const newRemaining = state.remaining.slice(matched).trim();
          if (newRemaining.length < 5) {
            continuing.delete(threadId);
          } else {
            state.remaining = newRemaining;
          }
        }
      }

      // Start new highlights at this line
      const lineHighlights = byLine.get(sourceLine);
      if (!lineHighlights) return;

      for (const highlight of lineHighlights) {
        if (applied.has(highlight.threadId)) continue;
        const result = applyHighlight(node, highlight);
        applied.add(highlight.threadId);
        if (result.remaining) {
          continuing.set(highlight.threadId, { highlight, remaining: result.remaining });
        }
      }
    });
  };
}

/**
 * Collects all text descendants of an element, returning their text content
 * and references to the text nodes plus their offset within the accumulated string.
 */
function collectTextNodes(node: Element): { nodes: { node: Text; parent: Element | Root; index: number; start: number }[]; text: string } {
  const result: { node: Text; parent: Element | Root; index: number; start: number }[] = [];
  let text = '';

  function walk(n: ElementContent, par: Element | Root, idx: number) {
    if (n.type === 'text') {
      result.push({ node: n as Text, parent: par, index: idx, start: text.length });
      text += (n as Text).value;
    } else if (n.type === 'element' && (n as Element).children) {
      for (let i = 0; i < (n as Element).children.length; i++) {
        walk((n as Element).children[i], n as Element, i);
      }
    }
  }

  for (let i = 0; i < node.children.length; i++) {
    walk(node.children[i], node, i);
  }

  return { nodes: result, text };
}

// ── Shared normalization and mark-insertion helpers ───────────────────

function normalizeHast(text: string): string {
  return text.replace(/[*_`]/g, '').replace(/\s+/g, ' ');
}

function normalizeSelected(selectedText: string): string {
  return selectedText
    .replace(/[*_`]/g, '')
    .replace(/^(?:#{1,6} |- |\* |\d+\. |> |- \[[ x]\] )/gm, '')
    .replace(/^-{3,}$/gm, '')  // thematic breaks (rendered as <hr> with no text)
    .replace(/\s+/g, ' ')
    .trim();
}

/**
 * Build a mapping from normalized-text index → original-text index.
 * Accounts for stripped formatting chars (*_`) and collapsed whitespace.
 */
function buildNormToOrigMap(text: string, normalizedText: string): number[] {
  const normToOrig: number[] = [];
  let ni = 0;
  for (let oi = 0; oi < text.length; oi++) {
    const ch = text[oi];
    if (ch === '*' || ch === '_' || ch === '`') continue;
    if (/\s/.test(ch)) {
      if (ni < normalizedText.length && normalizedText[ni] === ' ') {
        normToOrig.push(oi);
        ni++;
      }
    } else {
      normToOrig.push(oi);
      ni++;
    }
  }
  normToOrig.push(text.length); // sentinel for end mapping
  return normToOrig;
}

/**
 * Insert <mark> elements into the text nodes for the given match range
 * (specified in original/unnormalized text coordinates).
 */
function insertMarks(
  nodes: { node: Text; parent: Element | Root; index: number; start: number }[],
  origMatchStart: number,
  origMatchEnd: number,
  highlight: ThreadHighlight,
): void {
  for (let i = nodes.length - 1; i >= 0; i--) {
    const entry = nodes[i];
    const nodeStart = entry.start;

    const overlapStart = Math.max(origMatchStart - nodeStart, 0);
    const overlapEnd = Math.min(origMatchEnd - nodeStart, entry.node.value.length);
    if (overlapStart >= overlapEnd) continue;

    const nodeValue = entry.node.value;
    const newNodes: ElementContent[] = [];

    if (overlapStart > 0) {
      newNodes.push({ type: 'text', value: nodeValue.slice(0, overlapStart) } as Text);
    }

    const markElement: Element = {
      type: 'element',
      tagName: 'mark',
      properties: {
        className: highlight.pending ? ['comment-highlight', 'pending-highlight'] : ['comment-highlight'],
        dataThreadId: highlight.threadId,
      },
      children: [
        { type: 'text', value: nodeValue.slice(overlapStart, overlapEnd) } as Text,
      ],
    };
    newNodes.push(markElement);

    if (overlapEnd < nodeValue.length) {
      newNodes.push({ type: 'text', value: nodeValue.slice(overlapEnd) } as Text);
    }

    const parentChildren = (entry.parent as Element).children;
    parentChildren.splice(entry.index, 1, ...newNodes);

    const inserted = newNodes.length - 1;
    if (inserted !== 0) {
      for (let j = i - 1; j >= 0; j--) {
        if (nodes[j].parent === entry.parent && nodes[j].index > entry.index) {
          nodes[j].index += inserted;
        }
      }
    }
  }
}

// ── Highlight application ────────────────────────────────────────────

/**
 * Applies a single highlight to a hast element by finding the selectedText
 * in the element's accumulated text content and wrapping matching portions
 * in <mark> elements.
 *
 * Returns { remaining } — for cross-element selections, the normalized text
 * that was NOT matched in this element and needs continuation in subsequent elements.
 */
function applyHighlight(element: Element, highlight: ThreadHighlight): { remaining: string | null } {
  const { nodes, text } = collectTextNodes(element);
  if (nodes.length === 0) return { remaining: null };

  const normalizedText = normalizeHast(text);
  const normSelected = normalizeSelected(highlight.selectedText);

  // Use occurrenceIndex to find the Nth match within the block
  let matchIndex = -1;
  let matchLength = normSelected.length;
  const targetOccurrence = highlight.occurrenceIndex ?? 0;
  let pos = 0;
  for (let i = 0; i <= targetOccurrence; i++) {
    const found = normalizedText.indexOf(normSelected, pos);
    if (found === -1) break;
    if (i === targetOccurrence) matchIndex = found;
    pos = found + 1;
  }
  if (matchIndex === -1) matchIndex = normalizedText.indexOf(normSelected);

  let isCrossElement = false;

  // Cross-element fallback: find the longest prefix of selectedText
  // that matches within this element. The remainder will be highlighted
  // in subsequent elements via the continuation mechanism.
  if (matchIndex === -1 && normSelected.length > 10) {
    let lo = 10, hi = normSelected.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >>> 1;
      if (normalizedText.indexOf(normSelected.slice(0, mid)) !== -1) {
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    const prefixLen = hi;
    if (prefixLen >= 10) {
      matchIndex = normalizedText.indexOf(normSelected.slice(0, prefixLen));
      matchLength = prefixLen;
      isCrossElement = true;
    }
  }

  if (matchIndex === -1) return { remaining: null };

  const normToOrig = buildNormToOrigMap(text, normalizedText);
  const origMatchStart = normToOrig[matchIndex];
  const origMatchEnd = normToOrig[matchIndex + matchLength];

  insertMarks(nodes, origMatchStart, origMatchEnd, highlight);

  if (isCrossElement) {
    const remaining = normSelected.slice(matchLength).trim();
    return { remaining: remaining.length >= 5 ? remaining : null };
  }
  return { remaining: null };
}

/**
 * Continue a cross-element highlight into a subsequent element.
 * Searches for the remaining normalized text (or a prefix of it) in the
 * element's text and applies <mark> elements for the matched portion.
 *
 * Returns the number of normalized characters matched (0 if no match).
 */
function applyContinuation(element: Element, highlight: ThreadHighlight, remaining: string): number {
  const { nodes, text } = collectTextNodes(element);
  if (nodes.length === 0) return 0;

  const normalizedText = normalizeHast(text);

  // Try full remaining first
  let matchIndex = normalizedText.indexOf(remaining);
  let matchLength = remaining.length;

  if (matchIndex === -1) {
    // Binary search for the longest prefix of remaining in this element
    let lo = 1, hi = Math.min(remaining.length, normalizedText.length);
    while (lo <= hi) {
      const mid = (lo + hi) >>> 1;
      if (normalizedText.indexOf(remaining.slice(0, mid)) !== -1) {
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    matchLength = hi;
    if (matchLength < 3) return 0;
    matchIndex = normalizedText.indexOf(remaining.slice(0, matchLength));
  }

  if (matchIndex === -1) return 0;

  const normToOrig = buildNormToOrigMap(text, normalizedText);
  const origMatchStart = normToOrig[matchIndex];
  const origMatchEnd = normToOrig[matchIndex + matchLength];

  insertMarks(nodes, origMatchStart, origMatchEnd, highlight);

  return matchLength;
}
