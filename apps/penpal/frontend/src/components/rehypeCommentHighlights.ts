import { visit, SKIP } from 'unist-util-visit';
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

/**
 * Rehype plugin that inserts <mark> elements into the hast AST for comment highlights.
 * Replaces the previous DOM mutation approach (addCommentHighlights) so that React
 * owns the marks and reconciliation works correctly on re-render.
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
    visit(tree, 'element', (node: Element, index, parent) => {
      if (index === undefined || !parent) return;

      const sourceLine = node.position?.start?.line;
      if (!sourceLine) return;

      const lineHighlights = byLine.get(sourceLine);
      if (!lineHighlights) return;

      // Process each highlight for this block, then skip children
      // to avoid double-applying when parent and child share a start line
      // (e.g. <li> and its child <p> both start on the same line).
      for (const highlight of lineHighlights) {
        applyHighlight(node, highlight);
      }
      return SKIP;
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

/**
 * Applies a single highlight to a hast element by finding the selectedText
 * in the element's accumulated text content and wrapping matching portions
 * in <mark> elements.
 */
function applyHighlight(element: Element, highlight: ThreadHighlight) {
  const { nodes, text } = collectTextNodes(element);
  if (nodes.length === 0) return;

  // Normalize whitespace for matching (mirrors DOM-based approach)
  const normalizedText = text.replace(/\s+/g, ' ');
  const normalizedSelected = highlight.selectedText.replace(/\s+/g, ' ');

  // Use occurrenceIndex to find the Nth match within the block,
  // falling back to the first occurrence if the index is not set or not found.
  let matchIndex = -1;
  const targetOccurrence = highlight.occurrenceIndex ?? 0;
  let pos = 0;
  for (let i = 0; i <= targetOccurrence; i++) {
    const found = normalizedText.indexOf(normalizedSelected, pos);
    if (found === -1) break;
    if (i === targetOccurrence) {
      matchIndex = found;
    }
    pos = found + 1;
  }
  // Fall back to first occurrence if Nth not found
  if (matchIndex === -1) matchIndex = normalizedText.indexOf(normalizedSelected);
  if (matchIndex === -1) return;

  const matchStart = matchIndex;
  const matchEnd = matchIndex + normalizedSelected.length;

  // Map normalized positions back to original text positions.
  // Build a mapping from normalized index -> original index.
  const normToOrig: number[] = [];
  let ni = 0;
  for (let oi = 0; oi < text.length; oi++) {
    if (/\s/.test(text[oi])) {
      // In normalized form, consecutive whitespace maps to a single space
      if (ni < normalizedText.length && normalizedText[ni] === ' ') {
        normToOrig.push(oi);
        ni++;
      }
      // Skip additional whitespace chars in original
    } else {
      normToOrig.push(oi);
      ni++;
    }
  }
  // Sentinel for end mapping
  normToOrig.push(text.length);

  const origMatchStart = normToOrig[matchStart];
  const origMatchEnd = normToOrig[matchEnd];

  // Process text nodes in reverse order to preserve indices
  for (let i = nodes.length - 1; i >= 0; i--) {
    const entry = nodes[i];
    const nodeStart = entry.start;

    // Check overlap with match region
    const overlapStart = Math.max(origMatchStart - nodeStart, 0);
    const overlapEnd = Math.min(origMatchEnd - nodeStart, entry.node.value.length);
    if (overlapStart >= overlapEnd) continue;

    const nodeValue = entry.node.value;
    const newNodes: ElementContent[] = [];

    // Text before the highlight
    if (overlapStart > 0) {
      newNodes.push({ type: 'text', value: nodeValue.slice(0, overlapStart) } as Text);
    }

    // The highlighted portion wrapped in <mark>
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

    // Text after the highlight
    if (overlapEnd < nodeValue.length) {
      newNodes.push({ type: 'text', value: nodeValue.slice(overlapEnd) } as Text);
    }

    // Replace the original text node in its parent
    const parentChildren = (entry.parent as Element).children;
    parentChildren.splice(entry.index, 1, ...newNodes);

    // Adjust indices for any earlier entries that share the same parent
    // (only needed if multiple text nodes in the same parent, which is rare)
    const inserted = newNodes.length - 1; // net new nodes
    if (inserted !== 0) {
      for (let j = i - 1; j >= 0; j--) {
        if (nodes[j].parent === entry.parent && nodes[j].index > entry.index) {
          nodes[j].index += inserted;
        }
      }
    }
  }
}
