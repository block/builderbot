import { getContext, setContext } from 'svelte';

/**
 * Lets a sheet-mode menu close itself from inside its own content — the
 * scrim of either sheet, and a sub-sheet's Back row. bits-ui closes a menu
 * from its items, close keys, outside taps and focus moves, and does not
 * expose the open state to the content, so the `Root` and `Sub` wrappers
 * each publish a handle over their bindable `open`.
 */
export interface MenuCloseHandle {
  close(): void;
}

const ROOT_KEY = Symbol('menu-root');
const SUB_KEY = Symbol('menu-sub');

export function setMenuRoot(handle: MenuCloseHandle): void {
  setContext(ROOT_KEY, handle);
}

export function getMenuRoot(): MenuCloseHandle | undefined {
  return getContext<MenuCloseHandle | undefined>(ROOT_KEY);
}

export function setMenuSub(handle: MenuCloseHandle): void {
  setContext(SUB_KEY, handle);
}

export function getMenuSub(): MenuCloseHandle | undefined {
  return getContext<MenuCloseHandle | undefined>(SUB_KEY);
}

/**
 * Hands focus back to the sub-trigger that opened `content` before the sub
 * closes. bits-ui's own close-auto-focus for sub-menus is suppressed, and the
 * root FocusScope only watches its own container for the focused node
 * disappearing — a sheet-mode sub is portalled elsewhere — so without this a
 * Back row reached with the keyboard drops focus to `body` and the parent
 * sheet loses arrow-key navigation. bits-ui links the two with
 * `aria-controls` while the sub is open.
 */
export function focusMenuSubTrigger(content: HTMLElement | null): void {
  if (!content?.id) return;
  content.ownerDocument
    .querySelector<HTMLElement>(`[aria-controls="${CSS.escape(content.id)}"]`)
    ?.focus();
}
