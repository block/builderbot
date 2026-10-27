import { getContext, setContext } from 'svelte';

/**
 * Lets a sheet-mode sub-menu close itself from inside its own content (the
 * Back row). bits-ui only closes a sub-menu from close keys, outside taps and
 * focus moves, and does not expose the sub's open state to its content, so
 * the `Sub` wrapper publishes a handle over its bindable `open`.
 */
export interface MenuSubHandle {
  close(): void;
}

const KEY = Symbol('menu-sub');

export function setMenuSub(handle: MenuSubHandle): void {
  setContext(KEY, handle);
}

export function getMenuSub(): MenuSubHandle | undefined {
  return getContext<MenuSubHandle | undefined>(KEY);
}
