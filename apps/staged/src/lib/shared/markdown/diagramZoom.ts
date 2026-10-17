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
  isEnabled: () => boolean;
  onTransform: (transform: DiagramZoomTransform) => void;
  onDraggingChange: (dragging: boolean) => void;
}

export interface DiagramZoomController {
  reset: () => void;
  /** Re-fit after the viewport resized, unless the user has zoomed or panned since the last fit. */
  refit: () => void;
  zoomBy: (multiplier: number) => void;
  getResetTransform: () => DiagramZoomTransform;
  destroy: () => void;
}

const DOUBLE_CLICK_DELAY_MS = 500;
const DOUBLE_CLICK_DISTANCE_PX = 8;

export function createDiagramZoomController(
  viewportEl: HTMLDivElement,
  contentEl: HTMLElement,
  options: DiagramZoomOptions
): DiagramZoomController {
  const selection = select<HTMLDivElement, unknown>(viewportEl);
  let lastClick: { time: number; x: number; y: number } | null = null;
  let lastFit: ZoomTransform | null = null;
  const behavior = zoom<HTMLDivElement, unknown>()
    .scaleExtent([options.minScale, options.maxScale])
    .constrain((transform, extent) =>
      options.constrainToContent ? constrainToContent(transform, extent, contentEl) : transform
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
    })
    .on('zoom', (event: D3ZoomEvent<HTMLDivElement, unknown>) => {
      options.onTransform(toDiagramTransform(event.transform));
    })
    .on('end', () => {
      options.onDraggingChange(false);
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
    const currentTransform = zoomTransform(viewportEl);
    behavior.transform(
      selection,
      currentTransform.translate(-deltaX / currentTransform.k, -deltaY / currentTransform.k)
    );
  });

  return {
    reset,
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
