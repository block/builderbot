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
 * Puts focus somewhere sensible in the parent menu before a sheet-mode sub
 * closes. bits-ui's own close-auto-focus for sub-menus is suppressed, and the
 * root FocusScope only watches its own container for the focused node
 * disappearing — a sheet-mode sub is portalled elsewhere — so without this a
 * Back row drops focus to `body` and the parent sheet loses arrow-key
 * navigation.
 *
 * Where it lands depends on how the sub is being closed. Keyboard (the Back
 * row, or anything else in `content`, matches `:focus-visible`): the
 * sub-trigger that opened it, which bits-ui links by `aria-controls` while
 * the sub is open — the row the user came from. Touch or pointer (a Back tap,
 * a tap on the sub's scrim): the parent menu's content element instead.
 * Focusing the sub-trigger here would light it as `data-highlighted` via
 * bits-ui's item `onfocus`, and the sub-trigger styles `focus:` rather than
 * `focus-visible:`, so a sheet with no hover state would show a stuck
 * selected row until the next tap. From the content element arrow keys still
 * work: bits-ui's content `onkeydown` focuses the first or last item.
 */
export function returnFocusFromMenuSub(content: HTMLElement | null): void {
  if (!content?.id) return;
  const subTrigger = content.ownerDocument.querySelector<HTMLElement>(
    `[aria-controls="${CSS.escape(content.id)}"]`
  );
  if (!subTrigger) return;
  const keyboard =
    content.matches(':focus-visible') || content.querySelector(':focus-visible') !== null;
  if (keyboard) {
    subTrigger.focus();
    return;
  }
  subTrigger.closest<HTMLElement>('[role="menu"]')?.focus({ preventScroll: true });
}
