import { select } from 'd3-selection';
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
  isEnabled: () => boolean;
  onTransform: (transform: DiagramZoomTransform) => void;
  onDraggingChange: (dragging: boolean) => void;
}

export interface DiagramZoomController {
  reset: () => void;
  zoomBy: (multiplier: number) => void;
  getResetTransform: () => DiagramZoomTransform;
  destroy: () => void;
}

export function createDiagramZoomController(
  viewportEl: HTMLDivElement,
  contentEl: HTMLElement,
  options: DiagramZoomOptions
): DiagramZoomController {
  const selection = select<HTMLDivElement, unknown>(viewportEl);
  const behavior = zoom<HTMLDivElement, unknown>()
    .scaleExtent([options.minScale, options.maxScale])
    .extent(() => {
      const rect = viewportEl.getBoundingClientRect();
      return [
        [0, 0],
        [rect.width, rect.height],
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
    reset: () => {
      behavior.transform(selection, getFitTransform(viewportEl, contentEl, options));
      options.onDraggingChange(false);
    },
    zoomBy: (multiplier: number) => {
      const rect = viewportEl.getBoundingClientRect();
      behavior.scaleBy(selection, multiplier, [rect.width / 2, rect.height / 2]);
    },
    getResetTransform: () => toDiagramTransform(getFitTransform(viewportEl, contentEl, options)),
    destroy: () => {
      selection.on('.zoom', null);
      selection.on('.diagram-pan', null);
      options.onDraggingChange(false);
    },
  };
}

function getFitTransform(
  viewportEl: HTMLDivElement,
  contentEl: HTMLElement,
  options: DiagramZoomOptions
): ZoomTransform {
  const viewportRect = viewportEl.getBoundingClientRect();
  const contentWidth = contentEl.offsetWidth;
  const contentHeight = contentEl.offsetHeight;

  if (
    viewportRect.width <= 0 ||
    viewportRect.height <= 0 ||
    contentWidth <= 0 ||
    contentHeight <= 0
  ) {
    return zoomIdentity;
  }

  const availableWidth = Math.max(1, viewportRect.width - options.fitPadding * 2);
  const availableHeight = Math.max(1, viewportRect.height - options.fitPadding * 2);
  const fitScale = Math.min(1, availableWidth / contentWidth, availableHeight / contentHeight);
  const scale = Math.min(fitScale, options.maxScale);
  const offsetX = (viewportRect.width - contentWidth * scale) / 2;
  const offsetY = (viewportRect.height - contentHeight * scale) / 2;

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
