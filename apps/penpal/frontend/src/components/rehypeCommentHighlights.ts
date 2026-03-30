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
    const continuing = new Map<string, { highlight: ThreadHighlight; remaining: string; mermaidCrossed?: boolean }>();

    visit(tree, 'element', (node: Element, index, parent) => {
      if (index === undefined || !parent) return;

      // Inline <code> — already handled by parent's collectTextNodes
      if (node.tagName === 'code') return SKIP;

      // E-PENPAL-HIGHLIGHT-CROSS: fenced code blocks with syntax highlighting
      // can't have <mark> elements (SyntaxHighlighter re-parses children as a
      // string), so handleCodeBlock stores match info as data attributes instead.
      // Language-less <pre> falls through to normal mark insertion.
      if (node.tagName === 'pre') {
        // E-PENPAL-HIGHLIGHT-MEDIA: annotate mermaid blocks during continuation.
        // We can't wrap <pre> in <mark> because mermaid rendering uses imperative
        // DOM mutations (innerHTML = svg). Wrapping changes the tree structure,
        // causing React to recreate DOM nodes and lose the rendered SVG.
        // Instead, annotate the <code> element so MarkdownViewer can add highlight
        // classes to the mermaid container div (props-only change, no DOM recreation).
        const codeChild = node.children.find(
          (c): c is Element => c.type === 'element' && c.tagName === 'code'
        );
        const isMermaid = codeChild && Array.isArray(codeChild.properties?.className) &&
          (codeChild.properties.className as string[]).some(c => c === 'language-mermaid');
        if (isMermaid) {
          const sourceLine = node.position?.start?.line;
          let annotated = false;

          // Annotate mermaid during cross-element continuation
          if (continuing.size > 0 && sourceLine) {
            for (const [, state] of continuing) {
              if (sourceLine > state.highlight.startLine) {
                codeChild.properties = codeChild.properties || {};
                codeChild.properties.dataMermaidHighlight = JSON.stringify({
                  threadId: state.highlight.threadId,
                  pending: state.highlight.pending,
                });
                // Mark that this continuation crossed a mermaid block, so
                // post-mermaid matching is lenient (remaining contains SVG
                // text from sel.toString() that won't match any HAST element).
                state.mermaidCrossed = true;
                annotated = true;
                break;
              }
            }
          }

          // E-PENPAL-HIGHLIGHT-MEDIA: start highlights whose startLine falls on
          // this mermaid block. The selectedText from sel.toString() contains SVG
          // labels (not mermaid source), so we can't text-match — just annotate
          // the mermaid and schedule the full selectedText for continuation so
          // adjacent prose elements get highlighted.
          if (sourceLine) {
            for (let lineOffset = 0; lineOffset <= 3; lineOffset++) {
              const lineHighlights = byLine.get(sourceLine - lineOffset);
              if (!lineHighlights) continue;
              for (const highlight of lineHighlights) {
                if (applied.has(highlight.threadId)) continue;
                applied.add(highlight.threadId);
                if (!annotated) {
                  codeChild.properties = codeChild.properties || {};
                  codeChild.properties.dataMermaidHighlight = JSON.stringify({
                    threadId: highlight.threadId,
                    pending: highlight.pending,
                  });
                  annotated = true;
                }
                // Schedule continuation with the full text — mermaidCrossed
                // ensures lenient matching past the SVG text in remaining.
                const normSelected = normalizeSelected(highlight.selectedText);
                if (normSelected.length >= 3) {
                  continuing.set(highlight.threadId, {
                    highlight,
                    remaining: normSelected,
                    mermaidCrossed: true,
                  });
                }
              }
            }
          }

          return SKIP;
        }

        if (handleCodeBlock(node, continuing, byLine, applied)) {
          return SKIP;
        }
      }

      const sourceLine = node.position?.start?.line;
      if (!sourceLine) return;

      // E-PENPAL-HIGHLIGHT-MEDIA: wrap block-level images during continuation.
      // Images have no text content, so wrapping doesn't consume from remaining —
      // the next text element will continue matching. Only SKIP if we wrapped,
      // to avoid falling through to text-matching logic on an image-only block.
      if (isMediaOnlyBlock(node) && continuing.size > 0) {
        let wrapped = false;
        for (const [, state] of continuing) {
          if (sourceLine > state.highlight.startLine) {
            wrapNodeInMark(node, index!, parent! as Element | Root, state.highlight);
            wrapped = true;
            break;
          }
        }
        if (wrapped) return SKIP;
      }

      // Continue cross-element highlights into subsequent elements.
      // Only try elements on lines AFTER the highlight's startLine to avoid
      // double-matching in child elements of the start element (whose text
      // was already covered by collectTextNodes on the parent).
      let continuationMatched = false;
      for (const [threadId, state] of continuing) {
        if (sourceLine <= state.highlight.startLine) continue;
        const matched = applyContinuation(node, state.highlight, state.remaining, state.mermaidCrossed);
        if (matched > 0) {
          continuationMatched = true;
          const newRemaining = state.remaining.slice(matched).trim();
          if (newRemaining.length === 0) {
            continuing.delete(threadId);
          } else {
            state.remaining = newRemaining;
          }
        }
      }
      // E-PENPAL-HIGHLIGHT-MEDIA: wrap inline images after continuation marks
      if (continuationMatched) wrapInlineMedia(node);

      // Start new highlights at or near this line.
      // Check nearby lines (0-3 offset) because startLine may point to an empty
      // line or thematic break preceding the actual content element when the
      // selectedText starts with whitespace/newlines.
      for (let lineOffset = 0; lineOffset <= 3; lineOffset++) {
        const lineHighlights = byLine.get(sourceLine - lineOffset);
        if (!lineHighlights) continue;

        for (const highlight of lineHighlights) {
          if (applied.has(highlight.threadId)) continue;
          const result = applyHighlight(node, highlight);
          applied.add(highlight.threadId);
          if (result.remaining) {
            continuing.set(highlight.threadId, { highlight, remaining: result.remaining });
          } else if (!result.matched) {
            // Start element had no text (e.g. <hr>) — schedule full text for continuation
            const normSelected = normalizeSelected(highlight.selectedText);
            if (normSelected.length >= 3) {
              continuing.set(highlight.threadId, { highlight, remaining: normSelected });
            }
          }
          // E-PENPAL-HIGHLIGHT-MEDIA: wrap inline images after highlight marks
          if (result.matched) wrapInlineMedia(node);
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

// ── Shared helpers ──────────────────────────────────────────────────

/** Find the Nth occurrence of `search` in `text`. Falls back to first occurrence if target not found. */
export function nthIndexOf(text: string, search: string, occurrence: number): number {
  let pos = 0;
  for (let i = 0; i <= occurrence; i++) {
    const found = text.indexOf(search, pos);
    if (found === -1) return i === 0 ? -1 : text.indexOf(search);
    if (i === occurrence) return found;
    pos = found + 1;
  }
  return -1;
}

// ── Normalization and mark-insertion helpers ─────────────────────────

function normalizeHast(text: string): string {
  return text.replace(/[*_`]/g, '').replace(/\s+/g, ' ');
}

function normalizeSelected(selectedText: string): string {
  return selectedText
    .replace(/[*_`]/g, '')
    .replace(/^(?:#{1,6} |- |\* |\d*\. |> |- \[[ x]\] )/gm, '')
    .replace(/^-+$/gm, '')  // thematic breaks and partial fragments (rendered as <hr> with no text)
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
 * Returns { remaining, matched } — `matched` is true if any text was highlighted.
 * For cross-element selections, `remaining` is the normalized text that was NOT
 * matched in this element and needs continuation in subsequent elements.
 * When `matched` is false (e.g. element has no text like <hr>), the caller
 * should schedule the full normalizedSelected for continuation.
 */
function applyHighlight(element: Element, highlight: ThreadHighlight): { remaining: string | null; matched: boolean } {
  const { nodes, text } = collectTextNodes(element);
  if (nodes.length === 0) return { remaining: null, matched: false };

  const normalizedText = normalizeHast(text);
  const normSelected = normalizeSelected(highlight.selectedText);

  // Use occurrenceIndex to find the Nth match within the block
  let matchIndex = nthIndexOf(normalizedText, normSelected, highlight.occurrenceIndex ?? 0);
  let matchLength = normSelected.length;

  let isCrossElement = false;

  // Cross-element fallback: find the longest prefix of selectedText
  // that matches within this element. The remainder will be highlighted
  // in subsequent elements via the continuation mechanism.
  if (matchIndex === -1 && normSelected.length > 3) {
    let lo = 3, hi = normSelected.length - 1;
    while (lo <= hi) {
      const mid = (lo + hi) >>> 1;
      if (normalizedText.indexOf(normSelected.slice(0, mid)) !== -1) {
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    const prefixLen = hi;
    if (prefixLen >= 3) {
      matchIndex = normalizedText.indexOf(normSelected.slice(0, prefixLen));
      matchLength = prefixLen;
      isCrossElement = true;
    }
  }

  // Positionally-constrained match for very short prefixes (1-2 chars):
  // only accept if the element text ENDS with the prefix AND the element
  // is short (≤ 3 chars). This targets inline elements like <em>H</em>ello
  // while rejecting long elements that happen to end with the same char.
  if (matchIndex === -1 && normSelected.length > 1 && normalizedText.length <= 3) {
    const shortLen = Math.min(2, normSelected.length);
    for (let tryLen = shortLen; tryLen >= 1; tryLen--) {
      const prefix = normSelected.slice(0, tryLen);
      if (normalizedText.endsWith(prefix)) {
        matchIndex = normalizedText.length - tryLen;
        matchLength = tryLen;
        isCrossElement = true;
        break;
      }
    }
  }

  if (matchIndex === -1) return { remaining: null, matched: false };

  const normToOrig = buildNormToOrigMap(text, normalizedText);
  const origMatchStart = normToOrig[matchIndex];
  const origMatchEnd = normToOrig[matchIndex + matchLength];

  insertMarks(nodes, origMatchStart, origMatchEnd, highlight);

  if (isCrossElement) {
    const remaining = normSelected.slice(matchLength).trim();
    return { remaining: remaining.length >= 1 ? remaining : null, matched: true };
  }
  return { remaining: null, matched: true };
}

/**
 * Continue a cross-element highlight into a subsequent element.
 * Searches for the remaining normalized text (or a prefix of it) in the
 * element's text and applies <mark> elements for the matched portion.
 *
 * Returns the number of normalized characters matched (0 if no match).
 */
// Block-level tags whose children should be matched individually rather than
// as concatenated text (prevents wrong-position matches in container elements).
const BLOCK_TAGS = new Set([
  'li', 'p', 'div', 'blockquote', 'pre', 'ul', 'ol', 'table',
  'tr', 'td', 'th', 'section', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
]);

function applyContinuation(element: Element, highlight: ThreadHighlight, remaining: string, mermaidCrossed?: boolean): number {
  // Skip container elements with block children — let individual children handle
  // the continuation to avoid matching the wrong child's text when collectTextNodes
  // concatenates all descendants without separators.
  if (element.children.some(c => c.type === 'element' && BLOCK_TAGS.has((c as Element).tagName))) {
    return 0;
  }

  const { nodes, text } = collectTextNodes(element);
  if (nodes.length === 0) return 0;

  const normalizedText = normalizeHast(text);
  let matchIndex = -1;
  let matchLength = 0;
  let skippedChars = 0; // chars consumed from remaining before the match

  // Strategy 1: Try full remaining
  matchIndex = normalizedText.indexOf(remaining);
  matchLength = remaining.length;

  // Strategy 2: Binary search for longest prefix of remaining
  if (matchIndex === -1) {
    let lo = 1, hi = Math.min(remaining.length, normalizedText.length);
    while (lo <= hi) {
      const mid = (lo + hi) >>> 1;
      if (normalizedText.indexOf(remaining.slice(0, mid)) !== -1) {
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    if (hi >= 3) {
      matchLength = hi;
      matchIndex = normalizedText.indexOf(remaining.slice(0, matchLength));
    }
  }

  // Strategy 2b: Positionally-constrained match for very short remaining (1-2 chars).
  // Only accept if the element text STARTS with the remaining text.
  if (matchIndex === -1 && remaining.length < 3 && remaining.length >= 1) {
    if (normalizedText.startsWith(remaining)) {
      matchIndex = 0;
      matchLength = remaining.length;
    }
  }

  // Strategy 3: Overlap detection — the remaining may start with a short
  // fragment from the previous element. Check if the element's text starts
  // with a suffix of remaining (e.g. remaining="s Observability..." and
  // element text starts with "Observability...").
  // Thresholds are proportional to content length to avoid edge cases with
  // very short or very long text. When mermaidCrossed, remaining is polluted
  // with SVG text from sel.toString() that doesn't appear in HAST, so we
  // lower thresholds to accept shorter matches at any position.
  const minOverlapElement = mermaidCrossed ? 3 : Math.max(3, Math.min(10, Math.floor(remaining.length / 3)));
  const probeWindow = Math.max(5, Math.floor(remaining.length / 2));
  if (matchIndex === -1 && normalizedText.length >= minOverlapElement) {
    const probe = normalizedText.slice(0, Math.min(probeWindow, normalizedText.length));
    const overlapIdx = remaining.indexOf(probe);
    // Accept the overlap if it's within the probe window, OR if the probe
    // is long enough (≥8 chars) to be specific regardless of position.
    // After crossing a mermaid block, accept any position with ≥3 char probe
    // since the SVG text may push the real match arbitrarily far into remaining.
    if (overlapIdx > 0 && (mermaidCrossed ? probe.length >= 3 : (probe.length >= 8 || overlapIdx < probeWindow))) {
      matchIndex = 0;
      matchLength = Math.min(normalizedText.length, remaining.length - overlapIdx);
      skippedChars = overlapIdx;
    }
  }

  if (matchIndex === -1) return 0;

  const normToOrig = buildNormToOrigMap(text, normalizedText);
  const origMatchStart = normToOrig[matchIndex];
  const origMatchEnd = normToOrig[matchIndex + matchLength];

  insertMarks(nodes, origMatchStart, origMatchEnd, highlight);

  return skippedChars + matchLength;
}

// ── Media wrapping ──────────────────────────────────────────────────

// E-PENPAL-HIGHLIGHT-MEDIA: helpers for wrapping images and mermaid diagrams in highlights.

/** Returns true if the element is a block containing only <img> elements and whitespace text. */
function isMediaOnlyBlock(node: Element): boolean {
  if (!node.children || node.children.length === 0) return false;
  let hasImg = false;
  for (const child of node.children) {
    if (child.type === 'element' && (child as Element).tagName === 'img') {
      hasImg = true;
    } else if (child.type === 'text' && !(child as Text).value.trim()) {
      // whitespace text node — ok
    } else {
      return false;
    }
  }
  return hasImg;
}

/** Wrap a node in a <mark> element by replacing it in the parent's children array. */
function wrapNodeInMark(
  node: Element, index: number, parent: Element | Root, highlight: ThreadHighlight
): void {
  const mark: Element = {
    type: 'element',
    tagName: 'mark',
    properties: {
      className: highlight.pending
        ? ['comment-highlight', 'pending-highlight']
        : ['comment-highlight'],
      dataThreadId: highlight.threadId,
    },
    children: [node],
  };
  (parent as Element).children[index] = mark;
}

/** Find the nearest <mark> element searching from `fromIndex` in `direction`, skipping whitespace text. */
function findNearestMark(children: ElementContent[], fromIndex: number, direction: -1 | 1): Element | null {
  for (let i = fromIndex + direction; i >= 0 && i < children.length; i += direction) {
    const c = children[i];
    if (c.type === 'element' && (c as Element).tagName === 'mark') return c as Element;
    if (c.type === 'text' && !(c as Text).value.trim()) continue;
    break;
  }
  return null;
}

/**
 * After mark insertion, scan an element's children for <img> elements
 * sandwiched between <mark> elements with the same threadId and wrap them.
 */
function wrapInlineMedia(element: Element): void {
  const children = element.children;
  for (let i = children.length - 1; i >= 0; i--) {
    const child = children[i];
    if (child.type !== 'element') continue;
    const el = child as Element;
    if (el.tagName !== 'img') continue;

    const markBefore = findNearestMark(children, i, -1);
    const markAfter = findNearestMark(children, i, 1);

    if (markBefore && markAfter &&
        markBefore.properties?.dataThreadId === markAfter.properties?.dataThreadId) {
      const threadId = markBefore.properties?.dataThreadId as string;
      const pending = (markBefore.properties?.className as string[])?.includes('pending-highlight');
      const mark: Element = {
        type: 'element',
        tagName: 'mark',
        properties: {
          className: pending ? ['comment-highlight', 'pending-highlight'] : ['comment-highlight'],
          dataThreadId: threadId,
        },
        children: [el],
      };
      children[i] = mark;
    }
  }
}

// ── Code block bridging ─────────────────────────────────────────────

// E-PENPAL-HIGHLIGHT-CROSS: Handle syntax-highlighted code blocks for cross-boundary highlights.
// Can't insert <mark> elements (SyntaxHighlighter re-parses children as a string),
// so stores match info as dataCrossHighlights on the <code> element for
// MarkdownViewer's code component to read at render time.
// Returns true if the <pre> was handled (caller should SKIP), false to fall through.
function handleCodeBlock(
  preNode: Element,
  continuing: Map<string, { highlight: ThreadHighlight; remaining: string }>,
  byLine: Map<number, ThreadHighlight[]>,
  applied: Set<string>,
): boolean {
  const codeChild = preNode.children.find(
    (c): c is Element => c.type === 'element' && c.tagName === 'code'
  );
  if (!codeChild) return false;

  // Mermaid <pre> blocks are handled by the caller (annotated as media)
  // before handleCodeBlock is reached, so only syntax-highlighted and
  // language-less code blocks arrive here.
  const classes = Array.isArray(codeChild.properties?.className)
    ? (codeChild.properties.className as string[]) : [];
  const hasLanguage = classes.some(c => /^language-/.test(String(c)));

  // Language-less code: fall through to normal mark insertion
  // (SyntaxHighlighter is not used, so <mark> elements render fine)
  if (!hasLanguage) return false;

  const codeText = collectTextNodes(codeChild).text.replace(/\n$/, '');
  if (!codeText) return true;

  const preSourceLine = preNode.position?.start?.line;
  if (!preSourceLine) return true;

  const normalizedCode = normalizeHast(codeText);
  const normToOrig = buildNormToOrigMap(codeText, normalizedCode);
  const codeLineCount = codeText.split('\n').length;
  const crossHighlights: { threadId: string; selectedText: string; pending?: boolean }[] = [];

  // 1. Continue existing cross-element highlights into this code block
  for (const [threadId, state] of continuing) {
    if (preSourceLine <= state.highlight.startLine) continue;

    const match = matchTextInCode(normalizedCode, state.remaining);
    if (!match) continue;

    const origStart = normToOrig[match.matchIndex];
    const origEnd = normToOrig[match.matchIndex + match.matchLength];
    crossHighlights.push({
      threadId,
      selectedText: codeText.slice(origStart, origEnd),
      pending: state.highlight.pending,
    });

    const consumed = match.skippedChars + match.matchLength;
    const newRemaining = state.remaining.slice(consumed).trim();
    if (newRemaining.length === 0) {
      continuing.delete(threadId);
    } else {
      state.remaining = newRemaining;
    }
  }

  // 2. Start new highlights whose startLine falls within this code block
  for (let lineOffset = 0; lineOffset <= codeLineCount; lineOffset++) {
    const lineHighlights = byLine.get(preSourceLine + lineOffset);
    if (!lineHighlights) continue;

    for (const highlight of lineHighlights) {
      if (applied.has(highlight.threadId)) continue;

      const normSelected = normalizeSelected(highlight.selectedText);
      const match = matchTextInCode(normalizedCode, normSelected, highlight.occurrenceIndex);
      if (!match) continue;

      applied.add(highlight.threadId);

      // Full match (code-only highlight) — skip cross-highlight storage.
      // MarkdownViewer's existing startLine filter handles these; storing
      // here too would cause double-matching and text duplication.
      if (match.matchLength >= normSelected.length) continue;

      // Partial match — store cross-highlight for the code portion
      const origStart = normToOrig[match.matchIndex];
      const origEnd = normToOrig[match.matchIndex + match.matchLength];
      crossHighlights.push({
        threadId: highlight.threadId,
        selectedText: codeText.slice(origStart, origEnd),
        pending: highlight.pending,
      });

      // Set up continuation for remaining text past the code block
      if (match.matchLength < normSelected.length) {
        const remaining = normSelected.slice(match.matchLength).trim();
        if (remaining.length >= 1) {
          continuing.set(highlight.threadId, { highlight, remaining });
        }
      }
    }
  }

  // Store cross-highlights on the <code> element for MarkdownViewer to read
  if (crossHighlights.length > 0) {
    codeChild.properties = codeChild.properties || {};
    codeChild.properties.dataCrossHighlights = JSON.stringify(crossHighlights);
  }

  return true;
}

/** Match search text against normalized code text, returning match position or null. */
function matchTextInCode(
  normalizedCode: string,
  searchText: string,
  occurrenceIndex?: number,
): { matchIndex: number; matchLength: number; skippedChars: number } | null {
  // Try full match with occurrence index
  const matchIndex = nthIndexOf(normalizedCode, searchText, occurrenceIndex ?? 0);
  if (matchIndex !== -1) {
    return { matchIndex, matchLength: searchText.length, skippedChars: 0 };
  }

  // Binary search for longest prefix
  if (searchText.length > 3) {
    let lo = 3, hi = Math.min(searchText.length - 1, normalizedCode.length);
    while (lo <= hi) {
      const mid = (lo + hi) >>> 1;
      if (normalizedCode.indexOf(searchText.slice(0, mid)) !== -1) {
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    if (hi >= 3) {
      const prefixLen = hi;
      const prefixIdx = normalizedCode.indexOf(searchText.slice(0, prefixLen));
      if (prefixIdx !== -1) {
        return { matchIndex: prefixIdx, matchLength: prefixLen, skippedChars: 0 };
      }
    }
  }

  // Short text positional match
  if (searchText.length < 3 && searchText.length >= 1) {
    if (normalizedCode.startsWith(searchText)) {
      return { matchIndex: 0, matchLength: searchText.length, skippedChars: 0 };
    }
  }

  return null;
}
