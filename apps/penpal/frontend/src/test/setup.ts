import '@testing-library/jest-dom';

// Suppress unhandled AbortSignal rejections from react-router in jsdom.
// Node 24+ (undici v7) rejects jsdom's AbortSignal polyfill as not a "real" instance.
// See: https://github.com/vitest-dev/vitest/issues/8374
// This will be unnecessary once we upgrade to Vitest 4.
const _origListeners = process.listeners('unhandledRejection');
process.removeAllListeners('unhandledRejection');
process.on('unhandledRejection', (reason: unknown) => {
  if (reason instanceof TypeError && String(reason).includes('AbortSignal')) return;
  // Re-throw anything that isn't the known jsdom/undici mismatch
  throw reason;
});

// Ensure localStorage is available in jsdom environment
if (typeof globalThis.localStorage === 'undefined' || typeof globalThis.localStorage.clear !== 'function') {
  const store: Record<string, string> = {};
  globalThis.localStorage = {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => { store[key] = value; },
    removeItem: (key: string) => { delete store[key]; },
    clear: () => { Object.keys(store).forEach((k) => delete store[k]); },
    get length() { return Object.keys(store).length; },
    key: (i: number) => Object.keys(store)[i] ?? null,
  };
}

// Provide window.matchMedia mock if not available
if (typeof window.matchMedia !== 'function') {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}
