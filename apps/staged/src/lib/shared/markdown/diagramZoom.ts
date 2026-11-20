import { pointer, select } from 'd3-selection';
import { zoom, zoomIdentity, zoomTransform, type D3ZoomEvent, type ZoomTransform } from 'd3-zoom';

export interface DiagramZoomTransform {
  scale: number;
  offsetX: number;
  offsetY: number;
}

interface DiagramZoomOptions {
  minScale: number;
  maxScale: number;
  fitPadding: number;
  doubleClickScale: number;
  /**
   * Double-click returns to the fit transform once the view has been zoomed
   * or panned away from it, instead of zooming in again (photo-viewer style).
   */
  doubleClickTogglesFit?: boolean;
  /**
   * Keep the content within the viewport: a zoomed-in image must cover the
   * viewport, a smaller one stays centred, so it can never be panned out of
   * view.
   */
  constrainToContent?: boolean;
  /**
   * Photo-viewer dismiss, for touch only. A pinch that starts at the fit and
   * goes below it is a close gesture: the content follows the fingers
   * unconstrained and `onProgress` reports how far below the fit it is. On
   * release `onRelease` says whether to close; the caller then closes, or
   * returns the view to the fit with `reset()`. A cancelled touch sequence
   * never closes.
   *
   * A touch gesture that starts anywhere else never closes, so zooming back
   * out cannot overshoot into a close. Pinched in past the fit it meets a
   * rubber band: the content shrinks a little, with growing resistance, and
   * on release `onRelease(false)` asks the caller to return it to the fit.
   */
  pinchDismiss?: PinchDismissOptions;
  isEnabled: () => boolean;
  onTransform: (transform: DiagramZoomTransform) => void;
  onDraggingChange: (dragging: boolean) => void;
}

export interface PinchDismissOptions {
  /** 0 at the fit, 1 once the content is small enough that the view behind should show fully. */
  onProgress: (progress: number) => void;
  onRelease: (close: boolean) => void;
}

export interface DiagramZoomController {
  reset: () => void;
  /** Place the content programmatically, bypassing the scale limits and the content constraint. */
  setTransform: (transform: DiagramZoomTransform) => void;
  /**
   * Bring a programmatic placement to where an ordinary zoom would have left
   * it: the scale within the limits, scaled about `anchor` (viewport
   * coordinates) when it has to change, and the content constraint applied.
   */
  settleTransform: (transform: DiagramZoomTransform, anchor: [number, number]) => void;
  /** Re-fit after the viewport resized, unless the user has zoomed or panned since the last fit. */
  refit: () => void;
  zoomBy: (multiplier: number) => void;
  getResetTransform: () => DiagramZoomTransform;
  destroy: () => void;
}

const DOUBLE_CLICK_DELAY_MS = 500;
const DOUBLE_CLICK_DISTANCE_PX = 8;

// Pinch dismiss, as fractions of the fit scale: how small the content can get,
// where the view behind is fully revealed, and where a release closes.
const DISMISS_MIN_RATIO = 0.3;
const DISMISS_FULL_RATIO = 0.6;
const DISMISS_CLOSE_RATIO = 0.82;
// A quick pinch closes from a smaller distance: it has to be past the nearer
// ratio and shrinking by at least this share of the fit per second.
const DISMISS_FLICK_RATIO = 0.95;
const DISMISS_FLICK_SPEED = 1.2;
const DISMISS_FLICK_WINDOW_MS = 120;
// Rubber band at the fit, for a pinch-in that started zoomed in: the most the
// content gives (as a share of the fit scale, approached but never reached)
// and how much of the fingers' travel it follows to begin with.
const RUBBER_MAX_GIVE = 0.2;
const RUBBER_FOLLOW = 0.55;
// Far enough below the fit that the fingers never reach the scale floor.
const RUBBER_MIN_RATIO = 0.01;

