import { test, expect } from '@playwright/test';

// SPA routes require the Vite dev server (Go server serves the SPA at /app/).
test.use({ baseURL: 'http://localhost:18924' });

// E-PENPAL-HOME-DEFAULT: verifies index route renders the home welcome screen.
test('index renders home view', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('Select a project from the sidebar')).toBeVisible();
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

