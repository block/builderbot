import { isTauri } from '../transport';

const TRAFFIC_LIGHT_SPACER_WIDTH_PX = 70;

function isMacPlatform(): boolean {
  if (typeof navigator === 'undefined') return false;

  return /mac/i.test(navigator.platform) || /Macintosh/i.test(navigator.userAgent);
}

export const windowChrome = $state({
  isMac: isMacPlatform(),
  isFullscreen: false,
});

let subscriberCount = 0;
let stopBrowserResize: (() => void) | null = null;
let stopTauriResize: (() => void) | null = null;
let tauriResizeListenerGeneration = 0;
let fullscreenSyncGeneration = 0;

async function syncFullscreen() {
  const generation = ++fullscreenSyncGeneration;

  if (!isTauri) {
    windowChrome.isFullscreen = false;
    return;
  }

  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const isFullscreen = await getCurrentWindow().isFullscreen();
    if (generation === fullscreenSyncGeneration && subscriberCount > 0) {
      windowChrome.isFullscreen = isFullscreen;
    }
  } catch (error) {
    console.warn('[windowChrome] Failed to sync fullscreen state:', error);
    if (generation === fullscreenSyncGeneration && subscriberCount > 0) {
      windowChrome.isFullscreen = false;
    }
  }
}

function syncWindowChrome() {
  windowChrome.isMac = isMacPlatform();
  void syncFullscreen();
}

function registerTauriResizeListener() {
  if (!isTauri) return;

  const generation = ++tauriResizeListenerGeneration;

  void (async () => {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const unlisten = await getCurrentWindow().onResized(syncWindowChrome);

    if (subscriberCount === 0 || generation !== tauriResizeListenerGeneration) {
      unlisten();
    } else {
      stopTauriResize = unlisten;
    }
  })().catch((error) => {
    console.warn('[windowChrome] Failed to watch resize events:', error);
  });
}

export function getTrafficLightSpacerWidth(isMobile: boolean): number {
  if (!isTauri || !windowChrome.isMac || isMobile || windowChrome.isFullscreen) return 0;
  return TRAFFIC_LIGHT_SPACER_WIDTH_PX;
}

export function watchWindowChrome(): () => void {
  if (typeof window === 'undefined') return () => {};

  subscriberCount += 1;
  syncWindowChrome();

  if (subscriberCount === 1) {
    const onResize = () => syncWindowChrome();
    window.addEventListener('resize', onResize);
    stopBrowserResize = () => window.removeEventListener('resize', onResize);
    registerTauriResizeListener();
  }

  return () => {
    subscriberCount = Math.max(0, subscriberCount - 1);

    if (subscriberCount === 0) {
      fullscreenSyncGeneration += 1;
      tauriResizeListenerGeneration += 1;
      stopBrowserResize?.();
      stopTauriResize?.();
      stopBrowserResize = null;
      stopTauriResize = null;
    }
  };
}
