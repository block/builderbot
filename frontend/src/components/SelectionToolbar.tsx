import { useEffect, useRef, useState, useCallback } from 'react';
import type { Anchor } from '../types';

interface SelectionToolbarProps {
  contentRef: React.RefObject<HTMLDivElement | null>;
  rawMarkdown: string;
  onComment: (anchor: Anchor, selectedText: string) => void;
}

/**
 * Walks up from a DOM node to find the nearest ancestor with data-source-line.
 */
function findSourceLine(node: Node, contentEl: HTMLElement): number {
  let el: HTMLElement | null = node.nodeType === 3 ? (node as Text).parentElement : (node as HTMLElement);
  while (el && el !== contentEl) {
    if (el.hasAttribute?.('data-source-line')) {
      return parseInt(el.getAttribute('data-source-line')!, 10);
    }
    el = el.parentElement;
  }
  return -1;
}

/**
 * Computes an anchor from the current text selection, matching the Go template logic.
 */
function computeAnchor(
  sel: Selection,
  selectedText: string,
  rawMarkdown: string,
  contentEl: HTMLElement,
): Anchor {
  const anchor: Anchor = {
    selectedText,
    before: '',
    after: '',
    headingPath: '',
    startLine: 0,
  };

  const startLine = findSourceLine(sel.anchorNode!, contentEl);
  const endLine = findSourceLine(sel.focusNode!, contentEl);

  if (startLine > 0) anchor.startLine = startLine;

  // Compute occurrence index within the block
  let occurrenceIndex = 0;
  let blockEl: HTMLElement | null =
    sel.anchorNode!.nodeType === 3
      ? (sel.anchorNode as Text).parentElement
      : (sel.anchorNode as HTMLElement);
  while (blockEl && blockEl !== contentEl && !blockEl.hasAttribute?.('data-source-line')) {
    blockEl = blockEl.parentElement;
  }
  if (blockEl && blockEl !== contentEl) {
    try {
      const range = sel.getRangeAt(0);
      const preRange = document.createRange();
      preRange.setStart(blockEl, 0);
      preRange.setEnd(range.startContainer, range.startOffset);
      const textBefore = preRange.toString();
      let searchPos = 0;
      while (true) {
        const found = textBefore.indexOf(selectedText, searchPos);
        if (found === -1) break;
        occurrenceIndex++;
        searchPos = found + 1;
      }
    } catch {
      // ignore range errors
    }
  }

  // Find nth occurrence helper
  function findNthOccurrence(haystack: string, needle: string, n: number): number {
    let pos = 0;
    for (let i = 0; i <= n; i++) {
      const found = haystack.indexOf(needle, pos);
      if (found === -1) return -1;
      if (i === n) return found;
      pos = found + 1;
    }
    return -1;
  }

  function setBeforeAfter(globalOffset: number) {
    const beforeStart = Math.max(0, globalOffset - 80);
    anchor.before = rawMarkdown.substring(beforeStart, globalOffset);
    const afterEnd = Math.min(rawMarkdown.length, globalOffset + selectedText.length + 80);
    anchor.after = rawMarkdown.substring(globalOffset + selectedText.length, afterEnd);
  }

  if (startLine > 0) {
    const lines = rawMarkdown.split('\n');
    const searchStart = Math.max(0, startLine - 1);
    const searchEnd = Math.min(lines.length, (endLine > 0 ? endLine : startLine) + 5);
    const regionText = lines.slice(searchStart, searchEnd).join('\n');
    let idx = findNthOccurrence(regionText, selectedText, occurrenceIndex);
    if (idx === -1) idx = regionText.indexOf(selectedText);
    if (idx !== -1) {
      const globalLines = lines.slice(0, searchStart).join('\n');
      const globalOffset = globalLines.length > 0 ? globalLines.length + 1 + idx : idx;
      setBeforeAfter(globalOffset);
    }
  } else {
    const idx = rawMarkdown.indexOf(selectedText);
    if (idx !== -1) setBeforeAfter(idx);
  }

  // Build heading path
  if (contentEl) {
    let headings: string[] = [];
    let el: HTMLElement | null = blockEl || (sel.anchorNode!.nodeType === 3 ? (sel.anchorNode as Text).parentElement : (sel.anchorNode as HTMLElement));
    // Walk backwards through siblings to find preceding headings
    while (el && el !== contentEl) {
      el = el.previousElementSibling as HTMLElement | null;
      if (!el) break;
      if (/^H[1-6]$/.test(el.tagName)) {
        headings.unshift(el.textContent || '');
        break;
      }
    }
    if (headings.length > 0) anchor.headingPath = headings.join(' > ');
  }

  return anchor;
}

