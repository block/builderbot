import { test, expect } from '@playwright/test';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { blockPendingNavigation, dismissUpdateModal } from '../helpers/fixtures';

const BASE_URL = 'http://localhost:18923';

// Use the Vite dev server for page navigation (SPA routing).
// API calls go directly to the Go server via VITE_API_URL.
test.use({ baseURL: 'http://localhost:18924' });

let tmpDir: string;
let projectName: string;
const filePath = 'thoughts/test-doc.md';

// E-PENPAL-FIND-BAR: verifies find-in-page scrolling in a real browser.
test.describe('find in page', () => {
  test.beforeAll(async ({ request }) => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'penpal-e2e-find-'));
    const thoughtsDir = path.join(tmpDir, 'thoughts');
    fs.mkdirSync(thoughtsDir);

    // Build a document long enough to require scrolling, with a unique
    // marker phrase near the bottom that we can search for.
    const lines: string[] = ['# Find Test\n'];
    for (let i = 0; i < 80; i++) {
      lines.push(`Paragraph ${i}: Lorem ipsum dolor sit amet.\n`);
    }
    lines.push('\nUNIQUE_SEARCH_TARGET: this line should be scrolled into view.\n');
    for (let i = 0; i < 20; i++) {
      lines.push(`Trailing paragraph ${i}.\n`);
    }
    fs.writeFileSync(path.join(thoughtsDir, 'test-doc.md'), lines.join('\n'));

    const openRes = await request.post(`${BASE_URL}/api/open`, {
      data: { path: tmpDir },
    });
    expect(openRes.ok()).toBeTruthy();
    const openData = await openRes.json();
    projectName = openData.url.replace('/project/', '');

    // Clear our own pending navigation.
    await request.get(`${BASE_URL}/api/navigate`);
  });

  test.afterAll(async ({ request }) => {
    await request.delete(`${BASE_URL}/api/projects`, {
      data: { path: tmpDir },
    });
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  test('scrolls to match on search and navigation', async ({ page }) => {
    // Inject __TAURI__ so isDesktopApp is true and FindBar renders
    await page.addInitScript(() => {
      (window as any).__TAURI__ = {};
    });
    await blockPendingNavigation(page);

    await page.goto(`/file/${projectName}/${filePath}`);

    // Wait for content to render
    const content = page.locator('#content');
    await expect(content).toBeVisible({ timeout: 10000 });
    await expect(content).toContainText('UNIQUE_SEARCH_TARGET');

    await dismissUpdateModal(page);

    // The scroll container on file pages is .file-main-scroll
    const scrollContainer = page.locator('.file-main-scroll');
    await expect(scrollContainer).toBeVisible();

    // Open find bar via the menu-find custom event (same as Tauri menu)
    await page.evaluate(() => {
      window.dispatchEvent(new Event('menu-find'));
    });

    const findBar = page.locator('.find-bar');
    await expect(findBar).toBeVisible();

    const input = page.locator('.find-bar-input');
    await expect(input).toBeFocused();

    // Type a query that matches the unique marker near the bottom
    await input.fill('UNIQUE_SEARCH_TARGET');

    // Wait for the match count to appear
    const count = page.locator('.find-bar-count');
    await expect(count).toContainText('1 of 1');

    // Wait for smooth scroll to finish
    await page.waitForTimeout(1000);

    // Verify the scroll container actually scrolled — the marker is far
    // down the page, so scrollTop should be well above 0.
    const scrollTop = await scrollContainer.evaluate((el) => el.scrollTop);
    expect(scrollTop).toBeGreaterThan(100);
  });

  test('navigating between matches scrolls to each one', async ({ page }) => {
    await page.addInitScript(() => {
      (window as any).__TAURI__ = {};
    });
    await blockPendingNavigation(page);

    await page.goto(`/file/${projectName}/${filePath}`);
    const content = page.locator('#content');
    await expect(content).toBeVisible({ timeout: 10000 });
    await expect(content).toContainText('Lorem ipsum');

    await dismissUpdateModal(page);

    const scrollContainer = page.locator('.file-main-scroll');

    // Open find bar
    await page.evaluate(() => {
      window.dispatchEvent(new Event('menu-find'));
    });
    const input = page.locator('.find-bar-input');
    await expect(input).toBeFocused();

    // Search for "Lorem ipsum" which appears 80 times
    await input.fill('Lorem ipsum');
    const count = page.locator('.find-bar-count');
    await expect(count).toContainText('1 of 80');

    // Record initial scroll position
    await page.waitForTimeout(300);
    const initialScroll = await scrollContainer.evaluate((el) => el.scrollTop);

    // Press Enter in the find input to advance to next match. Using
    // keyboard avoids being blocked by modal overlays that can appear
    // asynchronously (e.g. "Update Command Line Tools").
    for (let i = 0; i < 40; i++) {
      await input.press('Enter');
    }
    await expect(count).toContainText('41 of 80');

    // Wait for smooth scroll
    await page.waitForTimeout(500);

    // Scroll position should have changed significantly
    const laterScroll = await scrollContainer.evaluate((el) => el.scrollTop);
    expect(laterScroll).toBeGreaterThan(initialScroll + 100);
  });
});
