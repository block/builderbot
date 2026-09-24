/**
 * Persisted, resizable widths for the dialogs that carry a drag handle.
 *
 * Every mount site wraps its dialog in `{#if …}`, so the chosen width cannot
 * live in component scope — instances are cached per preference key at module
 * level and survive remounts. The default equals the minimum, which is the
 * width the dialog had before it became resizable, so a user who never drags
 * sees no change.
 *
 * Preferred widths are independent of the viewport. CSS caps rendering and the
 * resize handle constrains gestures to the available space. Previews never
 * replace the preference, so cancelling a gesture restores it without a write.
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

/** Inline style for a dialog of `width` px, capped to the viewport. */
export function dialogWidthStyle(width: number): string {
  return `width:${width}px;max-width:calc(100vw - ${DIALOG_VIEWPORT_GUTTER * 2}px);`;
}

export interface DialogWidth {
  readonly key: string;
  readonly minWidth: number;
  /** Preview or preferred width in px, independent of the viewport cap. */
  readonly width: number;
  readonly hydrated: boolean;
  /** `width` as an inline style, ready for `Dialog.Content`'s `style` prop. */
  readonly style: string;
  /** Apply a preferred width, or a temporary preview when `persist` is false. */
  set(width: number, persist?: boolean): void;
  /** Discard the preview without changing or persisting the preferred width. */
  clearPreview(): void;
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

  const inner = $state({ width: minWidth, preview: null as number | null, hydrated: false });
  let hydration: Promise<void> | null = null;
  let interacted = false;

  function normalize(width: number): number {
    return Math.max(minWidth, Math.round(width));
  }

  async function hydrate(): Promise<void> {
    try {
      const saved = await getStoreValue<number>(key);
      if (!interacted && typeof saved === 'number' && Number.isFinite(saved)) {
        inner.width = normalize(saved);
      }
    } catch (error) {
      console.error(`[DialogWidth] Failed to read ${key}:`, error);
    } finally {
      inner.hydrated = true;
    }
  }

  const instance: DialogWidth = {
    key,
    minWidth,
    get width() {
      return inner.preview ?? inner.width;
    },
    get hydrated() {
      return inner.hydrated;
    },
    get style() {
      return dialogWidthStyle(instance.width);
    },
    set(width: number, persist = true) {
      if (!Number.isFinite(width)) return;
      interacted = true;
      const normalized = normalize(width);
      if (persist) {
        inner.width = normalized;
        inner.preview = null;
        void setStoreValue(key, normalized).catch((error) => {
          console.error(`[DialogWidth] Failed to write ${key}:`, error);
        });
      } else {
        inner.preview = normalized;
      }
    },
    clearPreview() {
      inner.preview = null;
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
 * Start reading saved widths without blocking startup. A dialog opened before
 * hydration settles may still change width when its preference arrives.
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
