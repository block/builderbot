import { test, expect } from '@playwright/test';

test('index redirects to recent or workspace', async ({ page }) => {
  const response = await page.goto('/');
  expect(response?.url()).toMatch(/\/(recent|workspace)/);
});

test('recent page loads', async ({ page }) => {
  await page.goto('/recent');
  await expect(page).toHaveTitle(/penpal/i);
});

test('in-review page loads', async ({ page }) => {
  await page.goto('/in-review');
  await expect(page.locator('body')).toBeVisible();
});

test('search page loads', async ({ page }) => {
  await page.goto('/search');
  await expect(page.locator('body')).toBeVisible();
});
