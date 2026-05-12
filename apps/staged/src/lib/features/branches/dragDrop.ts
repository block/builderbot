/**
 * Shared drag-drop service for file drops via Tauri native events.
 *
 * Registers a single global Tauri `onDragDropEvent` listener instead of one
 * per component. Each subscriber registers its DOM element and callbacks;
 * the service hit-tests the drag position against all registered elements
 * and dispatches to the topmost match (last registered wins when elements
 * overlap, e.g. a modal over a branch card).
 *
 * This eliminates the O(N) listener storm that caused UI freezes when
 * multiple branch cards were rendered.
 */

import type { UnlistenFn } from '../../transport';
import { isTauri } from '../../transport';

export type DragDropSubscription = {
  /** The card's root DOM element, used for hit-testing. */
  element: HTMLElement;
  /** Called when the drag enters/leaves this card's bounds. */
  onDragOver: (over: boolean) => void;
  /** Called when files are dropped on this card. */
  onDrop: (paths: string[]) => void;
  /**
   * When true, this subscriber blocks all earlier subscribers from receiving
   * events — even at positions outside this element's bounds. Use this for
   * modal dialogs whose backdrop overlay covers the entire viewport.
   */
  blocking?: boolean;
};

let subscribers: DragDropSubscription[] = [];
let globalUnlisten: UnlistenFn | null = null;
let initPromise: Promise<void> | null = null;

/** Check if a logical position is within an element's bounding rect. */
function isPositionOverElement(el: HTMLElement, x: number, y: number): boolean {
  const rect = el.getBoundingClientRect();
  // Tauri on macOS gives logical coordinates that match getBoundingClientRect.
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

/** The currently hovered subscriber (tracked to avoid redundant callbacks). */
let currentHover: DragDropSubscription | null = null;

function handleEvent(type: string, x: number, y: number, paths?: string[]) {
  if (type === 'enter' || type === 'over') {
    // Find which subscriber the cursor is over.
    // Iterate in reverse so that later subscribers (e.g. modals layered on
    // top of branch cards) take priority over earlier ones at the same
    // coordinates.
    let found: DragDropSubscription | null = null;
    for (let i = subscribers.length - 1; i >= 0; i--) {
      if (isPositionOverElement(subscribers[i].element, x, y)) {
        found = subscribers[i];
        break;
      }
      // A blocking subscriber (e.g. a modal with a backdrop) prevents events
      // from reaching any earlier subscribers, even outside its own bounds.
      if (subscribers[i].blocking) {
        break;
      }
    }

    if (found !== currentHover) {
      // Left the previous card
      if (currentHover) {
        currentHover.onDragOver(false);
      }
      // Entered a new card
      if (found) {
        found.onDragOver(true);
      }
      currentHover = found;
    }
  } else if (type === 'drop') {
    // Find the drop target (reverse order — prefer topmost element)
    for (let i = subscribers.length - 1; i >= 0; i--) {
      if (isPositionOverElement(subscribers[i].element, x, y)) {
        subscribers[i].onDrop(paths ?? []);
        break;
      }
      if (subscribers[i].blocking) {
        break;
      }
    }
    // Clear hover state
    if (currentHover) {
      currentHover.onDragOver(false);
      currentHover = null;
    }
  } else if (type === 'leave') {
    if (currentHover) {
      currentHover.onDragOver(false);
      currentHover = null;
    }
  }
}

function ensureGlobalListener(): Promise<void> {
  if (initPromise) return initPromise;

  if (!isTauri) {
    // Native drag-drop is a Tauri-only feature; no-op in web mode
    initPromise = Promise.resolve();
    return initPromise;
  }

  initPromise = import('@tauri-apps/api/webview').then(({ getCurrentWebview }) => {
    return getCurrentWebview()
      .onDragDropEvent((event) => {
        const { type } = event.payload;
        if (type === 'enter' || type === 'over') {
          const { x, y } = event.payload.position;
          handleEvent(type, x, y);
        } else if (type === 'drop') {
          const { x, y } = event.payload.position;
          handleEvent('drop', x, y, event.payload.paths);
        } else if (type === 'leave') {
          handleEvent('leave', 0, 0);
        }
      })
      .then((unlisten) => {
        globalUnlisten = unlisten;
      });
  });

  return initPromise;
}

/**
 * Subscribe a component to drag-drop events.
 *
 * The global listener is lazily created on the first subscription and
 * torn down when the last subscriber unsubscribes. Later subscribers
 * take priority when elements overlap (e.g. modals over cards).
 *
 * Returns an unsubscribe function.
 */
export function subscribeDragDrop(sub: DragDropSubscription): () => void {
  subscribers.push(sub);
  ensureGlobalListener();

  return () => {
    subscribers = subscribers.filter((s) => s !== sub);
    if (currentHover === sub) {
      currentHover = null;
    }

    // Tear down the global listener when no subscribers remain
    if (subscribers.length === 0 && globalUnlisten) {
      globalUnlisten();
      globalUnlisten = null;
      initPromise = null;
    }
  };
}
