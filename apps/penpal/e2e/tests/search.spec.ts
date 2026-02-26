import { test, expect } from '@playwright/test';

test('search page shows results form', async ({ page }) => {
  await page.goto('/search');
  await expect(page.locator('body')).toContainText(/search/i);
});

test('search with query returns results page', async ({ page }) => {
  await page.goto('/search?q=test');
  await expect(page.locator('body')).toBeVisible();
});
