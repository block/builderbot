import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

let inlineValue: string;
let getComputedStyle: ReturnType<typeof vi.fn>;
let observerCallback: (() => void) | null;

async function importDarkMode() {
  const { darkMode } = await import('./isDark.svelte');
  return darkMode;
}

beforeEach(() => {
  vi.resetModules();
  // `$state` compiles away in the app build; under vitest it is a plain
  // global call, so stub it as identity.
  vi.stubGlobal('$state', (initial: unknown) => initial);
  inlineValue = '';
  observerCallback = null;
  getComputedStyle = vi.fn(() => ({ getPropertyValue: () => ' 1 ' }));
  vi.stubGlobal('getComputedStyle', getComputedStyle);
  vi.stubGlobal('document', {
    documentElement: {
      style: {
        getPropertyValue: (name: string) => (name === '--theme-is-dark' ? inlineValue : ''),
      },
    },
  });
  vi.stubGlobal(
    'MutationObserver',
    class {
      constructor(cb: () => void) {
        observerCallback = cb;
      }
      observe() {}
      disconnect() {}
    }
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('darkMode', () => {
  it('reads the inline --theme-is-dark without forcing a computed-style read', async () => {
    inlineValue = '1';
    const darkMode = await importDarkMode();
    darkMode.init();

    expect(darkMode.value).toBe(true);
    expect(getComputedStyle).not.toHaveBeenCalled();

    inlineValue = '0';
    observerCallback?.();
    expect(darkMode.value).toBe(false);
    expect(getComputedStyle).not.toHaveBeenCalled();
  });

  it('falls back to getComputedStyle when no inline value is set', async () => {
    const darkMode = await importDarkMode();
    darkMode.init();

    expect(getComputedStyle).toHaveBeenCalledTimes(1);
    expect(darkMode.value).toBe(true);
  });
});
