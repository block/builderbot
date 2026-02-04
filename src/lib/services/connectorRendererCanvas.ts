/**
 * Canvas Connector Renderer
 *
 * High-performance renderer for diff spine connectors using HTML Canvas.
 * Canvas batches all draw operations internally, avoiding the DOM overhead
 * of SVG setAttribute calls.
 *
 * Key advantages over SVG:
 * 1. Single composite operation instead of N DOM updates
 * 2. GPU-accelerated rendering
 * 3. No layout/style recalculation per element
 *
 * Architecture:
 * - Uses OffscreenCanvas where available for even better perf
 * - Binary search to find visible alignment range
 * - Batched path drawing (all fills, then all strokes)
 * - Comment highlights rendered as simple rects
 */

import type { Alignment, Span, Comment } from '../types';

// ============================================================================
// Types
// ============================================================================

/** Info about a comment highlight for click handling */
export interface CommentHighlightInfo {
  commentId: string;
  span: Span;
}

/** Configuration for the renderer */
export interface ConnectorRendererConfig {
  /** Callback when a comment highlight is clicked */
  onCommentClick?: (info: CommentHighlightInfo) => void;
}

/** Cached theme colors */
interface ThemeColors {
  fillColor: string;
  hoverFillColor: string;
  strokeColor: string;
  commentColor: string;
  commentHoverColor: string;
}

/** Stored comment hit region for click detection */
interface CommentHitRegion {
  x: number;
  y: number;
  width: number;
  height: number;
  commentId: string;
  span: Span;
}

// ============================================================================
// Constants
// ============================================================================

/** Comment highlight dimensions */
const COMMENT_WIDTH = 4;
const COMMENT_GAP = 2;
const COMMENT_VERTICAL_PADDING = 2;

/** Bezier control point offset as fraction of width */
const BEZIER_CP_FRACTION = 0.5;

// ============================================================================
// Helper Functions
// ============================================================================

/**
 * Get CSS custom property value from the document.
 */