export function createDiagramZoomController(
  viewportEl: HTMLDivElement,
  contentEl: HTMLElement,
  options: DiagramZoomOptions
): DiagramZoomController {
  const selection = select<HTMLDivElement, unknown>(viewportEl);
  let lastClick: { time: number; x: number; y: number } | null = null;
  let lastFit: ZoomTransform | null = null;
  // The fit a touch gesture is measured against while it is one that started
  // at the fit and may therefore dismiss; null for every other gesture.
  let dismissFit: ZoomTransform | null = null;
  let dismissSamples: Array<{ time: number; k: number }> = [];
  // The fit of a touch gesture that started away from it. Below this the
  // gesture is rubber-banded: d3-zoom keeps the scale the fingers ask for and
  // only what is reported to `onTransform` resists.
  let rubberFit: ZoomTransform | null = null;
  const behavior = zoom<HTMLDivElement, unknown>()
    .scaleExtent([options.minScale, options.maxScale])
    .constrain((transform, extent) =>
      options.constrainToContent && !isDismissing(transform.k)
        ? constrainToContent(transform, extent, contentEl)
        : transform
    )
    // Layout size rather than getBoundingClientRect: the dialog's open
    // animation scales its content, and a fit computed from the transformed
    // rect would be a few percent off.
    .extent(() => {
      return [
        [0, 0],
        [viewportEl.clientWidth, viewportEl.clientHeight],
      ] satisfies [[number, number], [number, number]];
    })
    .filter((event) => {
      if (event.type === 'wheel') {
        return options.isEnabled() && (event.ctrlKey || event.metaKey);
      }

      return (
        options.isEnabled() &&
        event.type !== 'dblclick' &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.button
      );
    })
    .on('start', (event: D3ZoomEvent<HTMLDivElement, unknown>) => {
      options.onDraggingChange(event.sourceEvent?.type === 'mousedown');
      if (event.sourceEvent?.type === 'touchstart') beginTouchGesture();
    })
    .on('zoom', (event: D3ZoomEvent<HTMLDivElement, unknown>) => {
      options.onTransform(toDiagramTransform(rubberBanded(event.transform)));
      if (dismissFit) trackDismiss(event.transform.k, dismissFit);
    })
    .on('end', (event: D3ZoomEvent<HTMLDivElement, unknown>) => {
      options.onDraggingChange(false);
      endTouchGesture(event.transform.k, event.sourceEvent?.type === 'touchcancel');
    });

  selection.call(behavior);
  selection.on('dblclick.zoom', null);
  selection.on('click.diagram-zoom', (event: MouseEvent) => {
    if (!options.isEnabled()) return;

    const now = Date.now();
    const isDoubleClick =
      lastClick !== null &&
      now - lastClick.time <= DOUBLE_CLICK_DELAY_MS &&
      distanceSquared(lastClick, event) <= DOUBLE_CLICK_DISTANCE_PX * DOUBLE_CLICK_DISTANCE_PX;

    if (!isDoubleClick) {
      lastClick = { time: now, x: event.clientX, y: event.clientY };
      return;
    }

    lastClick = null;
    if (options.doubleClickTogglesFit && !isAtLastFit()) {
      event.preventDefault();
      event.stopPropagation();
      reset();
      return;
    }
    zoomByAtPointer(event, event.altKey ? 1 / options.doubleClickScale : options.doubleClickScale);
  });

  selection.on('dblclick.diagram-zoom', (event: MouseEvent) => {
    if (!options.isEnabled()) return;

    event.preventDefault();
    event.stopPropagation();
  });

  selection.on('wheel.diagram-pan', (event: WheelEvent) => {
    if (!options.isEnabled() || event.ctrlKey || event.metaKey) return;

    event.preventDefault();
    const [deltaX, deltaY] = normalizeWheelDelta(event, viewportEl);
    // translateBy rather than transform(): only the former runs the
    // constrain function, so a constrained image cannot be scrolled off the
    // viewport.
    const { k } = zoomTransform(viewportEl);
    behavior.translateBy(selection, -deltaX / k, -deltaY / k);
  });

  return {
    reset,
    setTransform: (transform) => {
      behavior.transform(
        selection,
        zoomIdentity.translate(transform.offsetX, transform.offsetY).scale(transform.scale)
      );
    },
    settleTransform: (transform, anchor) => {
      syncScaleFloor();
      const [minScale, maxScale] = behavior.scaleExtent();
      const k = Math.min(maxScale, Math.max(minScale, transform.scale));
      const ratio = transform.scale > 0 ? k / transform.scale : 1;
      const placed = zoomIdentity
        .translate(
          anchor[0] - (anchor[0] - transform.offsetX) * ratio,
          anchor[1] - (anchor[1] - transform.offsetY) * ratio
        )
        .scale(k);
      const extent: [[number, number], [number, number]] = [
        [0, 0],
        [viewportEl.clientWidth, viewportEl.clientHeight],
      ];
      // transform() does not run the constrain function itself.
      behavior.transform(
        selection,
        options.constrainToContent ? constrainToContent(placed, extent, contentEl) : placed
      );
    },
    refit: () => {
      if (isAtLastFit()) {
        reset();
      } else {
        syncScaleFloor();
      }
    },
    zoomBy: (multiplier: number) => {
      behavior.scaleBy(selection, multiplier, [
        viewportEl.clientWidth / 2,
        viewportEl.clientHeight / 2,
      ]);
    },
    getResetTransform: () => toDiagramTransform(getFitTransform(viewportEl, contentEl, options)),
    destroy: () => {
      selection.on('.zoom', null);
      selection.on('.diagram-zoom', null);
      selection.on('.diagram-pan', null);
      options.onDraggingChange(false);
    },
  };

  function zoomByAtPointer(event: MouseEvent, multiplier: number) {
    event.preventDefault();
    event.stopPropagation();
    behavior.scaleBy(selection, multiplier, pointer(event, viewportEl));
  }

  function reset() {
    lastFit = getFitTransform(viewportEl, contentEl, options);
    syncScaleFloor(lastFit);
    behavior.transform(selection, lastFit);
    options.onDraggingChange(false);
  }

  // The fit scale of a very tall or wide image can fall below minScale; the
  // floor follows it so a pinch can always get back out to the whole image.
  function syncScaleFloor(fit = getFitTransform(viewportEl, contentEl, options)) {
    behavior.scaleExtent([Math.min(options.minScale, fit.k), options.maxScale]);
  }

  // With pinch dismiss on, where a touch gesture starts decides what its
  // pinch-in means. At the fit it may go below, into the dismiss zone;
  // anywhere else the fit is a rubber band.
  function beginTouchGesture() {
    if (!options.pinchDismiss) return;
    const atFit = lastFit !== null && isAtLastFit();
    const fit = atFit && lastFit ? lastFit : getFitTransform(viewportEl, contentEl, options);
    dismissFit = atFit ? fit : null;
    rubberFit = atFit ? null : fit;
    dismissSamples = [];
    behavior.scaleExtent([
      fit.k * (dismissFit ? DISMISS_MIN_RATIO : RUBBER_MIN_RATIO),
      options.maxScale,
    ]);
  }

  function isRubberBanding(k: number): boolean {
    return rubberFit !== null && k < rubberFit.k - 0.001;
  }

  // What to show for a rubber-banded transform: centred, as content smaller
  // than the viewport always is here, at a scale that gives less and less.
  function rubberBanded(transform: ZoomTransform): ZoomTransform {
    if (!rubberFit || !isRubberBanding(transform.k)) return transform;
    const pulled = 1 - transform.k / rubberFit.k;
    const give = RUBBER_MAX_GIVE * (1 - 1 / ((pulled * RUBBER_FOLLOW) / RUBBER_MAX_GIVE + 1));
    const k = rubberFit.k * (1 - give);
    return zoomIdentity
      .translate(
        (viewportEl.clientWidth - contentEl.offsetWidth * k) / 2,
        (viewportEl.clientHeight - contentEl.offsetHeight * k) / 2
      )
      .scale(k);
  }

  function isDismissing(k: number): boolean {
    return dismissFit !== null && k < dismissFit.k - 0.001;
  }

  function trackDismiss(k: number, fit: ZoomTransform) {
    const now = performance.now();
    dismissSamples.push({ time: now, k });
    while (dismissSamples.length > 2 && now - dismissSamples[1].time >= DISMISS_FLICK_WINDOW_MS) {
      dismissSamples.shift();
    }
    options.pinchDismiss?.onProgress(clamp01((fit.k - k) / (fit.k * (1 - DISMISS_FULL_RATIO))));
  }

  function endTouchGesture(k: number, cancelled: boolean) {
    if (!options.pinchDismiss) return;
    const fit = dismissFit;
    const wasDismissing = isDismissing(k);
    const wasRubberBanding = isRubberBanding(k);
    dismissFit = null;
    rubberFit = null;
    syncScaleFloor();
    if (wasRubberBanding) {
      options.pinchDismiss.onRelease(false);
      return;
    }
    if (!fit || !wasDismissing) return;
    // A touch sequence the system took away (a call sheet, an edge swipe) is
    // not a decision to close.
    options.pinchDismiss.onRelease(
      !cancelled &&
        (k < fit.k * DISMISS_CLOSE_RATIO ||
          (k < fit.k * DISMISS_FLICK_RATIO && isShrinkingFast(fit.k)))
    );
  }

  function isShrinkingFast(fitScale: number): boolean {
    const first = dismissSamples[0];
    const last = dismissSamples.at(-1);
    if (!first || !last || last.time <= first.time) return false;
    // A pinch held still before the fingers lift is not a flick.
    if (performance.now() - last.time > DISMISS_FLICK_WINDOW_MS) return false;
    const perSecond = ((last.k - first.k) / fitScale / (last.time - first.time)) * 1000;
    return perSecond <= -DISMISS_FLICK_SPEED;
  }

  function isAtLastFit(): boolean {
    if (!lastFit) return false;
    const current = zoomTransform(viewportEl);
    return (
      isClose(current.k, lastFit.k) &&
      isClose(current.x, lastFit.x) &&
      isClose(current.y, lastFit.y)
    );
  }
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

function isClose(a: number, b: number): boolean {
  return Math.abs(a - b) < 0.001;
}

// d3-zoom's default constrain with the content box as the translate extent,
// read at gesture time so it follows the content's current layout size.
function constrainToContent(
  transform: ZoomTransform,
  extent: [[number, number], [number, number]],
  contentEl: HTMLElement
): ZoomTransform {
  const dx0 = transform.invertX(extent[0][0]);
  const dx1 = transform.invertX(extent[1][0]) - contentEl.offsetWidth;
  const dy0 = transform.invertY(extent[0][1]);
  const dy1 = transform.invertY(extent[1][1]) - contentEl.offsetHeight;
  return transform.translate(
    dx1 > dx0 ? (dx0 + dx1) / 2 : Math.min(0, dx0) || Math.max(0, dx1),
    dy1 > dy0 ? (dy0 + dy1) / 2 : Math.min(0, dy0) || Math.max(0, dy1)
  );
}

function distanceSquared(
  a: { x: number; y: number },
  b: { clientX: number; clientY: number }
): number {
  const dx = a.x - b.clientX;
  const dy = a.y - b.clientY;
  return dx * dx + dy * dy;
}

function getFitTransform(
  viewportEl: HTMLDivElement,
  contentEl: HTMLElement,
  options: DiagramZoomOptions
): ZoomTransform {
  const viewportWidth = viewportEl.clientWidth;
  const viewportHeight = viewportEl.clientHeight;
  const contentWidth = contentEl.offsetWidth;
  const contentHeight = contentEl.offsetHeight;

  if (viewportWidth <= 0 || viewportHeight <= 0 || contentWidth <= 0 || contentHeight <= 0) {
    return zoomIdentity;
  }

  const availableWidth = Math.max(1, viewportWidth - options.fitPadding * 2);
  const availableHeight = Math.max(1, viewportHeight - options.fitPadding * 2);
  const fitScale = Math.min(1, availableWidth / contentWidth, availableHeight / contentHeight);
  const scale = Math.min(fitScale, options.maxScale);
  const offsetX = (viewportWidth - contentWidth * scale) / 2;
  const offsetY = (viewportHeight - contentHeight * scale) / 2;

  return zoomIdentity.translate(offsetX, offsetY).scale(scale);
}

function toDiagramTransform(transform: ZoomTransform): DiagramZoomTransform {
  return {
    scale: transform.k,
    offsetX: transform.x,
    offsetY: transform.y,
  };
}

function normalizeWheelDelta(event: WheelEvent, viewportEl: HTMLDivElement): [number, number] {
  if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
    return [event.deltaX * 16, event.deltaY * 16];
  }

  if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
    return [event.deltaX * viewportEl.clientWidth, event.deltaY * viewportEl.clientHeight];
  }

  return [event.deltaX, event.deltaY];
}
