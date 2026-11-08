/**
 * When the app takes its mobile layout: one pane at a time, no sidebar,
 * full-screen dialogs that fit above the on-screen keyboard. A narrow
 * window, or a phone in either orientation: a coarse pointer on a screen
 * whose short side is no more than PHONE_SHORT_SIDE_PX. A phone held sideways
 * is 874 px wide (iPhone 17 Pro) but only 340 px tall, and its on-screen
 * keyboard leaves 48 px of it, which the desktop layout's centred dialogs
 * can't fit in.
 *
 * The phone test reads the screen rather than the window: iOS reports
 * `device-width` / `device-height` in portrait whatever the orientation,
 * Android in the current one, hence both. Neither changes with the keyboard,
 * which on Android shrinks the window itself (`interactive-widget=
 * resizes-content`), so a tablet in landscape keeps its layout while typing.
 * Tablets' short side starts at 744 px (iPad mini).
 *
 * CSS uses the same query: the `mobile:` variant (app.css) and the
 * components' `@media` rules for this layout. Keep them in step.
 */
export const MOBILE_LAYOUT_QUERY =
  '(max-width: 768px), (pointer: coarse) and (max-device-width: 500px), (pointer: coarse) and (max-device-height: 500px)';
/**
 * Below this height a full-screen dialog can't show its header and its last
 * row together: the on-screen keyboard up on a phone held sideways leaves
 * about 48px of it. See dialog-content's `cramped`.
 */
export const SHORT_VIEWPORT_PX = 240;
const SHORT_WINDOW_QUERY = `(max-height: ${SHORT_VIEWPORT_PX - 0.02}px)`;
const SPLIT_QUERY = '(min-width: 1000px)';
const COARSE_POINTER_QUERY = '(pointer: coarse)';
const NO_HOVER_QUERY = '(hover: none)';

export interface ViewportState {
  isMobile: boolean;
  canSplit: boolean;
  // Visual shortcut affordances only; not a hardware keyboard capability check.
  showShortcutHints: boolean;
  hasCoarsePointer: boolean;
  // Dropdown and context menus render as a bottom sheet (large touch rows)
  // instead of a floating popover: a phone-sized viewport driven by touch.
  // Both conditions, so a narrow desktop window keeps its popovers and a
  // tablet in landscape keeps menus anchored to what was pressed.
  sheetMenus: boolean;
  // The window itself is shorter than SHORT_VIEWPORT_PX: on Android the
  // on-screen keyboard shrinks it (`interactive-widget=resizes-content`), so
  // a phone held sideways is left this short. iOS leaves the window alone
  // and only the visual viewport shrinks (visualViewport.svelte.ts).
  shortWindow: boolean;
}

/** The state for the given media query answers. */
export function readViewport(matches: (query: string) => boolean): ViewportState {
  const isMobile = matches(MOBILE_LAYOUT_QUERY);
  const hasCoarsePointer = matches(COARSE_POINTER_QUERY);
  return {
    isMobile,
    canSplit: matches(SPLIT_QUERY),
    hasCoarsePointer,
    showShortcutHints: !(hasCoarsePointer || matches(NO_HOVER_QUERY)),
    sheetMenus: isMobile && hasCoarsePointer,
    shortWindow: matches(SHORT_WINDOW_QUERY),
  };
}

function matchesNow(query: string): boolean {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function'
    ? window.matchMedia(query).matches
    : false;
}

// Read at once rather than when the first screen subscribes: that happens
// as it mounts, after the first render, so a phone would paint the desktop
// layout first and then animate the sidebar away.
export const viewport = $state<ViewportState>(readViewport(matchesNow));

const QUERIES = [
  MOBILE_LAYOUT_QUERY,
  SPLIT_QUERY,
  COARSE_POINTER_QUERY,
  NO_HOVER_QUERY,
  SHORT_WINDOW_QUERY,
];
/** The live query lists, while anything is subscribed. */
let lists: Map<string, MediaQueryList> | null = null;
let subscriberCount = 0;

function syncViewport() {
  Object.assign(
    viewport,
    readViewport((query) => lists?.get(query)?.matches ?? matchesNow(query))
  );
}

export function watchViewport(): () => void {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return () => {};

  subscriberCount += 1;

  if (!lists) {
    lists = new Map(QUERIES.map((query) => [query, window.matchMedia(query)]));
    for (const list of lists.values()) list.addEventListener('change', syncViewport);
  }
  syncViewport();

  return () => {
    subscriberCount = Math.max(0, subscriberCount - 1);
    if (subscriberCount === 0 && lists) {
      for (const list of lists.values()) list.removeEventListener('change', syncViewport);
      lists = null;
    }
  };
}
