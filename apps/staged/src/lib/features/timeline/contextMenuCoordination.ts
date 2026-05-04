/**
 * Module-level close-all signal for TimelineContextMenu instances.
 *
 * Multiple TimelineContextMenu instances exist (one per BranchTimeline,
 * one in ProjectSection). Because TimelineRow stops propagation on
 * contextmenu events, a right-click in one timeline can't reach the
 * window handler of another instance's menu. This Set of callbacks lets
 * any instance broadcast "close" to all others before opening itself.
 *
 * This must live at module scope (not inside a component function) so that
 * all instances share the same Set.
 */
const closeAllListeners = new Set<() => void>();

export function registerCloseListener(fn: () => void): void {
  closeAllListeners.add(fn);
}

export function unregisterCloseListener(fn: () => void): void {
  closeAllListeners.delete(fn);
}

export function broadcastCloseAll(): void {
  for (const fn of closeAllListeners) {
    fn();
  }
}
