// viewportFit.ts — drop `viewport-fit=cover` where iOS lays the page out short.
//
// A home-screen app keeps the status-bar style it had when it was added:
// iOS bakes it into the web clip, and the page cannot read it. Apps added
// while index.html asked for `black-translucent` keep that style. With
// `viewport-fit=cover`, iOS 27 draws such a page from the top of the screen,
// under the status bar, but keeps the layout viewport the height below the
// status bar (812 of 874 pt). The page then stops 62 pt short of the bottom
// of the screen: a `bottom: 0` scrim leaves that strip undimmed, and the
// bottom inset sits sheets and the composer 62 pt above the home indicator.
//
// Without cover, the same app is laid out below the status bar down to the
// bottom of the screen, with no insets, as it was before cover was added. So
// where the page is drawn under the status bar yet ends above the bottom of
// the window, cover is dropped. An app added since (status bar `default`) has
// no top inset and keeps cover, and so does a black-translucent app wherever
// iOS lays it out to the bottom edge.
//
// Dropping cover costs every `env()` inset: they all read 0 afterwards, for
// both kinds of install, and without cover the two cannot be told apart. So
// the guard is careful in four ways.
//
// - It acts on a settled reading only. iOS hands the page its insets a moment
//   after launch, and the status bar comes and goes with rotation. A reading
//   counts once it has held for SETTLE_MS since the guard last heard of a
//   change, and is read again then, so one passing value, mid-rotation say,
//   changes nothing. The cost falls on old installs: their first frames are
//   drawn under the status bar and shift 62 px when cover goes, SETTLE_MS
//   after the insets arrive.
// - It does not count on `resize` to hear of a change. On a cold launch in
//   the iOS 27.2 simulator a black-translucent app is handed its insets, and
//   its layout viewport goes from 874 to 812 pt, 140-175 ms after the first
//   script runs and most often with no `resize` at all. (A default app got
//   one each time.) So the guard also watches the size of two hidden
//   elements, one padded by the insets and one as tall as the layout
//   viewport. That is an observer, not a timer: it runs when one of them
//   changes size, and nothing runs in between.
// - It keeps watching. Once the layout reaches the bottom of the window with
//   cover dropped (an iPhone in landscape has no status bar), cover is put
//   back, on a settled reading too, and judged afresh from there.
// - It keeps the home indicator clear. The bottom inset read just before
//   cover goes is left on <html> as `--safe-bottom-fallback`. `--safe-bottom`
//   in app.css is the larger of that and the `env()` inset, and is what every
//   bottom-padded pane reads. The fallback is removed when cover returns.

/** How long a reading must hold, with no change the guard hears of, before it acts on it. */
export const SETTLE_MS = 300;

/** Set on <html> while cover is dropped: the bottom inset cover last reported. */
export const SAFE_BOTTOM_FALLBACK = '--safe-bottom-fallback';

export interface ViewportFitSample {
  /** Running as a home-screen app. */
  standalone: boolean;
  /** `env(safe-area-inset-top)`, px: the page is drawn under the status bar. */
  safeAreaTop: number;
  /**
   * The layout viewport's height, `document.documentElement.clientHeight`.
   * iOS leaves it alone when the keyboard opens.
   */
  layoutHeight: number;
  /** `window.outerHeight`: the app's window. */
  windowHeight: number;
}

/** The page is drawn under the status bar but ends above the window's bottom. */
export function coverLaysOutShort(sample: ViewportFitSample): boolean {
  return sample.standalone && sample.safeAreaTop > 0 && sample.layoutHeight < sample.windowHeight;
}

export type ViewportFitChange = 'drop' | 'restore';

/**
 * What a reading asks for: drop cover where it lays the page out short, put
 * it back where the page reaches the window's bottom without it.
 */
export function viewportFitChange(
  covered: boolean,
  sample: ViewportFitSample
): ViewportFitChange | null {
  if (!sample.standalone) return null;
  if (covered) return coverLaysOutShort(sample) ? 'drop' : null;
  return sample.layoutHeight >= sample.windowHeight ? 'restore' : null;
}

/** A viewport meta `content` without its `viewport-fit` key. */
export function withoutViewportFit(content: string): string {
  return content
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part && !/^viewport-fit\s*=/i.test(part))
    .join(', ');
}

/** A viewport meta `content` with `viewport-fit=cover` as its last key. */
export function withViewportFitCover(content: string): string {
  return [withoutViewportFit(content), 'viewport-fit=cover'].filter(Boolean).join(', ');
}

function isStandalone(): boolean {
  return (
    window.matchMedia('(display-mode: standalone)').matches ||
    (navigator as Navigator & { standalone?: boolean }).standalone === true
  );
}

