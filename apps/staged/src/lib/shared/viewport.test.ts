import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The module's `$state` rune compiles away in the app build; under vitest it
// is a plain global call, so stub it as identity (preferences.test.ts
// precedent). `window.matchMedia` answers from `matching`.

const PHONE_LAYOUT =
  '(max-width: 768px), (pointer: coarse) and (max-device-width: 500px), (pointer: coarse) and (max-device-height: 500px)';

let matching: Set<string>;
let listeners: Map<string, Set<() => void>>;

async function importModule() {
  return await import('./viewport.svelte');
}

beforeEach(() => {
  vi.resetModules();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  matching = new Set();
  listeners = new Map();
  vi.stubGlobal('window', {
    matchMedia: (query: string) => ({
      get matches() {
        return matching.has(query);
      },
      addEventListener: (_type: string, cb: () => void) => {
        if (!listeners.has(query)) listeners.set(query, new Set());
        listeners.get(query)!.add(cb);
      },
      removeEventListener: (_type: string, cb: () => void) => listeners.get(query)?.delete(cb),
    }),
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('viewport', () => {
  it('reads the media queries as the module loads, before anything subscribes', async () => {
    // The first render of a phone must already be the mobile layout.
    matching = new Set([PHONE_LAYOUT, '(pointer: coarse)', '(hover: none)']);

    const { MOBILE_LAYOUT_QUERY, viewport } = await importModule();
    expect(MOBILE_LAYOUT_QUERY).toBe(PHONE_LAYOUT);

    expect(viewport).toEqual({
      isMobile: true,
      canSplit: false,
      hasCoarsePointer: true,
      showShortcutHints: false,
      sheetMenus: true,
    });
  });

  it('starts at the desktop layout without matchMedia (node, jsdom)', async () => {
    vi.stubGlobal('window', {});
    const { viewport, watchViewport } = await importModule();
    expect(viewport.isMobile).toBe(false);
    expect(viewport.showShortcutHints).toBe(true);
    watchViewport()();
  });

  it('follows changes while subscribed, and stops listening after the last unsubscribe', async () => {
    const { MOBILE_LAYOUT_QUERY, viewport, watchViewport } = await importModule();
    const stopA = watchViewport();
    const stopB = watchViewport();
    expect(viewport.isMobile).toBe(false);

    // A phone rotated, or a window narrowed past the breakpoint.
    matching.add(MOBILE_LAYOUT_QUERY);
    for (const cb of listeners.get(MOBILE_LAYOUT_QUERY) ?? []) cb();
    expect(viewport.isMobile).toBe(true);
    expect(viewport.sheetMenus).toBe(false);

    stopA();
    expect(listeners.get(MOBILE_LAYOUT_QUERY)?.size).toBe(1);
    stopB();
    expect(listeners.get(MOBILE_LAYOUT_QUERY)?.size).toBe(0);
  });
});

// Every stylesheet and component, as text (Vite's raw import).
const styleSources = import.meta.glob('/src/**/*.{svelte,css}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

describe('the mobile layout in CSS', () => {
  it('uses the same query as MOBILE_LAYOUT_QUERY everywhere', async () => {
    // app.css's `mobile:` variant and the components' own @media rules for
    // the layout must switch together with `viewport.isMobile`.
    const { MOBILE_LAYOUT_QUERY } = await importModule();

    const uses: string[] = [];
    for (const [file, source] of Object.entries(styleSources)) {
      for (const match of source.matchAll(/@media ([^{]*\(max-width: 768px\)[^{]*)\{/g)) {
        uses.push(match[1].replace(/\s+/g, ' ').trim());
      }
      expect(source, file).not.toMatch(/max-\[768px\]:/);
    }

    // A silently empty glob would pass with nothing checked.
    expect(Object.keys(styleSources)).toContain('/src/app.css');
    expect(uses.length).toBeGreaterThan(5);
    for (const query of uses) expect(query).toBe(MOBILE_LAYOUT_QUERY);
  });
});
