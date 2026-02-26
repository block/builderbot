import { test, expect } from '@playwright/test';

test.describe('POST /api/view', () => {
  test('records a view and returns 204', async ({ request }) => {
    const response = await request.post('/api/view?project=test-proj&path=thoughts/test.md');
    expect(response.status()).toBe(204);
  });

  test('returns 400 when project is missing', async ({ request }) => {
    const response = await request.post('/api/view?path=thoughts/test.md');
    expect(response.status()).toBe(400);
  });

  test('returns 400 when path is missing', async ({ request }) => {
    const response = await request.post('/api/view?project=test-proj');
    expect(response.status()).toBe(400);
  });

  test('returns 405 for GET requests', async ({ request }) => {
    const response = await request.get('/api/view?project=test-proj&path=thoughts/test.md');
    expect(response.status()).toBe(405);
  });
});
