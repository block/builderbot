import { select } from 'd3-selection';
import { zoom, zoomIdentity, type D3ZoomEvent, type ZoomTransform } from 'd3-zoom';

export interface DiagramZoomTransform {
  scale: number;
  offsetX: number;
  offsetY: number;
}

interface DiagramZoomOptions {
  minScale: number;
  maxScale: number;
  isEnabled: () => boolean;
  onTransform: (transform: DiagramZoomTransform) => void;
  onDraggingChange: (dragging: boolean) => void;
}

export interface DiagramZoomController {
  reset: () => void;
  zoomBy: (multiplier: number) => void;
  destroy: () => void;
}

export function createDiagramZoomController(
  viewportEl: HTMLDivElement,
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
      return (
        options.isEnabled() &&
        event.type !== 'dblclick' &&
        (!event.ctrlKey || event.type === 'wheel') &&
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

  return {
    reset: () => {
      behavior.transform(selection, zoomIdentity);
      options.onDraggingChange(false);
    },
    zoomBy: (multiplier: number) => {
      behavior.scaleBy(selection, multiplier);
    },
    destroy: () => {
      selection.on('.zoom', null);
      options.onDraggingChange(false);
    },
  };
}

function toDiagramTransform(transform: ZoomTransform): DiagramZoomTransform {
  return {
    scale: transform.k,
    offsetX: transform.x,
    offsetY: transform.y,
  };
}
