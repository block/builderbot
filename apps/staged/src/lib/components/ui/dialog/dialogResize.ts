export interface ResizeGeometry {
  width: number;
  min: number;
  max: number;
}

export function resizeBounds(minWidth: number, availableWidth: number) {
  const max = Math.max(0, Math.round(availableWidth));
  return { min: Math.min(minWidth, max), max };
}

function clamp(width: number, bounds: { min: number; max: number }) {
  return Math.max(bounds.min, Math.min(bounds.max, Math.round(width)));
}

type ResizePointer = Pick<PointerEvent, 'pointerId' | 'clientX'>;

/** Gesture state is local; only a changed final width becomes a preference. */
export function createDialogResize(callbacks: {
  geometry: () => ResizeGeometry;
  preview: (width: number) => void;
  commit: (width: number) => void;
  end: () => void;
}) {
  let gesture: {
    pointerId: number;
    startX: number;
    startWidth: number;
    candidate: number;
    bounds: ResizeGeometry;
  } | null = null;

  function move(event: ResizePointer) {
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    // Pointer-up can arrive before the resize event or reactive layout cleanup.
    const bounds = callbacks.geometry();
    if (bounds.min !== gesture.bounds.min || bounds.max !== gesture.bounds.max) {
      finish(false);
      return;
    }
    const next = clamp(gesture.startWidth + 2 * (event.clientX - gesture.startX), gesture.bounds);
    if (next !== gesture.candidate) {
      gesture.candidate = next;
      callbacks.preview(next);
    }
  }

  function finish(commit: boolean) {
    if (!gesture) return;
    const { candidate, startWidth } = gesture;
    gesture = null;
    try {
      if (commit && candidate !== startWidth) callbacks.commit(candidate);
    } finally {
      callbacks.end();
    }
  }

  return {
    start(event: ResizePointer) {
      if (gesture) return false;
      const bounds = callbacks.geometry();
      const startWidth = clamp(bounds.width, bounds);
      gesture = {
        pointerId: event.pointerId,
        startX: event.clientX,
        startWidth,
        candidate: startWidth,
        bounds,
      };
      // Claim the preview even for a click so a delayed read cannot overwrite it.
      callbacks.preview(startWidth);
      return true;
    },
    move,
    release(event: ResizePointer) {
      if (gesture?.pointerId !== event.pointerId) return;
      move(event);
      finish(true);
    },
    cancel(pointerId?: number) {
      if (pointerId !== undefined && gesture?.pointerId !== pointerId) return;
      finish(false);
    },
    key(key: string) {
      const bounds = callbacks.geometry();
      const width = clamp(bounds.width, bounds);
      const widths: Record<string, number> = {
        ArrowLeft: width - 16,
        ArrowRight: width + 16,
        Home: bounds.min,
        End: bounds.max,
      };
      const requested = widths[key];
      if (requested === undefined) return false;
      if (gesture) return true;
      const next = clamp(requested, bounds);
      if (next !== width) callbacks.commit(next);
      return true;
    },
  };
}
