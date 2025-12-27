/**
 * Spine Connectors
 *
 * Draws bezier curve connectors between corresponding changed ranges
 * in the center spine. These visualize how regions in the "before"
 * pane map to the "after" pane.
 */

import type { Range } from './types';

export interface ConnectorConfig {
  lineHeight: number;
  width: number;
  fillColor: string;
  strokeColor: string;
  /** Vertical offset to adjust bezier alignment (positive = down, negative = up) */
  verticalOffset: number;
}

const DEFAULT_CONFIG: ConnectorConfig = {
  lineHeight: 20,
  width: 24,
  fillColor: 'rgba(110, 118, 129, 0.12)',
  strokeColor: 'rgba(110, 118, 129, 0.7)',
  verticalOffset: 0,
};

/**
 * Draw connectors between changed ranges.
 */
export function drawConnectors(
  svg: SVGSVGElement,
  ranges: Range[],
  beforeScroll: number,
  afterScroll: number,
  config: Partial<ConnectorConfig> = {}
): void {
  const cfg = { ...DEFAULT_CONFIG, ...config };
  const svgHeight = svg.clientHeight;
  const cpOffset = cfg.width * 0.5; // bezier control point offset

  // Clear existing and set crisp rendering
  svg.innerHTML = '';
  svg.setAttribute('shape-rendering', 'geometricPrecision');

  for (const range of ranges) {
    if (!range.changed) continue;

    // Calculate pixel positions relative to viewport
    // Top border: at the top edge of the first line (start * lineHeight)
    // Bottom border: at the bottom edge of the last line (end * lineHeight)
    // The CSS pseudo-elements for range-start/range-end are at top:0 and bottom:0 of their lines
    // Add 0.5px offset for crisp 1px stroke rendering on pixel boundaries
    // Bottom uses -0.5 to align with the CSS ::after pseudo-element at bottom:0
    // verticalOffset adjusts for structural differences between SVG and code container
    const beforeTop = range.before.start * cfg.lineHeight - beforeScroll + 0.5 + cfg.verticalOffset;
    const beforeBottom =
      range.before.end * cfg.lineHeight - beforeScroll - 0.5 + cfg.verticalOffset;
    const afterTop = range.after.start * cfg.lineHeight - afterScroll + 0.5 + cfg.verticalOffset;
    const afterBottom = range.after.end * cfg.lineHeight - afterScroll - 0.5 + cfg.verticalOffset;

    // Skip if completely out of view
    if (beforeBottom < 0 && afterBottom < 0) continue;
    if (beforeTop > svgHeight && afterTop > svgHeight) continue;

    const isInsertion = range.before.start === range.before.end;
    const isDeletion = range.after.start === range.after.end;

    // Build bezier path
    let d: string;
    if (isInsertion) {
      // Point on left, range on right
      d =
        `M 0 ${beforeTop} ` +
        `C ${cpOffset} ${beforeTop}, ${cfg.width - cpOffset} ${afterTop}, ${cfg.width} ${afterTop} ` +
        `L ${cfg.width} ${afterBottom} ` +
        `C ${cfg.width - cpOffset} ${afterBottom}, ${cpOffset} ${beforeTop}, 0 ${beforeTop} Z`;
    } else if (isDeletion) {
      // Range on left, point on right
      d =
        `M 0 ${beforeTop} ` +
        `C ${cpOffset} ${beforeTop}, ${cfg.width - cpOffset} ${afterTop}, ${cfg.width} ${afterTop} ` +
        `C ${cfg.width - cpOffset} ${afterTop}, ${cpOffset} ${beforeBottom}, 0 ${beforeBottom} Z`;
    } else {
      // Curved trapezoid
      d =
        `M 0 ${beforeTop} ` +
        `C ${cpOffset} ${beforeTop}, ${cfg.width - cpOffset} ${afterTop}, ${cfg.width} ${afterTop} ` +
        `L ${cfg.width} ${afterBottom} ` +
        `C ${cfg.width - cpOffset} ${afterBottom}, ${cpOffset} ${beforeBottom}, 0 ${beforeBottom} Z`;
    }

    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute('d', d);
    path.setAttribute('fill', cfg.fillColor);
    path.setAttribute('stroke', cfg.strokeColor);
    path.setAttribute('stroke-width', '1');
    path.setAttribute('vector-effect', 'non-scaling-stroke');
    svg.appendChild(path);
  }
}
