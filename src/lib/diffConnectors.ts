/**
 * Range Connectors
 *
 * Draws bezier curve connectors between corresponding changed ranges
 * in the center gutter. These visualize how regions in the "before"
 * pane map to the "after" pane.
 */

import type { Range } from './types';

export interface ConnectorConfig {
  lineHeight: number;
  width: number;
  fillColor: string;
  strokeColor: string;
}

const DEFAULT_CONFIG: ConnectorConfig = {
  lineHeight: 20,
  width: 24,
  fillColor: 'rgba(110, 118, 129, 0.12)',
  strokeColor: 'rgba(110, 118, 129, 0.5)',
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

  // Clear existing
  svg.innerHTML = '';

  for (const range of ranges) {
    if (!range.changed) continue;

    // Calculate pixel positions relative to viewport
    const beforeTop = range.before.start * cfg.lineHeight - beforeScroll;
    const beforeBottom = range.before.end * cfg.lineHeight - beforeScroll;
    const afterTop = range.after.start * cfg.lineHeight - afterScroll;
    const afterBottom = range.after.end * cfg.lineHeight - afterScroll;

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
    svg.appendChild(path);
  }
}
