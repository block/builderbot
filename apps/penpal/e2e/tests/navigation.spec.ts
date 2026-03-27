import { test, expect } from '@playwright/test';

// E-PENPAL-HOME-REDIRECT: verifies index route redirects to /recent or /workspace.
test('index redirects to recent or workspace', async ({ page }) => {
  const response = await page.goto('/');
  expect(response?.url()).toMatch(/\/(recent|workspace)/);
});

// E-PENPAL-SPA-SERVE: verifies SPA serves the recent page at /recent.
test('recent page loads', async ({ page }) => {
  await page.goto('/recent');
  await expect(page).toHaveTitle(/penpal/i);
});

// E-PENPAL-SPA-SERVE: verifies SPA serves the in-review page at /in-review.
test('in-review page loads', async ({ page }) => {
  await page.goto('/in-review');
  await expect(page.locator('body')).toBeVisible();
});

// E-PENPAL-SPA-SERVE: verifies SPA serves the search page at /search.
test('search page loads', async ({ page }) => {
  await page.goto('/search');
  await expect(page.locator('body')).toBeVisible();
});
