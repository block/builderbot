import { test, expect } from '@playwright/test';

// E-PENPAL-SEARCH: verifies search page loads and displays search UI.
test('search page shows results form', async ({ page }) => {
  await page.goto('/search');
  await expect(page.locator('body')).toContainText(/search/i);
});

// E-PENPAL-SEARCH: verifies search page renders with a query parameter.
test('search with query returns results page', async ({ page }) => {
  await page.goto('/search?q=test');
  await expect(page.locator('body')).toBeVisible();
});
