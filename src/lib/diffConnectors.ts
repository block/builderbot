/**
 * Spine Connectors
 *
 * Draws bezier curve connectors between corresponding changed regions
 * in the center spine. These visualize how regions in the "before"
 * pane map to the "after" pane.
 *
 * The connectors only draw the top and bottom curves - no vertical lines
 * on the edges, since those would duplicate the range borders in the text panes.
 *
 * Comment indicators are drawn as vertical highlight lines on the spine,
 * spanning the full height of the commented region.
 */

import type { Alignment, Span, Comment } from './types';

/** Info about a comment highlight for click handling */
export interface CommentHighlightInfo {
  /** The comment ID */
  commentId: string;
  /** The span this comment covers */
  span: Span;
}

export interface ConnectorConfig {
  lineHeight: number;
  /** Vertical offset to adjust bezier alignment (positive = down, negative = up) */
  verticalOffset: number;
  /** Index of the hovered alignment (in the changed alignments list), or null */
  hoveredIndex: number | null;
  /** Comments for the current file - used to draw highlight bars on the spine */
  comments?: Comment[];
  /** Callback when a comment highlight is clicked */
  onCommentClick?: (info: CommentHighlightInfo) => void;
}

const DEFAULT_CONFIG: ConnectorConfig = {
  lineHeight: 20,
  verticalOffset: 0,
  hoveredIndex: null,
  comments: [],
};

/**
 * Get CSS custom property value from the document.
 */
