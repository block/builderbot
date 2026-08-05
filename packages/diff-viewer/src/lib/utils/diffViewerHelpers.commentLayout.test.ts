import { describe, expect, it } from 'vitest';
import {
  buildLineCommentEditorLayout,
  buildRangeCommentEditorLayout,
  MAX_COMMENT_EDITOR_WIDTH,
} from './diffViewerHelpers';

/** Minimal DOMRect stand-in; the layout builders only read top/bottom/left/width. */
function rect(partial: Partial<DOMRect>): DOMRect {
  return {
    top: 0,
    bottom: 0,
    left: 0,
    right: 0,
    width: 0,
    height: 0,
    x: 0,
    y: 0,
    toJSON: () => ({}),
    ...partial,
  } as DOMRect;
}

const viewerRect = rect({ top: 0, left: 0, width: 2400 });
const anchorLineRect = rect({ top: 300, bottom: 320 });
const EDITOR_HEIGHT = 120;
const PANE_HORIZONTAL_PADDING = 12;

describe('buildRangeCommentEditorLayout', () => {
  it('spans the pane minus padding when the pane is narrow', () => {
    const paneRect = rect({ top: 0, bottom: 800, left: 0, width: 500 });
    const layout = buildRangeCommentEditorLayout(
      viewerRect,
      paneRect,
      anchorLineRect,
      'below',
      EDITOR_HEIGHT,
      PANE_HORIZONTAL_PADDING
    );
    expect(layout.width).toBe(500 - 2 * PANE_HORIZONTAL_PADDING);
  });

  it('clamps to MAX_COMMENT_EDITOR_WIDTH on a wide pane', () => {
    const paneRect = rect({ top: 0, bottom: 800, left: 0, width: 2400 });
    const layout = buildRangeCommentEditorLayout(viewerRect, paneRect, anchorLineRect, 'below');
    expect(layout.width).toBe(MAX_COMMENT_EDITOR_WIDTH);
  });
});

describe('buildLineCommentEditorLayout', () => {
  it('spans the pane minus padding when the pane is narrow', () => {
    const paneRect = rect({ top: 0, bottom: 800, left: 0, width: 500 });
    const layout = buildLineCommentEditorLayout(
      viewerRect,
      paneRect,
      anchorLineRect,
      'below',
      EDITOR_HEIGHT,
      PANE_HORIZONTAL_PADDING
    );
    expect(layout.width).toBe(500 - 2 * PANE_HORIZONTAL_PADDING);
  });

  it('clamps to MAX_COMMENT_EDITOR_WIDTH on a wide pane', () => {
    const paneRect = rect({ top: 0, bottom: 800, left: 0, width: 2400 });
    const layout = buildLineCommentEditorLayout(viewerRect, paneRect, anchorLineRect, 'below');
    expect(layout.width).toBe(MAX_COMMENT_EDITOR_WIDTH);
  });
});
