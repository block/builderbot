import { describe, expect, it, vi } from 'vitest';
import { createDialogResize, resizeBounds, type ResizeGeometry } from './dialogResize';

function setup(initial: ResizeGeometry = { width: 1136, min: 700, max: 1136 }) {
  let geometry = initial;
  const preview = vi.fn();
  const commit = vi.fn();
  const end = vi.fn();
  const resize = createDialogResize({ geometry: () => geometry, preview, commit, end });
  return { resize, preview, commit, end, layout: (next: ResizeGeometry) => (geometry = next) };
}

const pointer = (clientX: number, pointerId = 1) => ({ clientX, pointerId });

describe('dialog resize gestures', () => {
  it('starts at the rendered cap and tracks a centered right edge immediately', () => {
    const { resize, preview, commit, end } = setup();
    resize.start(pointer(1168));
    resize.move(pointer(1158));
    expect(preview).toHaveBeenLastCalledWith(1116);
    resize.release(pointer(1158));
    resize.release(pointer(1158));
    expect(commit).toHaveBeenCalledExactlyOnceWith(1116);
    expect(end).toHaveBeenCalledTimes(1);
  });

  it.each(['click', 'vertical', 'bound', 'return'])(
    'preserves a capped preference after %s',
    (kind) => {
      const { resize, commit, end } = setup();
      resize.start(pointer(100));
      if (kind === 'return') resize.move(pointer(90));
      const finalX = kind === 'bound' ? 200 : 100;
      resize.move(pointer(finalX));
      resize.release(pointer(finalX));
      expect(commit).not.toHaveBeenCalled();
      expect(end).toHaveBeenCalledTimes(1);
    }
  );

  it('commits the final pointer position even without a final move event', () => {
    const { resize, commit } = setup();
    resize.start(pointer(100));
    resize.release(pointer(90));
    expect(commit).toHaveBeenCalledExactlyOnceWith(1116);
  });

  it('ignores secondary pointers and an unrelated cancellation', () => {
    const { resize, preview, commit, end } = setup();
    resize.start(pointer(100));
    expect(resize.start(pointer(100, 2))).toBe(false);
    resize.move(pointer(50, 2));
    resize.release(pointer(50, 2));
    resize.cancel(2);
    expect(preview).toHaveBeenCalledTimes(1);
    expect(commit).not.toHaveBeenCalled();
    expect(end).not.toHaveBeenCalled();
    resize.move(pointer(90));
    resize.release(pointer(90));
    expect(commit).toHaveBeenCalledExactlyOnceWith(1116);
  });

  it.each([1, undefined])('discards interrupted previews once (pointer=%s)', (id) => {
    const { resize, commit, end } = setup();
    resize.start(pointer(100));
    resize.move(pointer(80));
    resize.cancel(id);
    resize.cancel(id);
    resize.release(pointer(80));
    expect(commit).not.toHaveBeenCalled();
    expect(end).toHaveBeenCalledTimes(1);
  });

  it('clamps drag previews to both bounds', () => {
    const { resize, preview, commit } = setup();
    resize.start(pointer(100));
    resize.move(pointer(-1000));
    expect(preview).toHaveBeenLastCalledWith(700);
    resize.release(pointer(2000));
    expect(preview).toHaveBeenLastCalledWith(1136);
    expect(commit).not.toHaveBeenCalled();
  });

  it('uses fresh geometry after a cancelled layout change', () => {
    const { resize, layout, commit } = setup();
    resize.start(pointer(100));
    resize.move(pointer(80));
    resize.cancel();
    layout({ width: 1400, min: 1080, max: 1536 });
    resize.start(pointer(100));
    resize.release(pointer(90));
    expect(commit).toHaveBeenCalledExactlyOnceWith(1380);
  });

  it.each([
    { width: 730, min: 700, max: 935 },
    { width: 1110, min: 1080, max: 1136 },
  ])('cancels when release precedes layout cleanup: %o', (geometry) => {
    const { resize, layout, commit, end } = setup();
    resize.start(pointer(100));
    resize.move(pointer(90));
    layout(geometry);
    resize.release(pointer(90));
    expect(commit).not.toHaveBeenCalled();
    expect(end).toHaveBeenCalledTimes(1);
  });
});

describe('dialog keyboard resizing', () => {
  it.each([
    ['ArrowLeft', 1120],
    ['Home', 700],
  ])('starts %s at rendered width', (key, expected) => {
    const { resize, commit } = setup();
    expect(resize.key(key as string)).toBe(true);
    expect(commit).toHaveBeenCalledExactlyOnceWith(expected);
  });

  it.each(['ArrowRight', 'End'])('does not persist %s beyond the rendered cap', (key) => {
    const { resize, commit } = setup();
    expect(resize.key(key)).toBe(true);
    expect(commit).not.toHaveBeenCalled();
  });

  it.each(['ArrowLeft', 'Home'])('does not persist %s below the minimum', (key) => {
    const { resize, commit } = setup({ width: 700, min: 700, max: 1136 });
    resize.key(key);
    expect(commit).not.toHaveBeenCalled();
  });

  it('does not let keyboard actions commit during a pointer gesture', () => {
    const { resize, commit } = setup();
    resize.start(pointer(100));
    resize.key('ArrowLeft');
    expect(resize.key('Tab')).toBe(false);
    resize.cancel();
    expect(commit).not.toHaveBeenCalled();
  });

  it.each([936, 1079])('exposes a collapsed range below the split minimum at %spx', (available) => {
    const bounds = resizeBounds(1080, available);
    expect(bounds).toEqual({ min: available, max: available });
    const { resize, commit } = setup({ width: available, ...bounds });
    for (const key of ['ArrowLeft', 'ArrowRight', 'Home', 'End']) resize.key(key);
    resize.start(pointer(100));
    resize.release(pointer(80));
    expect(commit).not.toHaveBeenCalled();
  });
});
