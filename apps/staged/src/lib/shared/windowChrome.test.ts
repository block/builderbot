import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The room the top bar keeps for the macOS window controls. It follows the
// window (the desktop app, on a Mac, not fullscreen), not the layout: a
// window narrower than the mobile breakpoint still has its traffic lights.

let isTauri: boolean;

async function importWindowChrome() {
  return await import('./windowChrome.svelte');
}

function setNavigator(platform: string, userAgent = '') {
  vi.stubGlobal('navigator', { platform, userAgent });
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (navigation.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);
  isTauri = true;
  setNavigator('MacIntel');
  vi.doMock('../transport', () => ({
    get isTauri() {
      return isTauri;
    },
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('getTrafficLightSpacerWidth', () => {
  it('keeps room for the traffic lights in the desktop app on a Mac', async () => {
    const { getTrafficLightSpacerWidth } = await importWindowChrome();

    expect(getTrafficLightSpacerWidth(false)).toBe(70);
  });

  it('keeps none in a fullscreen window, where the controls are hidden', async () => {
    const { getTrafficLightSpacerWidth, windowChrome } = await importWindowChrome();
    windowChrome.isFullscreen = true;

    expect(getTrafficLightSpacerWidth(false)).toBe(0);
  });

  it('keeps none in a browser, on a Mac or a phone', async () => {
    isTauri = false;
    const { getTrafficLightSpacerWidth } = await importWindowChrome();

    expect(getTrafficLightSpacerWidth(false)).toBe(0);
    expect(getTrafficLightSpacerWidth(true)).toBe(0);
  });

  it('keeps none on a touch device, even one that says it is a Mac', async () => {
    // iPadOS reports MacIntel.
    const { getTrafficLightSpacerWidth } = await importWindowChrome();

    expect(getTrafficLightSpacerWidth(true)).toBe(0);
  });

  it('keeps none in the desktop app off macOS', async () => {
    setNavigator('Win32', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)');
    const { getTrafficLightSpacerWidth } = await importWindowChrome();

    expect(getTrafficLightSpacerWidth(false)).toBe(0);
  });
});