/**
 * Keep `viewport-fit=cover` off the viewport meta while it lays the page out
 * short, and on otherwise (see above). A no-op outside a home-screen app.
 * Returns the cleanup, which leaves the meta as it then stands.
 */
export function guardViewportFit(): () => void {
  if (typeof window === 'undefined' || !isStandalone()) return () => {};
  const meta = document.querySelector<HTMLMetaElement>('meta[name="viewport"]');
  if (!meta || !/viewport-fit\s*=\s*cover/i.test(meta.content)) return () => {};
  const root = document.documentElement;

  // Two hidden elements for the guard's lifetime, outside <body>, so no check
  // inserts a node next to the app. `probe` is padded by the top and bottom
  // insets, which is how they are read. It is also as wide as the top inset,
  // so its box changes size when either inset does, even if their sum does
  // not. `span` runs from the top of the layout viewport to its bottom.
  const hidden = 'position:fixed;visibility:hidden;pointer-events:none;';
  const probe = document.createElement('div');
  probe.style.cssText =
    hidden +
    'width:env(safe-area-inset-top);' +
    'padding-top:env(safe-area-inset-top);padding-bottom:env(safe-area-inset-bottom)';
  const span = document.createElement('div');
  span.style.cssText = hidden + 'top:0;bottom:0;width:0';
  root.appendChild(probe);
  root.appendChild(span);

  let covered = true;
  let settle: ReturnType<typeof setTimeout> | undefined;
  // Window sizes, so a pair of layouts that each ask for the other cannot
  // flip the meta for ever. `restoredAt` is the size at which cover was last
  // put back. Every drop sets `keepOffAt`: to the window's size if that is
  // where cover was last put back, to nothing otherwise. Cover is not put
  // back at the `keepOffAt` size.
  //
  // The window changing size does not clear `keepOffAt`. Only the next drop
  // does, and for one to happen cover must first come back at another size.
  // So cover stays off at that size through a rotation away and back, for
  // the rest of the session if it comes back nowhere else. That errs towards
  // no cover: at that size the page reaches the bottom of the window without
  // it, and the fallback inset stays. Where cover does come back at another
  // size, the drop on returning clears `keepOffAt`, and this size gets one
  // more restore and drop before cover is kept off here again.
  let restoredAt: string | null = null;
  let keepOffAt: string | null = null;

  const windowSize = () => `${window.outerWidth}x${window.outerHeight}`;

  const wanted = (): ViewportFitChange | null => {
    const change = viewportFitChange(covered, {
      standalone: true,
      safeAreaTop: parseFloat(getComputedStyle(probe).paddingTop) || 0,
      layoutHeight: root.clientHeight,
      windowHeight: window.outerHeight,
    });
    return change === 'restore' && windowSize() === keepOffAt ? null : change;
  };

  const apply = (change: ViewportFitChange) => {
    if (change === 'drop') {
      // Read before cover goes: afterwards it is 0.
      const bottom = parseFloat(getComputedStyle(probe).paddingBottom) || 0;
      if (bottom > 0) root.style.setProperty(SAFE_BOTTOM_FALLBACK, `${bottom}px`);
      meta.content = withoutViewportFit(meta.content);
      covered = false;
      keepOffAt = windowSize() === restoredAt ? restoredAt : null;
    } else {
      meta.content = withViewportFitCover(meta.content);
      root.style.removeProperty(SAFE_BOTTOM_FALLBACK);
      covered = true;
      restoredAt = windowSize();
    }
  };

  // A reading is acted on only if it still stands SETTLE_MS later; a change
  // the guard hears of in between starts over from what the page reads then.
  const check = () => {
    clearTimeout(settle);
    settle = undefined;
    const change = wanted();
    if (!change) return;
    settle = setTimeout(() => {
      settle = undefined;
      if (wanted() === change) apply(change);
    }, SETTLE_MS);
  };

  // What the decision reads, and what tells the guard it may have changed:
  // the insets and the layout viewport's height, by the two elements' sizes,
  // and the window's size, by `resize`. Each is one more call to `check`, so
  // a change with no `resize` gets the same settle time as one with it, and
  // restarts the settle time of a reading already waiting.
  const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(check) : undefined;
  observer?.observe(probe, { box: 'border-box' });
  observer?.observe(span, { box: 'border-box' });
  window.addEventListener('resize', check);
  check();
  return () => {
    clearTimeout(settle);
    observer?.disconnect();
    window.removeEventListener('resize', check);
    probe.remove();
    span.remove();
  };
}
