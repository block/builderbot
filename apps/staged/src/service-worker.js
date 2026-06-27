// @ts-nocheck
/// <reference lib="webworker" />

const CACHE_NAME = '__STAGED_CACHE_NAME__';

// The app shell entry. Precached on install so navigations can be served
// cache-first (instant paint) without ever awaiting the network.
const APP_SHELL_URL = '/';

// Install: pre-cache the app shell entry point.
// Vite-hashed assets (the JS/CSS bundle) are cached on first fetch via the
// fetch handler; once cached they are immutable and served cache-first too, so
// after the first load the entire shell paints from cache with no network.
self.addEventListener('install', (event) => {
  event.waitUntil(caches.open(CACHE_NAME).then((cache) => cache.addAll([APP_SHELL_URL])));
  // Activate immediately instead of waiting for old tabs to close.
  self.skipWaiting();
});

// Activate: clean up old caches from previous versions.
self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((key) => key !== CACHE_NAME).map((key) => caches.delete(key)))
      )
  );
  // Start controlling all open clients immediately.
  self.clients.claim();
});

// Fetch: stale-while-revalidate for navigation, cache-first for hashed assets,
// never-cache for the API.
self.addEventListener('fetch', (event) => {
  const url = new URL(event.request.url);

  // Never cache API calls or WebSocket upgrades.
  if (url.pathname.startsWith('/api/')) return;

  // Navigation requests (HTML pages): stale-while-revalidate against the
  // precached app shell. Serving the cached shell immediately is what removes
  // the multi-second blank screen on an iOS resume — the previous network-first
  // strategy awaited a full round trip and only fell back to cache on a network
  // *error*, so a slow connection still produced a blank page. We revalidate the
  // shell in the background and fall back to the network only on a cache miss.
  if (event.request.mode === 'navigate') {
    event.respondWith(
      caches.open(CACHE_NAME).then(async (cache) => {
        const cached = (await cache.match(event.request)) || (await cache.match(APP_SHELL_URL));

        const network = fetch(event.request)
          .then((response) => {
            if (response.ok) cache.put(APP_SHELL_URL, response.clone());
            return response;
          })
          .catch(() => null);

        if (cached) {
          // Paint immediately; revalidate the shell for the next load.
          event.waitUntil(network);
          return cached;
        }

        // Cold cache (first ever load): fall back to the network.
        const response = await network;
        return response || new Response('', { status: 503, statusText: 'Service Unavailable' });
      })
    );
    return;
  }

  // Static assets (JS, CSS, images): Vite hashes these filenames, so they are
  // immutable and safe to serve cache-first.
  if (
    url.pathname.startsWith('/assets/') ||
    url.pathname.endsWith('.svg') ||
    url.pathname.endsWith('.png') ||
    url.pathname.endsWith('.ico')
  ) {
    event.respondWith(
      caches.match(event.request).then(
        (cached) =>
          cached ||
          fetch(event.request).then((response) => {
            const clone = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(event.request, clone));
            return response;
          })
      )
    );
    return;
  }
});
