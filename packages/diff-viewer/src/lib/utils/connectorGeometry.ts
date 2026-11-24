/**
 * Connector geometry
 *
 * The shape of a spine connector: the band drawn between two panes that
 * joins a changed range on the before side (its left edge) to the same
 * change on the after side (its right edge).
 *
 * The shape is traced into a path sink rather than a `Path2D`, so the one
 * definition draws wherever a path can be built: `ConnectorRendererCanvas`
 * hands it a `Path2D`, and a caller that draws with SVG takes the same
 * outline as path data (`connectorFillPathData`, `connectorStrokePathData`).
 * Nothing here touches the DOM.
 */

/** Bezier control point offset as fraction of width */
export const BEZIER_CP_FRACTION = 0.5;

/**
 * Where a connector meets each pane, in the pixels of the surface it is
 * drawn on: the top and bottom of its range at the left edge (before) and
 * at the right edge (after).
 */
export interface ConnectorEdges {
  beforeTop: number;
  beforeBottom: number;
  afterTop: number;
  afterBottom: number;
}

/** The path operations a connector is traced with; `Path2D` is one. */
export interface ConnectorPathSink {
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  bezierCurveTo(cp1x: number, cp1y: number, cp2x: number, cp2y: number, x: number, y: number): void;
  closePath(): void;
}

/**
 * - `insertion`: the before side has no range, so the band is a point on
 *   the left opening to the range on the right.
 * - `deletion`: the after side has none, and the band closes to a point on
 *   the right.
 * - `band`: both sides have a range.
 */
export type ConnectorShape = 'insertion' | 'deletion' | 'band';

/** A side under a pixel tall is a point. */
export function connectorShape(edges: ConnectorEdges): ConnectorShape {
  if (Math.abs(edges.beforeTop - edges.beforeBottom) < 1) return 'insertion';
  if (Math.abs(edges.afterTop - edges.afterBottom) < 1) return 'deletion';
  return 'band';
}

/**
 * Trace the connector's fill: a curved trapezoid from x = 0 to x = `width`,
 * leaving each side level and crossing with one S-curve.
 */
export function traceConnectorFill(
  path: ConnectorPathSink,
  edges: ConnectorEdges,
  width: number
): void {
  const { beforeTop, beforeBottom, afterTop, afterBottom } = edges;
  const cpOffset = width * BEZIER_CP_FRACTION;
  const shape = connectorShape(edges);

  path.moveTo(0, beforeTop);
  path.bezierCurveTo(cpOffset, beforeTop, width - cpOffset, afterTop, width, afterTop);
  if (shape === 'insertion') {
    // Point on left, range on right
    path.lineTo(width, afterBottom);
    path.bezierCurveTo(width - cpOffset, afterBottom, cpOffset, beforeTop, 0, beforeTop);
  } else if (shape === 'deletion') {
    // Range on left, point on right
    path.bezierCurveTo(width - cpOffset, afterTop, cpOffset, beforeBottom, 0, beforeBottom);
  } else {
    // Curved trapezoid
    path.lineTo(width, afterBottom);
    path.bezierCurveTo(width - cpOffset, afterBottom, cpOffset, beforeBottom, 0, beforeBottom);
  }
  path.closePath();
}

/** Trace the connector's strokes: its top curve and its bottom curve. */
export function traceConnectorStroke(
  path: ConnectorPathSink,
  edges: ConnectorEdges,
  width: number
): void {
  const { beforeTop, beforeBottom, afterTop, afterBottom } = edges;
  const cpOffset = width * BEZIER_CP_FRACTION;
  const shape = connectorShape(edges);

  // Top stroke
  path.moveTo(0, beforeTop);
  path.bezierCurveTo(cpOffset, beforeTop, width - cpOffset, afterTop, width, afterTop);

  // Bottom stroke
  if (shape === 'insertion') {
    path.moveTo(width, afterBottom);
    path.bezierCurveTo(width - cpOffset, afterBottom, cpOffset, beforeTop, 0, beforeTop);
  } else if (shape === 'deletion') {
    path.moveTo(width, afterTop);
    path.bezierCurveTo(width - cpOffset, afterTop, cpOffset, beforeBottom, 0, beforeBottom);
  } else {
    path.moveTo(width, afterBottom);
    path.bezierCurveTo(width - cpOffset, afterBottom, cpOffset, beforeBottom, 0, beforeBottom);
  }
}

/** Hundredths of a pixel: finer than anything drawn, and short to write. */
function coordinate(value: number): string {
  return String(Math.round(value * 100) / 100);
}

/** A path sink that writes SVG path data, for a `<path>`'s `d`. */
export class SvgPathData implements ConnectorPathSink {
  private commands: string[] = [];

  moveTo(x: number, y: number): void {
    this.commands.push(`M${coordinate(x)} ${coordinate(y)}`);
  }

  lineTo(x: number, y: number): void {
    this.commands.push(`L${coordinate(x)} ${coordinate(y)}`);
  }

  bezierCurveTo(
    cp1x: number,
    cp1y: number,
    cp2x: number,
    cp2y: number,
    x: number,
    y: number
  ): void {
    this.commands.push(`C${[cp1x, cp1y, cp2x, cp2y, x, y].map(coordinate).join(' ')}`);
  }

  closePath(): void {
    this.commands.push('Z');
  }

  toString(): string {
    return this.commands.join(' ');
  }
}

/** The connector's fill as SVG path data. */
export function connectorFillPathData(edges: ConnectorEdges, width: number): string {
  const path = new SvgPathData();
  traceConnectorFill(path, edges, width);
  return path.toString();
}

/** The connector's strokes as SVG path data. */
export function connectorStrokePathData(edges: ConnectorEdges, width: number): string {
  const path = new SvgPathData();
  traceConnectorStroke(path, edges, width);
  return path.toString();
}
