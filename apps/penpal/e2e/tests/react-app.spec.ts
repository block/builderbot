import { test, expect } from '@playwright/test';

// React app tests use the Vite preview server
test.use({ baseURL: 'http://localhost:18924' });

test.describe('React app', () => {
  // E-PENPAL-FRONTEND-STACK: verifies React SPA loads and renders the app layout.
  test('loads and renders layout', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByTestId('app-layout')).toBeVisible();
  });

  // E-PENPAL-HOME-SIDEBAR: verifies sidebar renders Recent and In Review navigation links.
  test('renders sidebar with navigation links', async ({ page }) => {
    await page.goto('/');
    const sidebar = page.getByTestId('sidebar');
    await expect(sidebar).toBeVisible();
    await expect(sidebar.getByText('Recent')).toBeVisible();
    await expect(sidebar.getByText('In Review')).toBeVisible();
  });

  // E-PENPAL-THEME: verifies the dark mode toggle button is rendered.
  test('renders theme toggle', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByLabel('Toggle dark mode')).toBeVisible();
  });

  // E-PENPAL-SPA-SERVE: verifies client-side routing serves the recent page.
  test('SPA routing works for recent', async ({ page }) => {
    await page.goto('/recent');
    await expect(page.getByTestId('recent-page')).toBeVisible();
  });

  // E-PENPAL-SPA-SERVE: verifies client-side routing serves the in-review page.
  test('SPA routing works for in-review', async ({ page }) => {
    await page.goto('/in-review');
    await expect(page.getByTestId('in-review-page')).toBeVisible();
  });
});