function getCssVar(name: string, fallback: string): string {
  if (typeof document === 'undefined') return fallback;
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/**
 * Read all theme colors from CSS variables.
 */
function readThemeColors(): ThemeColors {
  return {
    fillColor: getCssVar('--diff-changed-bg', 'rgba(128, 128, 128, 0.04)'),
    hoverFillColor: getCssVar('--bg-hover', 'rgba(128, 128, 128, 0.08)'),
    strokeColor: getCssVar('--diff-range-border', 'rgba(128, 128, 128, 0.2)'),
    commentColor: getCssVar('--diff-comment-highlight', 'rgba(88, 166, 255, 0.5)'),
    commentHoverColor: getCssVar('--diff-comment-highlight', 'rgba(88, 166, 255, 0.8)'),
  };
}

// ============================================================================
// ConnectorRendererCanvas Class
// ============================================================================

/**
 * High-performance Canvas renderer for diff spine connectors.
 */
export class ConnectorRendererCanvas {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private config: ConnectorRendererConfig;

  // Cached state
  private colors: ThemeColors;
  private alignments: Alignment[] = [];
  private changedAlignmentIndices: number[] = [];
  private comments: Comment[] = [];

  // Current hover state
  private hoveredAlignmentIndex: number | null = null;
  private hoveredCommentId: string | null = null;

  // Hit regions for comment click detection
  private commentHitRegions: CommentHitRegion[] = [];

  // Device pixel ratio for sharp rendering
  private dpr: number = 1;

  constructor(canvas: HTMLCanvasElement, config: ConnectorRendererConfig = {}) {
    this.canvas = canvas;
    this.config = config;

    const ctx = canvas.getContext('2d', { alpha: true });
    if (!ctx) {
      throw new Error('Could not get 2d context from canvas');
    }
    this.ctx = ctx;

    this.colors = readThemeColors();
    this.dpr = window.devicePixelRatio || 1;

    // Set up event listeners for comment interaction
    this.canvas.addEventListener('mousemove', this.handleMouseMove);
    this.canvas.addEventListener('click', this.handleClick);
    this.canvas.addEventListener('mouseleave', this.handleMouseLeave);
  }

  /**
   * Update theme colors (call when theme changes).
   */
  updateColors(): void {
    this.colors = readThemeColors();
  }

  /**
   * Set the alignments to render.
   */
  setAlignments(alignments: Alignment[]): void {
    this.alignments = alignments;

    // Pre-compute indices of changed alignments
    this.changedAlignmentIndices = [];
    for (let i = 0; i < alignments.length; i++) {
      if (alignments[i].changed) {
        this.changedAlignmentIndices.push(i);
      }
    }
  }

  /**
   * Set the comments to render.
   */
  setComments(comments: Comment[]): void {
    this.comments = comments;
  }

  /**
   * Set the currently hovered alignment index (among changed alignments).
   */
  setHoveredIndex(index: number | null): void {
    this.hoveredAlignmentIndex = index;
  }

  /**
   * Render connectors and comments for the current scroll position.
   */
  render(
    beforeScrollY: number,
    afterScrollY: number,
    lineHeight: number,
    verticalOffset: number
  ): void {
    const canvas = this.canvas;
    const ctx = this.ctx;

    // Update canvas size if needed (handle resize)
    const rect = canvas.getBoundingClientRect();
    const width = rect.width;
    const height = rect.height;

    // Scale for device pixel ratio
    const scaledWidth = Math.floor(width * this.dpr);
    const scaledHeight = Math.floor(height * this.dpr);

    if (canvas.width !== scaledWidth || canvas.height !== scaledHeight) {
      canvas.width = scaledWidth;
      canvas.height = scaledHeight;
      ctx.scale(this.dpr, this.dpr);
    }

    // Clear canvas
    ctx.clearRect(0, 0, width, height);

    // Set up clipping region (below header)
    const clipTop = Math.max(0, verticalOffset);
    ctx.save();
    ctx.beginPath();
    ctx.rect(0, clipTop, width, height - clipTop);
    ctx.clip();

    // Render connectors
    this.renderConnectors(beforeScrollY, afterScrollY, lineHeight, verticalOffset, width, height);

    // Render comments
    this.renderComments(afterScrollY, lineHeight, verticalOffset, width, height, clipTop);

    ctx.restore();
  }

  /**
   * Render connector paths for visible changed alignments.
   */
  private renderConnectors(
    beforeScrollY: number,
    afterScrollY: number,
    lineHeight: number,
    verticalOffset: number,
    width: number,
    height: number
  ): void {
    const ctx = this.ctx;
    const cpOffset = width * BEZIER_CP_FRACTION;

    // Find first potentially visible changed alignment
    const firstVisible = this.findFirstVisibleChangedAlignment(
      beforeScrollY,
      afterScrollY,
      lineHeight,
      verticalOffset
    );

    // Collect paths for batched rendering
    const normalFills: Path2D[] = [];
    const hoverFills: Path2D[] = [];
    const strokes: Path2D[] = [];

    let changedIndex = 0;

    for (const alignmentIndex of this.changedAlignmentIndices) {
      const alignment = this.alignments[alignmentIndex];

      // Skip alignments before the visible range
      if (changedIndex < firstVisible) {
        changedIndex++;
        continue;
      }

      // Calculate pixel positions
      const beforeTop = alignment.before.start * lineHeight - beforeScrollY + verticalOffset + 0.5;
      const beforeBottom = alignment.before.end * lineHeight - beforeScrollY + verticalOffset - 0.5;
      const afterTop = alignment.after.start * lineHeight - afterScrollY + verticalOffset + 0.5;
      const afterBottom = alignment.after.end * lineHeight - afterScrollY + verticalOffset - 0.5;

      // Check if completely below viewport - stop processing
      if (beforeTop > height && afterTop > height) {
        break;
      }

      // Skip if completely above viewport
      if (beforeBottom < 0 && afterBottom < 0) {
        changedIndex++;
        continue;
      }

      // Build paths
      const isHovered = this.hoveredAlignmentIndex === changedIndex;
      const fillPath = this.buildConnectorFillPath(
        beforeTop,
        beforeBottom,
        afterTop,
        afterBottom,
        width,
        cpOffset
      );
      const strokePath = this.buildConnectorStrokePath(
        beforeTop,
        beforeBottom,
        afterTop,
        afterBottom,
        width,
        cpOffset
      );

      if (isHovered) {
        hoverFills.push(fillPath);
      } else {
        normalFills.push(fillPath);
      }
      strokes.push(strokePath);

      changedIndex++;
    }

    // Batch render: all normal fills, then hover fills, then all strokes
    if (normalFills.length > 0) {
      ctx.fillStyle = this.colors.fillColor;
      for (const path of normalFills) {
        ctx.fill(path);
      }
    }

    if (hoverFills.length > 0) {
      ctx.fillStyle = this.colors.hoverFillColor;
      for (const path of hoverFills) {
        ctx.fill(path);
      }
    }

    if (strokes.length > 0) {
      ctx.strokeStyle = this.colors.strokeColor;
      ctx.lineWidth = 1;
      for (const path of strokes) {
        ctx.stroke(path);
      }
    }
  }

  /**
   * Build a Path2D for the connector fill (curved trapezoid).
   */
  private buildConnectorFillPath(
    beforeTop: number,
    beforeBottom: number,
    afterTop: number,
    afterBottom: number,
    width: number,
    cpOffset: number
  ): Path2D {
    const path = new Path2D();
    const isInsertion = Math.abs(beforeTop - beforeBottom) < 1;
    const isDeletion = Math.abs(afterTop - afterBottom) < 1;

    if (isInsertion) {
      // Point on left, range on right
      path.moveTo(0, beforeTop);
      path.bezierCurveTo(cpOffset, beforeTop, width - cpOffset, afterTop, width, afterTop);
      path.lineTo(width, afterBottom);
      path.bezierCurveTo(width - cpOffset, afterBottom, cpOffset, beforeTop, 0, beforeTop);
    } else if (isDeletion) {
      // Range on left, point on right
      path.moveTo(0, beforeTop);
      path.bezierCurveTo(cpOffset, beforeTop, width - cpOffset, afterTop, width, afterTop);
      path.bezierCurveTo(width - cpOffset, afterTop, cpOffset, beforeBottom, 0, beforeBottom);
    } else {
      // Curved trapezoid
      path.moveTo(0, beforeTop);
      path.bezierCurveTo(cpOffset, beforeTop, width - cpOffset, afterTop, width, afterTop);
      path.lineTo(width, afterBottom);
      path.bezierCurveTo(width - cpOffset, afterBottom, cpOffset, beforeBottom, 0, beforeBottom);
    }

    path.closePath();
    return path;
  }

  /**
   * Build a Path2D for the connector strokes (top and bottom curves).
   */
  private buildConnectorStrokePath(
    beforeTop: number,
    beforeBottom: number,
    afterTop: number,
    afterBottom: number,
    width: number,
    cpOffset: number
  ): Path2D {
    const path = new Path2D();
    const isInsertion = Math.abs(beforeTop - beforeBottom) < 1;
    const isDeletion = Math.abs(afterTop - afterBottom) < 1;

    // Top stroke
    path.moveTo(0, beforeTop);
    path.bezierCurveTo(cpOffset, beforeTop, width - cpOffset, afterTop, width, afterTop);

    // Bottom stroke
    if (isInsertion) {
      path.moveTo(width, afterBottom);
      path.bezierCurveTo(width - cpOffset, afterBottom, cpOffset, beforeTop, 0, beforeTop);
    } else if (isDeletion) {
      path.moveTo(width, afterTop);
      path.bezierCurveTo(width - cpOffset, afterTop, cpOffset, beforeBottom, 0, beforeBottom);
    } else {
      path.moveTo(width, afterBottom);
      path.bezierCurveTo(width - cpOffset, afterBottom, cpOffset, beforeBottom, 0, beforeBottom);
    }

    return path;
  }

  /**
   * Find the first changed alignment that might be visible.
   */
  private findFirstVisibleChangedAlignment(
    beforeScrollY: number,
    afterScrollY: number,
    lineHeight: number,
    verticalOffset: number
  ): number {
    if (this.changedAlignmentIndices.length === 0) return 0;

    let low = 0;
    let high = this.changedAlignmentIndices.length - 1;

    while (low < high) {
      const mid = Math.floor((low + high) / 2);
      const alignmentIndex = this.changedAlignmentIndices[mid];
      const alignment = this.alignments[alignmentIndex];

      const beforeBottom = alignment.before.end * lineHeight - beforeScrollY + verticalOffset;
      const afterBottom = alignment.after.end * lineHeight - afterScrollY + verticalOffset;
      const maxBottom = Math.max(beforeBottom, afterBottom);

      if (maxBottom < 0) {
        low = mid + 1;
      } else {
        high = mid;
      }
    }

    return low;
  }

  /**
   * Render comment highlight bars.
   */
  private renderComments(
    afterScrollY: number,
    lineHeight: number,
    verticalOffset: number,
    width: number,
    height: number,
    clipTop: number
  ): void {
    const ctx = this.ctx;

    // Clear hit regions
    this.commentHitRegions = [];

    // Filter and sort comments (largest spans first for pyramid stacking)
    const validComments = this.comments
      .filter((c) => c.span.start !== 0 || c.span.end !== 0)
      .sort((a, b) => {
        const sizeA = a.span.end - a.span.start;
        const sizeB = b.span.end - b.span.start;
        if (sizeB !== sizeA) return sizeB - sizeA;
        return a.span.start - b.span.start;
      });

    // Calculate offsets for pyramid stacking
    const commentOffsets = new Map<string, number>();
    for (let i = 0; i < validComments.length; i++) {
      const comment = validComments[i];
      let offset = 0;
      for (let j = 0; j < i; j++) {
        const other = validComments[j];
        if (comment.span.start < other.span.end && comment.span.end > other.span.start) {
          offset++;
        }
      }
      commentOffsets.set(comment.id, offset);
    }

    for (const comment of validComments) {
      const { span } = comment;
      const offset = commentOffsets.get(comment.id) || 0;

      // Calculate pixel positions
      const top =
        span.start * lineHeight - afterScrollY + verticalOffset + COMMENT_VERTICAL_PADDING;
      const bottom =
        Math.max(span.end, span.start + 1) * lineHeight -
        afterScrollY +
        verticalOffset -
        COMMENT_VERTICAL_PADDING;

      // Skip if out of view or too small
      if (bottom < clipTop || top > height || bottom <= top) {
        continue;
      }

      const xPos = width - COMMENT_WIDTH - offset * (COMMENT_WIDTH + COMMENT_GAP);
      const rectHeight = bottom - top;

      // Determine if hovered
      const isHovered = this.hoveredCommentId === comment.id;
      const rectWidth = isHovered ? COMMENT_WIDTH + 1 : COMMENT_WIDTH;

      // Determine color
      const fillColor = isHovered ? this.colors.commentHoverColor : this.colors.commentColor;

      // Draw rectangle
      ctx.fillStyle = fillColor;
      ctx.beginPath();
      ctx.roundRect(xPos, top, rectWidth, rectHeight, 1);
      ctx.fill();

      // Store hit region
      this.commentHitRegions.push({
        x: xPos,
        y: top,
        width: COMMENT_WIDTH + 4, // Slightly larger hit area
        height: rectHeight,
        commentId: comment.id,
        span: comment.span,
      });
    }
  }

  /**
   * Handle mouse move for comment hover detection.
   */
  private handleMouseMove = (e: MouseEvent): void => {
    const rect = this.canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;

    let foundHover: string | null = null;

    for (const region of this.commentHitRegions) {
      if (
        x >= region.x &&
        x <= region.x + region.width &&
        y >= region.y &&
        y <= region.y + region.height
      ) {
        foundHover = region.commentId;
        break;
      }
    }

    if (foundHover !== this.hoveredCommentId) {
      this.hoveredCommentId = foundHover;
      this.canvas.style.cursor = foundHover ? 'pointer' : '';
      // Note: caller should trigger re-render if hover state matters for visuals
    }
  };

  /**
   * Handle click for comment selection.
   */
  private handleClick = (e: MouseEvent): void => {
    const rect = this.canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;

    for (const region of this.commentHitRegions) {
      if (
        x >= region.x &&
        x <= region.x + region.width &&
        y >= region.y &&
        y <= region.y + region.height
      ) {
        e.stopPropagation();
        this.config.onCommentClick?.({
          commentId: region.commentId,
          span: region.span,
        });
        break;
      }
    }
  };

  /**
   * Handle mouse leave to clear hover state.
   */
  private handleMouseLeave = (): void => {
    if (this.hoveredCommentId !== null) {
      this.hoveredCommentId = null;
      this.canvas.style.cursor = '';
    }
  };

  /**
   * Clear all rendered content (call when switching files).
   */
  clear(): void {
    const rect = this.canvas.getBoundingClientRect();
    this.ctx.clearRect(0, 0, rect.width, rect.height);

    this.alignments = [];
    this.changedAlignmentIndices = [];
    this.comments = [];
    this.hoveredAlignmentIndex = null;
    this.hoveredCommentId = null;
    this.commentHitRegions = [];
  }

  /**
   * Destroy the renderer and clean up.
   */
  destroy(): void {
    this.canvas.removeEventListener('mousemove', this.handleMouseMove);
    this.canvas.removeEventListener('click', this.handleClick);
    this.canvas.removeEventListener('mouseleave', this.handleMouseLeave);

    this.clear();
  }
}
