import { test, expect } from '@playwright/test';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { blockPendingNavigation } from '../helpers/fixtures';

const BASE_URL = 'http://localhost:18923';

// Use the Vite dev server for page navigation (SPA routing).
test.use({ baseURL: 'http://localhost:18924' });

let tmpDir: string;
let projectName: string;
const filePath = 'thoughts/live-doc.md';

// E-PENPAL-SSE: verifies that file changes on disk are reflected in the browser via SSE.
// Serial mode: these tests depend on real-time fsnotify + SSE delivery within
// 500ms.  Parallel workers that hammer the watcher create I/O contention that
// pushes latency past the SLA, so we pin this suite to a single worker.
test.describe.configure({ mode: 'serial' });
test.describe('SSE file refresh', () => {
  test.beforeAll(async ({ request }) => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'penpal-e2e-sse-'));
    const thoughtsDir = path.join(tmpDir, 'thoughts');
    fs.mkdirSync(thoughtsDir);
    fs.writeFileSync(
      path.join(thoughtsDir, 'live-doc.md'),
      '# Original Title\n\nOriginal content paragraph.\n',
    );

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

  // E-PENPAL-SSE: verifies file content refreshes automatically when file changes on disk.
  test('file content updates when file changes on disk', async ({ page }) => {
    await blockPendingNavigation(page);
    // Wait for the focus POST so the watcher is registered before we write.
    const focusReady = page.waitForResponse(
      (resp) => resp.url().includes('/api/focus') && resp.ok(),
    );
    await page.goto(`/file/${projectName}/${filePath}`);

    const content = page.locator('#content');
    await expect(content).toBeVisible({ timeout: 10000 });
    await expect(content).toContainText('Original content paragraph');
    await focusReady;

    // Modify the file on disk — the watcher should detect the change,
    // broadcast via SSE, and FilePage should re-fetch and re-render.
    fs.writeFileSync(
      path.join(tmpDir, 'thoughts', 'live-doc.md'),
      '# Updated Title\n\nBrand new content after SSE refresh.\n',
    );

    // fsnotify + 100ms debounce + SSE + render should complete well under 500ms
    await expect(content).toContainText('Brand new content after SSE refresh', {
      timeout: 500,
    });

    // Verify the old content is gone
    await expect(content).not.toContainText('Original content paragraph');
  });

  // E-PENPAL-SSE: verifies multiple sequential file edits each trigger a refresh.
  test('multiple edits are each reflected in the browser', async ({ page }) => {
    // Reset the file to a known state
    fs.writeFileSync(
      path.join(tmpDir, 'thoughts', 'live-doc.md'),
      '# Step Zero\n\nInitial state for multi-edit test.\n',
    );

    await blockPendingNavigation(page);
    // Wait for the focus POST so the watcher is registered before we write.
    const focusReady = page.waitForResponse(
      (resp) => resp.url().includes('/api/focus') && resp.ok(),
    );
    await page.goto(`/file/${projectName}/${filePath}`);

    const content = page.locator('#content');
    await expect(content).toBeVisible({ timeout: 10000 });
    await expect(content).toContainText('Initial state for multi-edit test');
    await focusReady;

    // First edit
    fs.writeFileSync(
      path.join(tmpDir, 'thoughts', 'live-doc.md'),
      '# Step One\n\nFirst live update arrived.\n',
    );
    await expect(content).toContainText('First live update arrived', {
      timeout: 500,
    });

    // Wait for the first debounce cycle (100ms) to fully settle before
    // writing again, so the second edit triggers its own broadcast.
    await page.waitForTimeout(200);

    // Second edit — verify SSE keeps working after the first refresh
    fs.writeFileSync(
      path.join(tmpDir, 'thoughts', 'live-doc.md'),
      '# Step Two\n\nSecond live update arrived.\n',
    );
    await expect(content).toContainText('Second live update arrived', {
      timeout: 500,
    });
  });
});
