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
// the window, cover is dropped for the session. An app added since (status
// bar `default`) has no top inset and keeps cover, and so does a
// black-translucent app wherever iOS lays it out to the bottom edge.
//
// iOS hands the page its safe-area insets a moment after launch, with a
// `resize`: on the simulator, 70-140 ms after the first script runs and after
// `load`. The status bar also comes and goes with rotation, so the check runs
// on every `resize` until cover is gone. Until the insets arrive an old
// install is drawn under the status bar, but only as a blank page: on cold
// launches the app first rendered after the insets arrived, or in the same
// frame, so the TopBar never showed under the clock. Should the app ever
// render earlier, its first frames would sit under the status bar and shift
// 62 px when cover is dropped.

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

/** A viewport meta `content` without its `viewport-fit` key. */
export function withoutViewportFit(content: string): string {
  return content
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part && !/^viewport-fit\s*=/i.test(part))
    .join(', ');
}

function isStandalone(): boolean {
  return (
    window.matchMedia('(display-mode: standalone)').matches ||
    (navigator as Navigator & { standalone?: boolean }).standalone === true
  );
}

/**
 * Drop `viewport-fit=cover` from the viewport meta once the page is laid out
 * short (see above). A no-op outside a home-screen app. Returns the cleanup.
 */
export function guardViewportFit(): () => void {
  if (typeof window === 'undefined' || !isStandalone()) return () => {};
  const meta = document.querySelector<HTMLMetaElement>('meta[name="viewport"]');
  if (!meta || !/viewport-fit\s*=\s*cover/i.test(meta.content)) return () => {};

  // One hidden probe for the guard's lifetime, outside <body>, so no check
  // inserts a node next to the app.
  const probe = document.createElement('div');
  probe.style.cssText =
    'position:fixed;visibility:hidden;pointer-events:none;padding-top:env(safe-area-inset-top)';
  document.documentElement.appendChild(probe);

  const check = () => {
    const short = coverLaysOutShort({
      standalone: true,
      safeAreaTop: parseFloat(getComputedStyle(probe).paddingTop) || 0,
      layoutHeight: document.documentElement.clientHeight,
      windowHeight: window.outerHeight,
    });
    if (!short) return;
    meta.content = withoutViewportFit(meta.content);
    stop();
  };
  const stop = () => {
    window.removeEventListener('resize', check);
    probe.remove();
  };
  window.addEventListener('resize', check);
  check();
  return stop;
}