function getCssVar(name: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback;
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/**
 * Draw connectors between changed alignments.
 */
export function drawConnectors(
  svg: SVGSVGElement,
  alignments: Alignment[],
  beforeScroll: number,
  afterScroll: number,
  config: Partial<ConnectorConfig> = {}
): void {
  const cfg = { ...DEFAULT_CONFIG, ...config };
  const svgWidth = svg.clientWidth;
  const svgHeight = svg.clientHeight;
  const cpOffset = svgWidth * 0.5; // bezier control point offset

  // Get colors from CSS variables (matches the text pane styling)
  const fillColor = getCssVar('--diff-changed-bg', 'rgba(128, 128, 128, 0.04)');
  const hoverFillColor = getCssVar('--bg-hover', 'rgba(128, 128, 128, 0.08)');
  const strokeColor = getCssVar('--diff-range-border', 'rgba(128, 128, 128, 0.2)');

  // Clear existing and set crisp rendering
  svg.innerHTML = '';
  svg.setAttribute('shape-rendering', 'geometricPrecision');

  // Clip to the code area (below the header) using verticalOffset
  // verticalOffset is the distance from SVG top to where code content starts
  const clipTop = Math.max(0, cfg.verticalOffset);
  const defs = document.createElementNS('http://www.w3.org/2000/svg', 'defs');
  const clipPath = document.createElementNS('http://www.w3.org/2000/svg', 'clipPath');
  clipPath.setAttribute('id', 'code-area-clip');
  const clipRect = document.createElementNS('http://www.w3.org/2000/svg', 'rect');
  clipRect.setAttribute('x', '0');
  clipRect.setAttribute('y', String(clipTop));
  clipRect.setAttribute('width', String(svgWidth));
  clipRect.setAttribute('height', String(svgHeight - clipTop));
  clipPath.appendChild(clipRect);
  defs.appendChild(clipPath);
  svg.appendChild(defs);

  // Create a group with clipping applied
  const clippedGroup = document.createElementNS('http://www.w3.org/2000/svg', 'g');
  clippedGroup.setAttribute('clip-path', 'url(#code-area-clip)');
  svg.appendChild(clippedGroup);

  // Track index among changed alignments (to match hoveredIndex)
  let changedIndex = 0;

  for (const alignment of alignments) {
    if (!alignment.changed) continue;

    const isHovered = cfg.hoveredIndex === changedIndex;
    changedIndex++;

    // Calculate pixel positions relative to viewport
    // Top border: at the top edge of the first line (start * lineHeight)
    // Bottom border: at the bottom edge of the last line (end * lineHeight)
    // The CSS pseudo-elements for range-start/range-end are at top:0 and bottom:0 of their lines
    // Add 0.5px offset for crisp 1px stroke rendering on pixel boundaries
    // Bottom uses -0.5 to align with the CSS ::after pseudo-element at bottom:0
    // verticalOffset adjusts for structural differences between SVG and code container
    const beforeTop =
      alignment.before.start * cfg.lineHeight - beforeScroll + 0.5 + cfg.verticalOffset;
    const beforeBottom =
      alignment.before.end * cfg.lineHeight - beforeScroll - 0.5 + cfg.verticalOffset;
    const afterTop =
      alignment.after.start * cfg.lineHeight - afterScroll + 0.5 + cfg.verticalOffset;
    const afterBottom =
      alignment.after.end * cfg.lineHeight - afterScroll - 0.5 + cfg.verticalOffset;

    // Skip if completely out of view
    if (beforeBottom < 0 && afterBottom < 0) continue;
    if (beforeTop > svgHeight && afterTop > svgHeight) continue;

    const isInsertion = alignment.before.start === alignment.before.end;
    const isDeletion = alignment.after.start === alignment.after.end;

    // Draw filled shape (no stroke - we'll draw strokes separately for top/bottom only)
    let fillPath: string;
    if (isInsertion) {
      // Point on left, range on right
      fillPath =
        `M 0 ${beforeTop} ` +
        `C ${cpOffset} ${beforeTop}, ${svgWidth - cpOffset} ${afterTop}, ${svgWidth} ${afterTop} ` +
        `L ${svgWidth} ${afterBottom} ` +
        `C ${svgWidth - cpOffset} ${afterBottom}, ${cpOffset} ${beforeTop}, 0 ${beforeTop} Z`;
    } else if (isDeletion) {
      // Range on left, point on right
      fillPath =
        `M 0 ${beforeTop} ` +
        `C ${cpOffset} ${beforeTop}, ${svgWidth - cpOffset} ${afterTop}, ${svgWidth} ${afterTop} ` +
        `C ${svgWidth - cpOffset} ${afterTop}, ${cpOffset} ${beforeBottom}, 0 ${beforeBottom} Z`;
    } else {
      // Curved trapezoid
      fillPath =
        `M 0 ${beforeTop} ` +
        `C ${cpOffset} ${beforeTop}, ${svgWidth - cpOffset} ${afterTop}, ${svgWidth} ${afterTop} ` +
        `L ${svgWidth} ${afterBottom} ` +
        `C ${svgWidth - cpOffset} ${afterBottom}, ${cpOffset} ${beforeBottom}, 0 ${beforeBottom} Z`;
    }

    const fill = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    fill.setAttribute('d', fillPath);
    fill.setAttribute('fill', isHovered ? hoverFillColor : fillColor);
    fill.setAttribute('stroke', 'none');
    clippedGroup.appendChild(fill);

    // Draw top curve stroke only (no vertical edges)
    const topCurve =
      `M 0 ${beforeTop} ` +
      `C ${cpOffset} ${beforeTop}, ${svgWidth - cpOffset} ${afterTop}, ${svgWidth} ${afterTop}`;

    const topStroke = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    topStroke.setAttribute('d', topCurve);
    topStroke.setAttribute('fill', 'none');
    topStroke.setAttribute('stroke', strokeColor);
    topStroke.setAttribute('stroke-width', '1');
    topStroke.setAttribute('vector-effect', 'non-scaling-stroke');
    clippedGroup.appendChild(topStroke);

    // Draw bottom curve stroke only (no vertical edges)
    // For insertions/deletions where one side is a point, bottom curve connects differently
    let bottomCurve: string;
    if (isInsertion) {
      // Bottom connects afterBottom back to beforeTop (the point)
      bottomCurve =
        `M ${svgWidth} ${afterBottom} ` +
        `C ${svgWidth - cpOffset} ${afterBottom}, ${cpOffset} ${beforeTop}, 0 ${beforeTop}`;
    } else if (isDeletion) {
      // Bottom connects afterTop (the point) back to beforeBottom
      bottomCurve =
        `M ${svgWidth} ${afterTop} ` +
        `C ${svgWidth - cpOffset} ${afterTop}, ${cpOffset} ${beforeBottom}, 0 ${beforeBottom}`;
    } else {
      // Normal case: bottom connects afterBottom to beforeBottom
      bottomCurve =
        `M ${svgWidth} ${afterBottom} ` +
        `C ${svgWidth - cpOffset} ${afterBottom}, ${cpOffset} ${beforeBottom}, 0 ${beforeBottom}`;
    }

    const bottomStroke = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    bottomStroke.setAttribute('d', bottomCurve);
    bottomStroke.setAttribute('fill', 'none');
    bottomStroke.setAttribute('stroke', strokeColor);
    bottomStroke.setAttribute('stroke-width', '1');
    bottomStroke.setAttribute('vector-effect', 'non-scaling-stroke');
    clippedGroup.appendChild(bottomStroke);
  }

  // Draw comment highlight lines on the spine
  // These are vertical bars indicating commented regions
  drawCommentHighlights(clippedGroup, afterScroll, cfg, svgWidth, svgHeight, clipTop);
}

/**
 * Draw comment highlight lines on the spine.
 * Shows a vertical highlight bar for each commented region, aligned to the right
 * edge to visually connect with the after pane. Highlights are clickable.
 *
 * Overlapping comments are stacked in a pyramid style - widest spans on the
 * outside (right edge), narrower spans nested inside (further left).
 */
function drawCommentHighlights(
  group: SVGGElement,
  afterScroll: number,
  cfg: ConnectorConfig,
  svgWidth: number,
  svgHeight: number,
  clipTop: number
): void {
  const comments = cfg.comments || [];
  if (comments.length === 0) return;

  const commentColor = getCssVar('--diff-comment-highlight', 'rgba(88, 166, 255, 0.5)');
  const hoverColor = getCssVar('--diff-comment-highlight', 'rgba(88, 166, 255, 0.8)');
  const highlightWidth = 4;
  const highlightGap = 2; // Gap between stacked highlights
  const hitAreaPadding = 6; // Extra padding around each highlight for easier clicking

  // Filter out global comments (0,0 span) and sort by span size (largest first)
  // This ensures wider spans are drawn first (on the right edge)
  const validComments = comments
    .filter((c) => c.span.start !== 0 || c.span.end !== 0)
    .sort((a, b) => {
      const sizeA = a.span.end - a.span.start;
      const sizeB = b.span.end - b.span.start;
      // Largest first, then by start position for stability
      if (sizeB !== sizeA) return sizeB - sizeA;
      return a.span.start - b.span.start;
    });

  // For each comment, calculate how many wider comments overlap with it
  // This determines its x-offset (pyramid stacking)
  const commentOffsets = new Map<string, number>();

  for (let i = 0; i < validComments.length; i++) {
    const comment = validComments[i];
    let offset = 0;

    // Count how many larger comments (earlier in sorted list) overlap with this one
    for (let j = 0; j < i; j++) {
      const other = validComments[j];
      // Check if spans overlap
      if (comment.span.start < other.span.end && comment.span.end > other.span.start) {
        offset++;
      }
    }

    commentOffsets.set(comment.id, offset);
  }

  // Draw highlights for each comment
  for (const comment of validComments) {
    const { span } = comment;
    const offset = commentOffsets.get(comment.id) || 0;

    // Calculate pixel positions
    const top = span.start * cfg.lineHeight - afterScroll + cfg.verticalOffset;
    const bottom =
      Math.max(span.end, span.start + 1) * cfg.lineHeight - afterScroll + cfg.verticalOffset;

    // Skip if completely out of view
    if (bottom < clipTop || top > svgHeight) continue;

    // X position: start from right edge, offset left for nested comments
    const xPos = svgWidth - highlightWidth - offset * (highlightWidth + highlightGap);

    // Draw the visible highlight bar
    const rect = document.createElementNS('http://www.w3.org/2000/svg', 'rect');
    rect.setAttribute('x', String(xPos));
    rect.setAttribute('y', String(top));
    rect.setAttribute('width', String(highlightWidth));
    rect.setAttribute('height', String(bottom - top));
    rect.setAttribute('fill', commentColor);
    rect.setAttribute('rx', '1'); // Slight rounding
    group.appendChild(rect);

    // Add hit area for clicking (slightly larger than visible rect)
    if (cfg.onCommentClick) {
      const hitArea = document.createElementNS('http://www.w3.org/2000/svg', 'rect');
      hitArea.setAttribute('x', String(xPos - hitAreaPadding));
      hitArea.setAttribute('y', String(top));
      hitArea.setAttribute('width', String(highlightWidth + hitAreaPadding * 2));
      hitArea.setAttribute('height', String(bottom - top));
      hitArea.setAttribute('fill', 'transparent');
      hitArea.setAttribute('cursor', 'pointer');

      // Hover effect
      hitArea.addEventListener('mouseenter', () => {
        rect.setAttribute('fill', hoverColor);
        rect.setAttribute('width', String(highlightWidth + 1)); // Slightly wider on hover
      });
      hitArea.addEventListener('mouseleave', () => {
        rect.setAttribute('fill', commentColor);
        rect.setAttribute('width', String(highlightWidth));
      });

      // Click handler
      hitArea.addEventListener('click', (e) => {
        e.stopPropagation();
        cfg.onCommentClick!({ commentId: comment.id, span: comment.span });
      });

      group.appendChild(hitArea);
    }
  }
}
