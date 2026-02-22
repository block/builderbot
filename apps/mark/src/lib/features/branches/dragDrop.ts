/**
 * Shared drag-drop service for BranchCards.
 *
 * Registers a single global Tauri `onDragDropEvent` listener instead of one
 * per BranchCard. Each card subscribes with its DOM element and callbacks;
 * the service hit-tests the drag position against all registered cards and
 * dispatches to the correct one.
 *
 * This eliminates the O(N) listener storm that caused UI freezes when
 * multiple branch cards were rendered.
 */

import type { UnlistenFn } from '@tauri-apps/api/event';

export type DragDropSubscription = {
  /** The card's root DOM element, used for hit-testing. */
  element: HTMLElement;
  /** Called when the drag enters/leaves this card's bounds. */
  onDragOver: (over: boolean) => void;
  /** Called when files are dropped on this card. */
  onDrop: (paths: string[]) => void;
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
    // Find which subscriber the cursor is over
    let found: DragDropSubscription | null = null;
    for (const sub of subscribers) {
      if (isPositionOverElement(sub.element, x, y)) {
        found = sub;
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
    // Find the drop target
    for (const sub of subscribers) {
      if (isPositionOverElement(sub.element, x, y)) {
        sub.onDrop(paths ?? []);
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
 * Subscribe a BranchCard to drag-drop events.
 *
 * The global listener is lazily created on the first subscription and
 * torn down when the last subscriber unsubscribes.
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
