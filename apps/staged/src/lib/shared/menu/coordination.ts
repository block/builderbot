type CloseMenu = () => void;

const closeListeners = new Set<CloseMenu>();

export function registerMenuCloseListener(listener: CloseMenu): void {
  closeListeners.add(listener);
}

export function unregisterMenuCloseListener(listener: CloseMenu): void {
  closeListeners.delete(listener);
}

export function closeAllMenus(except?: CloseMenu): void {
  for (const listener of closeListeners) {
    if (listener !== except) {
      listener();
    }
  }
}