/**
 * Applies pending highlight marks to the current selection.
 */
export function applyPendingHighlight(sel: Selection, contentEl: HTMLElement) {
  removePendingHighlight();
  if (!sel.rangeCount) return;
  const range = sel.getRangeAt(0);
  if (!contentEl.contains(range.commonAncestorContainer)) return;

  const treeWalker = document.createTreeWalker(contentEl, NodeFilter.SHOW_TEXT);
  const textNodes: Text[] = [];
  while (treeWalker.nextNode()) {
    if (range.intersectsNode(treeWalker.currentNode)) {
      textNodes.push(treeWalker.currentNode as Text);
    }
  }

  for (let i = textNodes.length - 1; i >= 0; i--) {
    const node = textNodes[i];
    const start = node === range.startContainer ? range.startOffset : 0;
    const end = node === range.endContainer ? range.endOffset : node.nodeValue!.length;
    if (start >= end) continue;

    const mark = document.createElement('mark');
    mark.className = 'pending-highlight';

    if (start === 0 && end === node.nodeValue!.length) {
      node.parentNode!.insertBefore(mark, node);
      mark.appendChild(node);
    } else {
      const subRange = document.createRange();
      subRange.setStart(node, start);
      subRange.setEnd(node, end);
      subRange.surroundContents(mark);
    }
  }
  sel.removeAllRanges();
}

export function removePendingHighlight() {
  document.querySelectorAll('.pending-highlight').forEach((mark) => {
    const parent = mark.parentNode!;
    while (mark.firstChild) parent.insertBefore(mark.firstChild, mark);
    parent.removeChild(mark);
    parent.normalize();
  });
}

/**
 * Finds which mermaid container index corresponds to a given startLine
 * by counting ```mermaid fences in the raw markdown.
 */
