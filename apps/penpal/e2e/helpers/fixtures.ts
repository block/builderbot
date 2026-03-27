import type { Page } from '@playwright/test';

/**
 * Prevent cross-test navigation interference. Call once per page, before page.goto().
 *
 * POST /api/open both sets a server-side pendingNav AND broadcasts a
 * "navigate" SSE event to all connected browsers. When tests run in
 * parallel, another test's /api/open can redirect this page away from
 * its intended URL through either channel.
 *
 * This helper blocks both:
 *  1. Intercepts GET /api/navigate HTTP requests (used by onConnect)
 *  2. Filters "navigate" events from the SSE stream (received in real-time)
 */
export async function blockPendingNavigation(page: Page) {
  // Block the HTTP endpoint used on SSE reconnect
  await page.route('**/api/navigate', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({}),
    });
  });

  // Wrap EventSource to suppress SSE "navigate" events from other tests
  await page.addInitScript(() => {
    const RealEventSource = window.EventSource;
    window.EventSource = function (url: string | URL, init?: EventSourceInit) {
      const es = new RealEventSource(url, init);
      const origAddEventListener = es.addEventListener.bind(es);
      es.addEventListener = function (type: string, listener: EventListenerOrEventListenerObject | null, options?: boolean | AddEventListenerOptions) {
        if (type === 'change' && typeof listener === 'function') {
          const original = listener;
          const filtered = function (this: EventSource, e: Event) {
            try {
              const data = JSON.parse((e as MessageEvent).data);
              if (data.type === 'navigate') return;
            } catch { /* pass through non-JSON events */ }
            original.call(this, e);
          };
          return origAddEventListener(type, filtered, options);
        }
        return origAddEventListener(type, listener, options);
      };
      return es;
    } as unknown as typeof EventSource;
    window.EventSource.CONNECTING = RealEventSource.CONNECTING;
    window.EventSource.OPEN = RealEventSource.OPEN;
    window.EventSource.CLOSED = RealEventSource.CLOSED;
  });
}

/**
 * Light-weight navigation guard: only intercepts the HTTP
 * GET /api/navigate endpoint (consumed on SSE connect/reconnect).
 * Does NOT wrap EventSource, so it won't interfere with keyboard
 * event dispatch or other page-level JS behaviour.
 *
 * Use this for tests that don't create projects themselves but could
 * be affected by stale pendingNav left from a previous test run.
 */
export async function blockStalePendingNavigation(page: Page) {
  await page.route('**/api/navigate', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({}),
    });
  });
}

/**
 * Dismiss the "Update Command Line Tools" modal if it appears.
 */
export async function dismissUpdateModal(page: Page) {
  const notNow = page.getByRole('button', { name: 'Not Now' });
  if (await notNow.isVisible({ timeout: 2000 }).catch(() => false)) {
    await notNow.click();
  }
}
