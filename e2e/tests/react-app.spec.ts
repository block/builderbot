import { test, expect } from '@playwright/test';

// React app tests use the Vite preview server
test.use({ baseURL: 'http://localhost:18924' });

test.describe('React app', () => {
  test('loads and renders layout', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByTestId('app-layout')).toBeVisible();
  });

  test('renders topbar with logo and search', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByText('Penpal', { exact: true })).toBeVisible();
    await expect(page.getByPlaceholder('Search all thoughts...')).toBeVisible();
  });

  test('renders sidebar with navigation links', async ({ page }) => {
    await page.goto('/');
    const sidebar = page.getByTestId('sidebar');
    await expect(sidebar).toBeVisible();
    await expect(sidebar.getByText('Recent')).toBeVisible();
    await expect(sidebar.getByText('In Review')).toBeVisible();
  });

  test('renders theme toggle', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByLabel('Toggle dark mode')).toBeVisible();
  });

  test('SPA routing works for recent', async ({ page }) => {
    await page.goto('/recent');
    await expect(page.getByTestId('recent-page')).toBeVisible();
  });

  test('SPA routing works for in-review', async ({ page }) => {
    await page.goto('/in-review');
    await expect(page.getByTestId('in-review-page')).toBeVisible();
  });

  test('SPA routing works for search', async ({ page }) => {
    await page.goto('/search?q=test');
    await expect(page.getByTestId('search-page')).toBeVisible();
    await expect(page.getByText('Results for: test')).toBeVisible();
  });
});
