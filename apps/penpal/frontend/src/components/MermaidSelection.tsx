import { useEffect, useRef, useCallback } from 'react';
import type { Anchor, SvgRect } from '../types';

interface MermaidSelectionProps {
  contentRef: React.RefObject<HTMLDivElement | null>;
  rawMarkdown: string;
  onComment: (anchor: Anchor, selectedText: string) => void;
  /** Ref that signals when a mermaid drag-select is in progress. */
  draggingRef?: React.MutableRefObject<boolean>;
}

/**
 * Remove any pending SVG selection highlight rects from the content area.
 * Called when the comment form is cancelled or submitted.
 */
export function removePendingSvgHighlight() {
  document.querySelectorAll('.penpal-pending-svg-highlight').forEach(el => el.remove());
}

// E-PENPAL-SVG-EXTRACT: SVG snippet extraction with cropped viewBox and re-IDing.
/**
 * Extracts a viewBox-cropped SVG snippet from an SVG element.
 * Re-IDs all elements to avoid DOM collisions when the snippet is rendered
 * alongside the original diagram.
 */
function extractSvgSnippet(svgElement: SVGSVGElement, rect: SvgRect): string {
  const clone = svgElement.cloneNode(true) as SVGSVGElement;
  clone.setAttribute('viewBox', `${rect.x} ${rect.y} ${rect.width} ${rect.height}`);

  // Size to reasonable display dimensions
  const aspect = rect.width / rect.height;
  const maxW = 300, maxH = 200;
  let w: number, h: number;
  if (aspect > maxW / maxH) { w = maxW; h = maxW / aspect; }
  else { h = maxH; w = maxH * aspect; }
  clone.setAttribute('width', String(Math.round(w)));
  clone.setAttribute('height', String(Math.round(h)));

  // Remove any highlight rects from the clone
  clone.querySelectorAll('.penpal-pending-svg-highlight, .penpal-svg-highlight').forEach(el => el.remove());

  // Re-ID every element to avoid DOM collisions
  const prefix = 'ps' + Math.random().toString(36).substring(2, 8) + '-';
  const idMap: Record<string, string> = {};

  clone.querySelectorAll('[id]').forEach(el => {
    const oldId = el.getAttribute('id')!;
    const newId = prefix + oldId;
    idMap[oldId] = newId;
    el.setAttribute('id', newId);
  });

  // Also re-ID the root SVG itself
  const rootOldId = svgElement.getAttribute('id');
  if (rootOldId && !idMap[rootOldId]) {
    idMap[rootOldId] = prefix + rootOldId;
  }
  if (rootOldId) {
    clone.setAttribute('id', idMap[rootOldId]);
  }

  // Rewrite url(#oldId) references in attributes
  const urlAttrs = ['marker-end', 'marker-start', 'marker-mid', 'fill', 'stroke', 'clip-path', 'mask', 'filter'];
  clone.querySelectorAll('*').forEach(el => {
    urlAttrs.forEach(attr => {
      const val = el.getAttribute(attr);
      if (val && val.includes('url(#')) {
        el.setAttribute(attr, val.replace(/url\(#([^)]+)\)/g, (_m, id) => {
          return 'url(#' + (idMap[id] || id) + ')';
        }));
      }
    });
    // Rewrite href/xlink:href="#id" references
    ['href', 'xlink:href'].forEach(attr => {
      const val = el.getAttribute(attr);
      if (val && val.charAt(0) === '#') {
        const refId = val.substring(1);
        if (idMap[refId]) el.setAttribute(attr, '#' + idMap[refId]);
      }
    });
  });

  // Rewrite IDs in <style> blocks
  clone.querySelectorAll('style').forEach(styleEl => {
    let css = styleEl.textContent || '';
    Object.keys(idMap).forEach(oldId => {
      const escaped = oldId.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      css = css.replace(new RegExp('#' + escaped + '(?=[\\s{,.:\\[>~+)])', 'g'), '#' + idMap[oldId]);
    });
    styleEl.textContent = css;
  });

  return clone.outerHTML;
}

/**
 * Computes the heading path for an element by walking up/back through siblings.
 */
