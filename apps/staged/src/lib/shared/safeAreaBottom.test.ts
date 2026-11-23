import { describe, expect, it } from 'vitest';

// The bottom safe-area inset has one definition: `--safe-bottom` on `:root`
// in app.css, the larger of the env() bottom inset and the inset
// viewportFit.ts leaves in `--safe-bottom-fallback` when it drops
// `viewport-fit=cover`. Without cover the env() inset reads 0. A style that
// reads it directly then sits under the home indicator, on the installs the
// guard acts on and nowhere else, so nothing shows the mistake in a browser.
//
// These tests read the source text and fail on a read of the env() inset or
// of the fallback outside that definition. They do not parse the files, so
// they match comments too: in prose, write "the env() bottom inset".

// index.html and everything under src but the tests, as text (Vite's raw
// import; vitest.config.ts lets it through for stylesheets). The diff viewer
// package is scanned too: its components render in the same document.
const sources = import.meta.glob(
  [
    '/index.html',
    '/src/**/*.{css,html,svelte,ts}',
    '!/src/**/*.test.ts',
    '../../../../../packages/diff-viewer/src/**/*.{css,html,svelte,ts}',
    '!../../../../../packages/diff-viewer/src/**/*.test.ts',
  ],
  { query: '?raw', import: 'default', eager: true }
) as Record<string, string>;

const APP_CSS = '/src/app.css';
// The guard's probe reads the inset itself, to save it before cover goes.
const VIEWPORT_FIT = '/src/lib/shared/viewportFit.ts';
// Files outside the app are keyed by their path from it: `../../packages/…`.
const DIFF_VIEWER_SRC = '/packages/diff-viewer/src/';

const DEFINITION = 'max(env(safe-area-inset-bottom), var(--safe-bottom-fallback, 0px))';

const ENV_BOTTOM = /env\(\s*safe-area-inset-bottom/g;
const FALLBACK_READ = /var\(\s*--safe-bottom-fallback/g;
const SAFE_BOTTOM_DECLARATION = /(?<![\w-])--safe-bottom\s*:/g;

/** `path:line` of each match of `pattern`, over every source. */
function matches(pattern: RegExp): string[] {
  const found: string[] = [];
  for (const [path, source] of Object.entries(sources)) {
    for (const match of source.matchAll(pattern)) {
      found.push(`${path}:${source.slice(0, match.index).split('\n').length}`);
    }
  }
  return found.sort();
}

const pathOf = (location: string) => location.slice(0, location.lastIndexOf(':'));

/** The matches outside `allowed`, and how many each allowed file has. */
function split(pattern: RegExp, allowed: string[]) {
  const all = matches(pattern);
  return {
    elsewhere: all.filter((location) => !allowed.includes(pathOf(location))),
    counts: allowed.map((path) => all.filter((location) => pathOf(location) === path).length),
  };
}

describe('the bottom safe-area inset', () => {
  it('is read from the source files', () => {
    expect(sources[APP_CSS]).toContain('--safe-bottom');
    expect(sources[VIEWPORT_FIT]).toContain('guardViewportFit');
    expect(sources['/index.html']).toContain('viewport-fit=cover');
    const diffViewer = Object.entries(sources).filter(([path]) => path.includes(DIFF_VIEWER_SRC));
    expect(diffViewer.length).toBeGreaterThan(10);
    const text = (name: string) => diffViewer.find(([path]) => path.endsWith(name))?.[1];
    expect(text('/DiffViewer.svelte')).toContain('<style>');
    expect(text('/search.css')).toContain('{');
    expect(matches(/var\(\s*--safe-bottom\s*[,)]/g).length).toBeGreaterThan(10);
  });

  it('is defined once, as --safe-bottom on :root in app.css', () => {
    expect(split(SAFE_BOTTOM_DECLARATION, [APP_CSS])).toEqual({ elsewhere: [], counts: [1] });
    const rule = /:root\s*\{\s*--safe-bottom:\s*([^;]+?)\s*;\s*\}/.exec(sources[APP_CSS]);
    expect(rule?.[1]).toBe(DEFINITION);
  });

  // In the two below, `elsewhere` lists each stray read as `path:line`. Use
  // `var(--safe-bottom)` there instead.

  it('has its env() inset read only by that definition and the guard’s probe', () => {
    expect(split(ENV_BOTTOM, [APP_CSS, VIEWPORT_FIT])).toEqual({ elsewhere: [], counts: [1, 1] });
  });

  it('has its fallback read only by that definition', () => {
    expect(split(FALLBACK_READ, [APP_CSS])).toEqual({ elsewhere: [], counts: [1] });
  });
});
