import { describe, expect, it } from 'vitest';

import {
  BEZIER_CP_FRACTION,
  connectorFillPathData,
  connectorShape,
  connectorStrokePathData,
  SvgPathData,
  traceConnectorFill,
  traceConnectorStroke,
  type ConnectorEdges,
  type ConnectorPathSink,
} from './connectorGeometry';

/** A sink that keeps the calls made on it, as a canvas path would receive them. */
function recorder() {
  const calls: Array<[string, ...number[]]> = [];
  const sink: ConnectorPathSink = {
    moveTo: (...args) => calls.push(['moveTo', ...args]),
    lineTo: (...args) => calls.push(['lineTo', ...args]),
    bezierCurveTo: (...args) => calls.push(['bezierCurveTo', ...args]),
    closePath: () => calls.push(['closePath']),
  };
  return { sink, calls };
}

const WIDTH = 16;
const CP = WIDTH * BEZIER_CP_FRACTION;

const band: ConnectorEdges = { beforeTop: 10, beforeBottom: 50, afterTop: 30, afterBottom: 90 };
const insertion: ConnectorEdges = {
  beforeTop: 40,
  beforeBottom: 40,
  afterTop: 30,
  afterBottom: 90,
};
const deletion: ConnectorEdges = { beforeTop: 10, beforeBottom: 50, afterTop: 70, afterBottom: 70 };

describe('connectorShape', () => {
  it('is a band when both sides have a range', () => {
    expect(connectorShape(band)).toBe('band');
  });

  it('is an insertion when the before side is under a pixel tall', () => {
    expect(connectorShape(insertion)).toBe('insertion');
    expect(connectorShape({ ...insertion, beforeBottom: 40.9 })).toBe('insertion');
  });

  it('is a deletion when the after side is under a pixel tall', () => {
    expect(connectorShape(deletion)).toBe('deletion');
  });

  it('is a band at a pixel exactly, which is what a range of no lines measures after its insets', () => {
    // The canvas renderer draws a top half a pixel in and a bottom half a
    // pixel out, so an empty range is a side one pixel tall, upside down.
    expect(connectorShape({ ...band, beforeTop: 40.5, beforeBottom: 39.5 })).toBe('band');
  });

  it('is an insertion when both sides are points', () => {
    expect(connectorShape({ beforeTop: 5, beforeBottom: 5, afterTop: 9, afterBottom: 9 })).toBe(
      'insertion'
    );
  });
});

describe('traceConnectorFill', () => {
  it('is a trapezoid with an S-curve along the top and the bottom', () => {
    const { sink, calls } = recorder();

    traceConnectorFill(sink, band, WIDTH);

    expect(calls).toEqual([
      ['moveTo', 0, 10],
      ['bezierCurveTo', CP, 10, WIDTH - CP, 30, WIDTH, 30],
      ['lineTo', WIDTH, 90],
      ['bezierCurveTo', WIDTH - CP, 90, CP, 50, 0, 50],
      ['closePath'],
    ]);
  });

  it('leaves each side level: the control points are at the height of the end they are nearest', () => {
    const { sink, calls } = recorder();

    traceConnectorFill(sink, band, 40);

    const [, cp1x, cp1y, cp2x, cp2y] = calls[1];
    expect([cp1x, cp1y]).toEqual([20, band.beforeTop]);
    expect([cp2x, cp2y]).toEqual([20, band.afterTop]);
  });

  it('opens from a point on the left for an insertion', () => {
    const { sink, calls } = recorder();

    traceConnectorFill(sink, insertion, WIDTH);

    expect(calls).toEqual([
      ['moveTo', 0, 40],
      ['bezierCurveTo', CP, 40, WIDTH - CP, 30, WIDTH, 30],
      ['lineTo', WIDTH, 90],
      ['bezierCurveTo', WIDTH - CP, 90, CP, 40, 0, 40],
      ['closePath'],
    ]);
  });

  it('closes to a point on the right for a deletion', () => {
    const { sink, calls } = recorder();

    traceConnectorFill(sink, deletion, WIDTH);

    expect(calls).toEqual([
      ['moveTo', 0, 10],
      ['bezierCurveTo', CP, 10, WIDTH - CP, 70, WIDTH, 70],
      ['bezierCurveTo', WIDTH - CP, 70, CP, 50, 0, 50],
      ['closePath'],
    ]);
  });
});

describe('traceConnectorStroke', () => {
  it('is the fill’s top curve and its bottom curve, and neither end', () => {
    const { sink, calls } = recorder();

    traceConnectorStroke(sink, band, WIDTH);

    expect(calls).toEqual([
      ['moveTo', 0, 10],
      ['bezierCurveTo', CP, 10, WIDTH - CP, 30, WIDTH, 30],
      ['moveTo', WIDTH, 90],
      ['bezierCurveTo', WIDTH - CP, 90, CP, 50, 0, 50],
    ]);
  });

  it('meets at the point of an insertion and of a deletion', () => {
    const forInsertion = recorder();
    traceConnectorStroke(forInsertion.sink, insertion, WIDTH);
    expect(forInsertion.calls[0]).toEqual(['moveTo', 0, 40]);
    expect(forInsertion.calls[3]).toEqual(['bezierCurveTo', WIDTH - CP, 90, CP, 40, 0, 40]);

    const forDeletion = recorder();
    traceConnectorStroke(forDeletion.sink, deletion, WIDTH);
    expect(forDeletion.calls[1].slice(-2)).toEqual([WIDTH, 70]);
    expect(forDeletion.calls[2]).toEqual(['moveTo', WIDTH, 70]);
  });
});

describe('SvgPathData', () => {
  it('writes the calls as path data', () => {
    const path = new SvgPathData();
    path.moveTo(0, 1);
    path.bezierCurveTo(2, 3, 4, 5, 6, 7);
    path.lineTo(8, 9);
    path.closePath();

    expect(path.toString()).toBe('M0 1 C2 3 4 5 6 7 L8 9 Z');
  });

  it('keeps hundredths of a pixel', () => {
    const path = new SvgPathData();
    path.moveTo(0.004, 10.5);
    path.lineTo(1 / 3, -2.999);

    expect(path.toString()).toBe('M0 10.5 L0.33 -3');
  });
});

describe('the path data of a connector', () => {
  it('is the traced fill and the traced strokes', () => {
    expect(connectorFillPathData(band, WIDTH)).toBe(
      'M0 10 C8 10 8 30 16 30 L16 90 C8 90 8 50 0 50 Z'
    );
    expect(connectorStrokePathData(band, WIDTH)).toBe(
      'M0 10 C8 10 8 30 16 30 M16 90 C8 90 8 50 0 50'
    );
  });

  it('tapers to the point of a deletion', () => {
    expect(connectorFillPathData(deletion, WIDTH)).toBe('M0 10 C8 10 8 70 16 70 C8 70 8 50 0 50 Z');
  });
});