function computeHeadingPath(el: HTMLElement, contentEl: HTMLElement): string {
  const headings: string[] = [];
  let current: HTMLElement | null = el;
  while (current && current !== contentEl) {
    let sibling = current.previousElementSibling as HTMLElement | null;
    while (sibling) {
      if (/^H[1-6]$/.test(sibling.tagName)) {
        headings.unshift(sibling.textContent?.trim() || '');
        break;
      }
      sibling = sibling.previousElementSibling as HTMLElement | null;
    }
    current = current.parentElement;
  }
  return headings.join(' > ');
}

// E-PENPAL-SVG-STARTLINE: startLine computed by counting ```mermaid fences.
/**
 * Finds the source line number of the nth mermaid fence in the raw markdown.
 * Returns 1-indexed line number, or 0 if not found.
 */
export function findMermaidFenceLine(rawMarkdown: string, containerIndex: number): number {
  const lines = rawMarkdown.split('\n');
  let mermaidIdx = 0;
  let inFence = false;
  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim();
    if (trimmed.startsWith('```')) {
      if (!inFence) {
        if (/^```mermaid\b/.test(trimmed)) {
          if (mermaidIdx === containerIndex) return i + 1;
          mermaidIdx++;
        }
        inFence = true;
      } else {
        inFence = false;
      }
    }
  }
  return 0;
}

// E-PENPAL-SVG-DRAG: handles drag on .mermaid-container with 5px threshold for diagram selection.
export default function MermaidSelection({
  contentRef,
  rawMarkdown,
  onComment,
  draggingRef,
}: MermaidSelectionProps) {
  // Track cleanup functions for event listeners
  const cleanupRef = useRef<(() => void)[]>([]);
  // Track whether a drag is actively in progress (to suppress observer re-attach)
  const dragActiveRef = useRef(false);
  // Keep rawMarkdown in a ref so handlers always have the latest value
  const rawMarkdownRef = useRef(rawMarkdown);
  rawMarkdownRef.current = rawMarkdown;

  const attachHandlers = useCallback(() => {
    const contentEl = contentRef.current;
    if (!contentEl) return;

    // Clean up previous handlers
    cleanupRef.current.forEach(fn => fn());
    cleanupRef.current = [];

    const containers = contentEl.querySelectorAll('.mermaid-container');
    containers.forEach(container => {
      const containerEl = container as HTMLElement;

      const onMouseDown = (e: MouseEvent) => {
        // Don't interfere with expand button
        if ((e.target as HTMLElement).closest('.expand-zoom-btn')) return;

        const svg = containerEl.querySelector('svg');
        if (!svg) return;

        const ctm = svg.getScreenCTM();
        if (!ctm) return;
        const inv = ctm.inverse();
        const pt = new DOMPoint(e.clientX, e.clientY).matrixTransform(inv);

        const startX = e.clientX;
        const startY = e.clientY;
        const startSvgPt = { x: pt.x, y: pt.y };
        let overlayRect: SVGRectElement | null = null;
        let dragging = false;

        const onMouseMove = (e2: MouseEvent) => {
          const dx = e2.clientX - startX;
          const dy = e2.clientY - startY;
          if (!dragging && Math.sqrt(dx * dx + dy * dy) < 5) return;

          if (!dragging) {
            dragging = true;
            dragActiveRef.current = true;
            if (draggingRef) draggingRef.current = true;
            containerEl.style.userSelect = 'none';
            window.getSelection()?.removeAllRanges();
            // Remove any existing pending highlight from a previous selection
            removePendingSvgHighlight();
            const ns = 'http://www.w3.org/2000/svg';
            overlayRect = document.createElementNS(ns, 'rect') as SVGRectElement;
            overlayRect.setAttribute('class', 'penpal-pending-svg-highlight');
            svg.appendChild(overlayRect);
          }

          const ctm2 = svg.getScreenCTM();
          if (!ctm2) return;
          const inv2 = ctm2.inverse();
          const curPt = new DOMPoint(e2.clientX, e2.clientY).matrixTransform(inv2);
          const rx = Math.min(startSvgPt.x, curPt.x);
          const ry = Math.min(startSvgPt.y, curPt.y);
          const rw = Math.abs(curPt.x - startSvgPt.x);
          const rh = Math.abs(curPt.y - startSvgPt.y);
          overlayRect!.setAttribute('x', String(rx));
          overlayRect!.setAttribute('y', String(ry));
          overlayRect!.setAttribute('width', String(rw));
          overlayRect!.setAttribute('height', String(rh));
        };

        const onMouseUp = (e2: MouseEvent) => {
          document.removeEventListener('mousemove', onMouseMove);
          document.removeEventListener('mouseup', onMouseUp);
          containerEl.style.userSelect = '';
          dragActiveRef.current = false;

          if (!dragging) {
            if (draggingRef) draggingRef.current = false;
            return;
          }

          const ctm2 = svg.getScreenCTM();
          if (!ctm2) {
            if (overlayRect) overlayRect.remove();
            if (draggingRef) draggingRef.current = false;
            return;
          }
          const inv2 = ctm2.inverse();
          const endPt = new DOMPoint(e2.clientX, e2.clientY).matrixTransform(inv2);
          const selRect: SvgRect = {
            x: Math.min(startSvgPt.x, endPt.x),
            y: Math.min(startSvgPt.y, endPt.y),
            width: Math.abs(endPt.x - startSvgPt.x),
            height: Math.abs(endPt.y - startSvgPt.y),
          };

          // Ignore tiny selections
          if (selRect.width < 5 || selRect.height < 5) {
            if (overlayRect) overlayRect.remove();
            if (draggingRef) draggingRef.current = false;
            return;
          }

          let snippet: string;
          try {
            snippet = extractSvgSnippet(svg as SVGSVGElement, selRect);
          } catch (err) {
            console.error('Failed to extract SVG snippet:', err);
            if (overlayRect) overlayRect.remove();
            if (draggingRef) draggingRef.current = false;
            return;
          }

          // Compute startLine by finding which mermaid container this is (nth)
          // and matching to the nth ```mermaid fence in the raw markdown.
          // This is more reliable than data-source-line which can be misaligned.
          const allContainers = contentEl.querySelectorAll('.mermaid-container');
          const containerIndex = Array.from(allContainers).indexOf(containerEl);
          const startLineNum = findMermaidFenceLine(rawMarkdownRef.current, containerIndex);
          const headingPath = computeHeadingPath(containerEl, contentEl);

          const anchor: Anchor = {
            selectedText: '[Diagram selection]',
            svgSnippet: snippet,
            svgRect: selRect,
            headingPath,
            startLine: startLineNum,
          };

          // Keep the pending highlight rect visible on the diagram while the
          // comment form is open — it will be removed by removePendingSvgHighlight()
          // when the form is cancelled or submitted.

          onComment(anchor, '[Diagram selection]');

          // Reset dragging flag after a short delay
          setTimeout(() => {
            if (draggingRef) draggingRef.current = false;
          }, 100);
        };

        document.addEventListener('mousemove', onMouseMove);
        document.addEventListener('mouseup', onMouseUp);
      };

      containerEl.addEventListener('mousedown', onMouseDown);
      cleanupRef.current.push(() => {
        containerEl.removeEventListener('mousedown', onMouseDown);
      });
    });
  }, [contentRef, onComment, draggingRef]);

  // Re-attach handlers when mermaid finishes rendering
  useEffect(() => {
    const contentEl = contentRef.current;
    if (!contentEl) return;

    // Initial attachment
    attachHandlers();

    // Watch for mermaid rendering (childList changes when innerHTML is replaced)
    const observer = new MutationObserver((mutations) => {
      // Skip if a drag is in progress — the overlay rect changes would trigger this
      if (dragActiveRef.current) return;

      // Only re-attach on childList mutations (mermaid render replaces innerHTML),
      // not on attribute changes (which happen during drag)
      const hasChildListChange = mutations.some(m => m.type === 'childList');
      if (!hasChildListChange) return;

      const rendered = contentEl.querySelectorAll('.mermaid-container svg');
      if (rendered.length > 0) {
        attachHandlers();
      }
    });

    // Only watch childList + subtree, NOT attributes
    observer.observe(contentEl, { childList: true, subtree: true });

    return () => {
      observer.disconnect();
      cleanupRef.current.forEach(fn => fn());
      cleanupRef.current = [];
    };
  }, [contentRef, attachHandlers]);

  return null;
}
