/**
 * Persisted, resizable widths for the dialogs that carry a drag handle.
 *
 * Every mount site wraps its dialog in `{#if …}`, so the chosen width cannot
 * live in component scope — instances are cached per preference key at module
 * level and survive remounts. The default equals the minimum, which is the
 * width the dialog had before it became resizable, so a user who never drags
 * sees no change.
 *
 * The maximum follows the window: `set()` clamps against the live window size,
 * and the inline style also carries a viewport-relative `max-width` so a later
 * window shrink only caps rendering. The saved preference is untouched, and the
 * dialog returns to it when the window grows again.
 */

import { getStoreValue, setStoreValue } from '../../../shared/persistentStore';

/** Breathing room kept between the dialog and each window edge. */
export const DIALOG_VIEWPORT_GUTTER = 32;

export const NOTE_DIALOG_WIDTH_KEY = 'note-dialog-width';
export const SESSION_DIALOG_WIDTH_KEY = 'session-dialog-width';
export const NEW_SESSION_DIALOG_WIDTH_KEY = 'new-session-dialog-width';

export const NOTE_DIALOG_MIN_WIDTH = 700;
export const SESSION_DIALOG_MIN_WIDTH = 700;
export const NEW_SESSION_DIALOG_MIN_WIDTH = 580;

/**
 * Widest the dialog may be drawn right now. Never below `minWidth`: when the
 * window is narrower than the minimum, the CSS `max-width` caps rendering and
 * the stored width stays put, matching the pre-resize behaviour.
 */
export function dialogMaxWidth(minWidth: number): number {
  if (typeof window === 'undefined') return minWidth;
  return Math.max(minWidth, window.innerWidth - DIALOG_VIEWPORT_GUTTER * 2);
}

/** Inline style for a dialog of `width` px, capped to the viewport. */
export function dialogWidthStyle(width: number): string {
  return `width:${width}px;max-width:calc(100vw - ${DIALOG_VIEWPORT_GUTTER * 2}px);`;
}

export interface DialogWidth {
  readonly key: string;
  readonly minWidth: number;
  /** Stored width in px, clamped to `[minWidth, dialogMaxWidth(minWidth)]`. */
  readonly width: number;
  readonly hydrated: boolean;
  readonly maxWidth: number;
  /** `width` as an inline style, ready for `Dialog.Content`'s `style` prop. */
  readonly style: string;
  /** Clamp and apply a new width, persisting it unless `persist` is false. */
  set(width: number, persist?: boolean): void;
  /** Return to the default (the minimum) and persist that. */
  reset(): void;
  /** Read the saved width once per app run; safe to call on every mount. */
  ensureHydrated(): Promise<void>;
}

const instances = new Map<string, DialogWidth>();

/**
 * Reactive width for the dialog stored under `key`. Repeat calls with the same
 * key return the same instance, so a dialog that remounts (or two dialogs that
 * deliberately share a width, like the note viewer and note editor) stay in
 * sync.
 */
export function createDialogWidth(options: { key: string; minWidth: number }): DialogWidth {
  const { key, minWidth } = options;

  const cached = instances.get(key);
  if (cached) return cached;

  const inner = $state({ width: minWidth, hydrated: false });
  let hydration: Promise<void> | null = null;

  function clamp(width: number): number {
    if (!Number.isFinite(width)) return inner.width;
    return Math.max(minWidth, Math.min(dialogMaxWidth(minWidth), Math.round(width)));
  }

  async function hydrate(): Promise<void> {
    const saved = await getStoreValue<number>(key);
    if (typeof saved === 'number' && Number.isFinite(saved)) {
      inner.width = clamp(saved);
    }
    inner.hydrated = true;
  }

  const instance: DialogWidth = {
    key,
    minWidth,
    get width() {
      return inner.width;
    },
    get hydrated() {
      return inner.hydrated;
    },
    get maxWidth() {
      return dialogMaxWidth(minWidth);
    },
    get style() {
      return dialogWidthStyle(inner.width);
    },
    set(width: number, persist = true) {
      const clamped = clamp(width);
      if (inner.width !== clamped) {
        inner.width = clamped;
      }
      if (persist) {
        void setStoreValue(key, clamped);
      }
    },
    reset() {
      instance.set(minWidth);
    },
    ensureHydrated() {
      hydration ??= hydrate();
      return hydration;
    },
  };

  instances.set(key, instance);
  return instance;
}

/**
 * Read every saved dialog width at startup so the first open of an app run
 * renders at the user's width instead of jumping there from the minimum.
 */
export async function hydrateDialogWidths(): Promise<void> {
  await Promise.all(
    [
      { key: NOTE_DIALOG_WIDTH_KEY, minWidth: NOTE_DIALOG_MIN_WIDTH },
      { key: SESSION_DIALOG_WIDTH_KEY, minWidth: SESSION_DIALOG_MIN_WIDTH },
      { key: NEW_SESSION_DIALOG_WIDTH_KEY, minWidth: NEW_SESSION_DIALOG_MIN_WIDTH },
    ].map((options) => createDialogWidth(options).ensureHydrated())
  );
}
