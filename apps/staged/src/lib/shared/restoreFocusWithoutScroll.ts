type Focusable = Element & { focus(options?: FocusOptions): void };

export interface FocusRestore {
  /** Call from the focus scope's `onOpenAutoFocus`, before anything takes focus. */
  capture(): void;
  /** Call from `onCloseAutoFocus`, after the caller's own handler. */
  restore(event: Event): void;
}

/**
 * Return focus to what had it before a dialog opened, without scrolling it
 * into view.
 *
 * bits-ui's focus scope does the same restore with a plain `focus()`, which
 * scrolls every scrollable ancestor of the trigger to show it — `<body>`
 * included, where iOS Safari left the shell parked 40 px up (issue 02).
 * `capture()` records the element the same way the scope does (the active
 * element when it mounts, unless that is `<body>`); `restore()` takes over
 * the close focus only while `enabled()` holds and the caller has not
 * prevented it, and otherwise leaves bits-ui's default alone.
 */
export function restoreFocusWithoutScroll(enabled: () => boolean): FocusRestore {
  let target: Focusable | null = null;
  return {
    capture() {
      const active = document.activeElement;
      target = active && active !== document.body ? (active as Focusable) : null;
    },
    restore(event) {
      const el = target;
      target = null;
      if (event.defaultPrevented || !enabled()) return;
      event.preventDefault();
      if (el?.isConnected) el.focus({ preventScroll: true });
    },
  };
}
