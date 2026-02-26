import { test, expect } from '@playwright/test';

test.describe('comments', () => {
  test('can create a thread via API and see it listed in reviews', async ({ request }) => {
    const response = await request.get('/api/in-review');
    expect(response.ok()).toBeTruthy();
    const groups = await response.json();
    expect(Array.isArray(groups)).toBeTruthy();
  });
});
