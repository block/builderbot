import { test, expect, type Page } from '@playwright/test';
import { blockPendingNavigation } from '../helpers/fixtures';

test.use({ baseURL: 'http://localhost:18924' });

/** Navigate within the SPA by clicking a sidebar link via JS. */
async function navigateViaSidebar(page: Page, href: string) {
  // Wait for the link to render before clicking via evaluate. Using
  // evaluate (not Playwright .click()) preserves focus on the page body,
  // which is required for keyboard shortcut tests.
  await page.locator(`nav.sidebar a[href="${href}"]`).waitFor({ state: 'attached' });
  await page.evaluate((h) => {
    const link = document.querySelector(`nav.sidebar a[href="${h}"]`) as HTMLAnchorElement;
    if (link) link.click();
  }, href);
  await expect(page).toHaveURL(new RegExp(href.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
}

// E-PENPAL-TABS: verifies per-tab back/forward history navigation, keyboard shortcuts, and tab independence.
test.describe('Per-tab back/forward navigation', () => {
  // Block both stale pendingNav (HTTP) and real-time navigate SSE events
  // that other parallel tests may broadcast.  The keyboard shortcut test
  // uses page.evaluate(KeyboardEvent) so the EventSource wrapper is safe.
  test.beforeEach(async ({ page }) => {
    await blockPendingNavigation(page);
  });

  test('back and forward buttons render disabled on fresh tab', async ({ page }) => {
    await page.goto('/recent');
    const back = page.getByLabel('Go back');
    const forward = page.getByLabel('Go forward');
    await expect(back).toBeVisible();
    await expect(forward).toBeVisible();
    await expect(back).toBeDisabled();
    await expect(forward).toBeDisabled();
  });

  test('back button enables after navigating, forward after going back', async ({ page }) => {
    await page.goto('/recent');
    const back = page.getByLabel('Go back');
    const forward = page.getByLabel('Go forward');

    // Navigate to a second page
    await navigateViaSidebar(page, '/in-review');
    await expect(back).toBeEnabled();
    await expect(forward).toBeDisabled();

    // Go back
    await back.click();
    await expect(page).toHaveURL(/\/recent/);
    await expect(back).toBeDisabled();
    await expect(forward).toBeEnabled();

    // Go forward
    await forward.click();
    await expect(page).toHaveURL(/\/in-review/);
    await expect(back).toBeEnabled();
    await expect(forward).toBeDisabled();
  });

  test('Cmd+[ and Cmd+] keyboard shortcuts work', async ({ page }) => {
    await page.goto('/recent');
    const back = page.getByLabel('Go back');
    const forward = page.getByLabel('Go forward');

    await navigateViaSidebar(page, '/in-review');
    // The keydown handler is re-registered in a useEffect that depends on
    // canGoBack.  Waiting for the button state ensures the render committed;
    // a micro-wait lets the effect fire and register the new handler.
    await expect(back).toBeEnabled();

    // Cmd+[ to go back — dispatch via evaluate to avoid headless Chromium
    // swallowing Meta+[ as a browser shortcut.
    await page.evaluate(() =>
      window.dispatchEvent(new KeyboardEvent('keydown', { key: '[', metaKey: true, bubbles: true })),
    );
    await expect(page).toHaveURL(/\/recent/);
    await expect(forward).toBeEnabled();

    // Cmd+] to go forward
    await page.evaluate(() =>
      window.dispatchEvent(new KeyboardEvent('keydown', { key: ']', metaKey: true, bubbles: true })),
    );
    await expect(page).toHaveURL(/\/in-review/);
  });

  test('navigating after going back truncates forward history', async ({ page }) => {
    await page.goto('/recent');
    const back = page.getByLabel('Go back');
    const forward = page.getByLabel('Go forward');

    // Build history: recent -> in-review
    await navigateViaSidebar(page, '/in-review');

    // Go back to recent
    await back.click();
    await expect(page).toHaveURL(/\/recent/);
    await expect(forward).toBeEnabled();

    // Navigate somewhere new — forward history should be truncated
    await navigateViaSidebar(page, '/in-review');
    await expect(forward).toBeDisabled();
  });

  test('browser back/forward (POP) syncs tab history without duplicates', async ({ page }) => {
    await page.goto('/recent');
    const back = page.getByLabel('Go back');
    const forward = page.getByLabel('Go forward');

    // Navigate: recent -> in-review
    await navigateViaSidebar(page, '/in-review');
    await expect(back).toBeEnabled();

    // Use browser back (history.back)
    await page.goBack();
    await expect(page).toHaveURL(/\/recent/);
    await expect(back).toBeDisabled();
    await expect(forward).toBeEnabled();

    // Use browser forward (history.forward)
    await page.goForward();
    await expect(page).toHaveURL(/\/in-review/);
    await expect(back).toBeEnabled();
    await expect(forward).toBeDisabled();
  });

  test('re-activating current tab does not break next navigation', async ({ page }) => {
    await page.goto('/recent');
    const back = page.getByLabel('Go back');

    // Click the already-active tab
    const activeTab = page.getByTestId('topbar-tabs').locator('.tab-bar-tab.active');
    await activeTab.click();

    // Now navigate — use Playwright's native click (not evaluate) to navigate
    const inReviewLink = page.locator('nav.sidebar a[href="/in-review"]');
    await inReviewLink.click();
    await expect(page).toHaveURL(/\/in-review/);
    await expect(back).toBeEnabled();
  });

  test('each tab has independent history', async ({ page }) => {
    await page.goto('/recent');
    const back = page.getByLabel('Go back');

    // Navigate in first tab: recent -> in-review
    await navigateViaSidebar(page, '/in-review');
    await expect(back).toBeEnabled();

    // Open a new tab
    await page.getByLabel('New tab').click();

    // New tab should have no history — back disabled
    await expect(back).toBeDisabled();

    // Switch back to first tab — back should be enabled again
    const firstTab = page.getByTestId('topbar-tabs').locator('.tab-bar-tab').first();
    await firstTab.click();
    await expect(back).toBeEnabled();
  });
});