function findMermaidContainerIndex(rawMarkdown: string, startLine: number): number {
  if (startLine < 1) return -1;
  const lines = rawMarkdown.split('\n');
  let mermaidIdx = 0;
  let inFence = false;
  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim();
    if (trimmed.startsWith('```')) {
      if (!inFence) {
        if (/^```mermaid\b/.test(trimmed)) {
          if (i + 1 === startLine) return mermaidIdx;
          mermaidIdx++;
        }
        inFence = true;
      } else {
        inFence = false;
      }
    }
  }
  return -1;
}

/**
 * Adds an SVG highlight rect to a mermaid diagram for a thread with svgRect anchor.
 */
function showSvgHighlight(threadId: string, anchor: Anchor, contentEl: HTMLElement, rawMarkdown: string) {
  if (!anchor.svgRect) return;
  const r = anchor.svgRect;
  const startLine = anchor.startLine || 0;
  // Try data-source-line lookup first (set at render time, like Go template)
  let container = startLine > 0
    ? contentEl.querySelector(`.mermaid-container[data-source-line="${startLine}"]`)
    : null;
  // Fallback: find by index in raw markdown
  if (!container) {
    const containerIndex = findMermaidContainerIndex(rawMarkdown, startLine);
    const containers = contentEl.querySelectorAll('.mermaid-container');
    container = containerIndex >= 0 && containerIndex < containers.length ? containers[containerIndex] : null;
  }
  if (!container) return;
  const svg = container.querySelector('svg');
  if (!svg) return;
  const ns = 'http://www.w3.org/2000/svg';
  const rect = document.createElementNS(ns, 'rect');
  rect.setAttribute('x', String(r.x));
  rect.setAttribute('y', String(r.y));
  rect.setAttribute('width', String(r.width));
  rect.setAttribute('height', String(r.height));
  rect.setAttribute('class', 'penpal-svg-highlight');
  rect.setAttribute('data-thread-id', threadId);
  svg.appendChild(rect);
}

/**
 * Adds comment highlight marks to the content for existing threads.
 */
export function addCommentHighlights(
  threads: { id: string; status: string; anchor: Anchor }[],
  anchorLines: Record<string, number>,
  contentEl: HTMLElement,
  rawMarkdown?: string,
) {
  // Remove existing text highlights
  contentEl.querySelectorAll('.comment-highlight').forEach((mark) => {
    const parent = mark.parentNode!;
    while (mark.firstChild) parent.insertBefore(mark.firstChild, mark);
    parent.removeChild(mark);
    parent.normalize();
  });

  // Remove existing SVG highlights
  contentEl.querySelectorAll('.penpal-svg-highlight').forEach((el) => el.remove());

  threads.forEach((thread) => {
    if (thread.status === 'resolved') return;

    // SVG rect threads get overlay highlights instead of text highlighting
    if (thread.anchor.svgRect) {
      showSvgHighlight(thread.id, thread.anchor, contentEl, rawMarkdown || '');
      return;
    }

    const line = anchorLines[thread.id];
    if (line === -1 || line === undefined) return;
    const selectedText = thread.anchor.selectedText;
    if (!selectedText) return;

    // Find the block element with the matching source line
    const blockEl = contentEl.querySelector(`[data-source-line="${line}"]`);
    const searchRoot = blockEl || contentEl;

    // Walk text nodes to find and highlight the selected text
    const treeWalker = document.createTreeWalker(searchRoot, NodeFilter.SHOW_TEXT);
    let accumulated = '';
    const textNodes: { node: Text; start: number }[] = [];

    while (treeWalker.nextNode()) {
      const node = treeWalker.currentNode as Text;
      textNodes.push({ node, start: accumulated.length });
      accumulated += node.nodeValue || '';
    }

    // Normalize whitespace for matching
    const normalizedAccumulated = accumulated.replace(/\s+/g, ' ');
    const normalizedSelected = selectedText.replace(/\s+/g, ' ');
    const matchIndex = normalizedAccumulated.indexOf(normalizedSelected);
    if (matchIndex === -1) return;

    const matchStart = matchIndex;
    const matchEnd = matchIndex + normalizedSelected.length;

    // Apply highlights
    for (let i = textNodes.length - 1; i >= 0; i--) {
      const { node, start: nodeStart } = textNodes[i];
      const nodeText = node.nodeValue || '';

      // Check overlap
      const overlapStart = Math.max(matchStart - nodeStart, 0);
      const overlapEnd = Math.min(matchEnd - nodeStart, nodeText.length);
      if (overlapStart >= overlapEnd) continue;

      const mark = document.createElement('mark');
      mark.className = 'comment-highlight';
      mark.dataset.threadId = thread.id;

      if (overlapStart === 0 && overlapEnd === nodeText.length) {
        node.parentNode!.insertBefore(mark, node);
        mark.appendChild(node);
      } else {
        try {
          const range = document.createRange();
          range.setStart(node, overlapStart);
          range.setEnd(node, overlapEnd);
          range.surroundContents(mark);
        } catch {
          // surroundContents can fail across element boundaries
        }
      }
    }
  });
}

/**
 * Floating toolbar that appears on text selection within the content area.
 */
export default function SelectionToolbar({
  contentRef,
  rawMarkdown,
  onComment,
}: SelectionToolbarProps) {
  const [visible, setVisible] = useState(false);
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const toolbarRef = useRef<HTMLDivElement>(null);

  const handleMouseUp = useCallback(() => {
    setTimeout(() => {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed || !sel.toString().trim()) {
        setVisible(false);
        return;
      }
      const contentEl = contentRef.current;
      if (!contentEl || !contentEl.contains(sel.anchorNode) || !contentEl.contains(sel.focusNode)) {
        setVisible(false);
        return;
      }

      const range = sel.getRangeAt(0);
      const rect = range.getBoundingClientRect();
      const contentRect = contentEl.getBoundingClientRect();

      setPosition({
        top: rect.bottom - contentRect.top + 4,
        left: rect.right - contentRect.left,
      });
      setVisible(true);
    }, 10);
  }, [contentRef]);

  // Attach listener to content area
  useEffect(() => {
    const contentEl = contentRef.current;
    if (!contentEl) return;
    contentEl.addEventListener('mouseup', handleMouseUp);
    const handleClick = () => setVisible(false);
    document.addEventListener('click', handleClick);
    return () => {
      contentEl.removeEventListener('mouseup', handleMouseUp);
      document.removeEventListener('click', handleClick);
    };
  }, [contentRef, handleMouseUp]);

  // Clamp position after render
  useEffect(() => {
    if (!visible || !toolbarRef.current || !contentRef.current) return;
    const toolbar = toolbarRef.current;
    const contentRect = contentRef.current.getBoundingClientRect();
    const maxLeft = contentRect.width - toolbar.offsetWidth;
    if (position.left > maxLeft) {
      setPosition((p) => ({ ...p, left: Math.max(0, maxLeft) }));
    }
  }, [visible, position.left, contentRef]);

  const handleComment = (e: React.MouseEvent) => {
    e.stopPropagation();
    const sel = window.getSelection();
    if (!sel || !contentRef.current) return;
    const selectedText = sel.toString().trim();
    if (!selectedText) return;

    const anchor = computeAnchor(sel, selectedText, rawMarkdown, contentRef.current);
    applyPendingHighlight(sel, contentRef.current);
    setVisible(false);
    onComment(anchor, selectedText);
  };

  const handleCopyMarkdown = (e: React.MouseEvent) => {
    e.stopPropagation();
    const md = getSelectionMarkdown(rawMarkdown, contentRef.current!);
    if (md) navigator.clipboard.writeText(md);
    setVisible(false);
    window.getSelection()?.removeAllRanges();
  };

  if (!visible) return null;

  return (
    <div
      ref={toolbarRef}
      className="selection-toolbar"
      style={{ top: position.top, left: position.left }}
      onMouseDown={(e) => {
        e.preventDefault();
        e.stopPropagation();
      }}
    >
      <button onClick={handleComment}>Comment</button>
      <button onClick={handleCopyMarkdown}>Copy markdown</button>
    </div>
  );
}

/**
 * Extracts the raw markdown corresponding to the current text selection.
 */
function getSelectionMarkdown(rawMarkdown: string, contentEl: HTMLElement): string | null {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || !sel.toString().trim()) return null;
  if (!contentEl.contains(sel.anchorNode) || !contentEl.contains(sel.focusNode)) return null;

  const range = sel.getRangeAt(0);
  let startLine = findSourceLine(range.startContainer, contentEl);
  let endLine = findSourceLine(range.endContainer, contentEl);

  if (startLine < 1 && endLine < 1) return null;
  if (startLine < 1) startLine = endLine;
  if (endLine < 1) endLine = startLine;
  if (startLine > endLine) [startLine, endLine] = [endLine, startLine];

  const allSourceLines: number[] = [];
  contentEl.querySelectorAll('[data-source-line]').forEach((el) => {
    allSourceLines.push(parseInt(el.getAttribute('data-source-line')!, 10));
  });
  allSourceLines.sort((a, b) => a - b);

  const lines = rawMarkdown.split('\n');
  let endOfRegion = lines.length;
  for (let i = 0; i < allSourceLines.length; i++) {
    if (allSourceLines[i] > endLine) {
      endOfRegion = allSourceLines[i] - 1;
      break;
    }
  }

  const extracted = lines.slice(startLine - 1, endOfRegion);
  while (extracted.length > 0 && extracted[extracted.length - 1].trim() === '') {
    extracted.pop();
  }

  return extracted.length > 0 ? extracted.join('\n') : null;
}
