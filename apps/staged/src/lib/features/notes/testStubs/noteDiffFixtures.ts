/**
 * Two versions of a note for the note diff's component tests, and the
 * layout jsdom does not do.
 *
 * Between the versions one paragraph was removed, one reworded and one
 * added, each with an unchanged block either side: three hunks, in that
 * order, one of each kind.
 */

import { extractNoteDiffUnits } from '../noteDiffUnits';

export const PREVIOUS_NOTE = [
  '# Cache design',
  '',
  'Nothing is written to disk.',
  '',
  'Logs are kept for a week.',
  '',
  'See the design doc for more.',
  '',
  'The cache is warmed when the app starts.',
  '',
  'Entries are evicted oldest first.',
].join('\n');

export const CURRENT_NOTE = [
  '# Cache design',
  '',
  'Nothing is written to disk.',
  '',
  'See the design doc for more.',
  '',
  'The cache is filled when the app starts.',
  '',
  'Entries are evicted oldest first.',
  '',
  'A hundred entries go at a time.',
].join('\n');

/** The kinds of the three hunks, in order. */
export const HUNK_KINDS = ['removed', 'modified', 'added'];

/** Each block's height in the layout `layOut` gives a version. */
export const BLOCK_HEIGHT = 100;

/**
 * Give the blocks under `content` a layout: stacked from the top of
 * `scroller`, each `BLOCK_HEIGHT` tall, as the offsets `measureNoteDiffUnits`
 * reads. `viewport` is how much of them `scroller` shows.
 */
export function layOut(content: Element, scroller: HTMLElement, viewport: number): void {
  const units = extractNoteDiffUnits(content);
  units.forEach(({ element }, index) => {
    Object.defineProperties(element, {
      offsetTop: { configurable: true, value: index * BLOCK_HEIGHT },
      offsetHeight: { configurable: true, value: BLOCK_HEIGHT },
      offsetParent: { configurable: true, value: scroller },
    });
  });
  Object.defineProperties(scroller, {
    clientHeight: { configurable: true, value: viewport },
    scrollHeight: { configurable: true, value: Math.max(viewport, units.length * BLOCK_HEIGHT) },
  });
}

/** The `Highlight` a test registry holds. */
export class FakeHighlight {
  ranges: Range[];
  constructor(...ranges: Range[]) {
    this.ranges = ranges;
  }
}

/** The text painted under `name` in a stubbed `CSS.highlights`. */
export function paintedText(registry: Map<string, FakeHighlight>, name: string): string[] {
  return (registry.get(name)?.ranges ?? []).map((range) => range.toString());
}

/**
 * A `ResizeObserver` that reports when the test says so: `resize()` calls
 * every live observer, as a layout change would.
 */
export function fakeResizeObserver() {
  const live = new Set<() => void>();
  class FakeResizeObserver {
    private callback: () => void;
    constructor(callback: () => void) {
      this.callback = callback;
    }
    observe() {
      live.add(this.callback);
    }
    unobserve() {}
    disconnect() {
      live.delete(this.callback);
    }
  }
  return { FakeResizeObserver, resize: () => [...live].forEach((callback) => callback()) };
}
