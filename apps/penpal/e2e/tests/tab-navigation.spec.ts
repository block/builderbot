import { test, expect, type Page } from '@playwright/test';

test.use({ baseURL: 'http://localhost:18924' });

/** Navigate within the SPA by evaluating a click on a sidebar link. */
async function navigateViaSidebar(page: Page, href: string) {
  await page.evaluate((h) => {
    const link = document.querySelector(`nav.sidebar a[href="${h}"]`) as HTMLAnchorElement;
    if (link) link.click();
  }, href);
  await expect(page).toHaveURL(new RegExp(href.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
}

test.describe('Per-tab back/forward navigation', () => {
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

    await navigateViaSidebar(page, '/in-review');

    // Cmd+[ to go back
    await page.keyboard.press('Meta+[');
    await expect(page).toHaveURL(/\/recent/);

    // Cmd+] to go forward
    await page.keyboard.press('Meta+]');
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

    // Now navigate — back should enable (navigating flag wasn't corrupted)
    await navigateViaSidebar(page, '/in-review');
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
