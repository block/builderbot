import { test, expect } from '@playwright/test';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

const SERVER_PORT = 18923;
const BASE_URL = `http://localhost:${SERVER_PORT}`;

let tmpDir: string;

// E-PENPAL-CLI: verifies open command registers file, sets pending navigation, and consumes it.
test.describe('open command navigation', () => {
  test.beforeAll(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'penpal-e2e-cli-open-'));
    const thoughtsDir = path.join(tmpDir, 'thoughts');
    fs.mkdirSync(thoughtsDir);
    fs.writeFileSync(
      path.join(thoughtsDir, 'cli-test.md'),
      '# CLI Test\n\nOpened via penpal open command.\n',
    );
  });

  test.afterAll(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  test('/api/open registers file and sets pending navigation', async ({ request }) => {
    const mdFile = path.join(tmpDir, 'thoughts', 'cli-test.md');

    // POST /api/open — same as what `penpal open` does
    const openRes = await request.post(`${BASE_URL}/api/open`, {
      data: { path: mdFile },
    });
    expect(openRes.ok()).toBeTruthy();
    const openData = await openRes.json();
    expect(openData.url).toMatch(/\/file\/.*cli-test\.md/);

    // /api/navigate should return the same URL (pending navigation)
    const navRes = await request.get(`${BASE_URL}/api/navigate`);
    expect(navRes.ok()).toBeTruthy();
    const navData = await navRes.json();
    expect(navData.url).toBe(openData.url);
  });

  test('/api/navigate is consumed after first read', async ({ request }) => {
    const mdFile = path.join(tmpDir, 'thoughts', 'cli-test.md');
    await request.post(`${BASE_URL}/api/open`, {
      data: { path: mdFile },
    });

    // First read returns the URL
    const data = await (await request.get(`${BASE_URL}/api/navigate`)).json();
    expect(data.url).toMatch(/\/file\/.*cli-test\.md/);

    // Second read returns empty (consumed)
    const data2 = await (await request.get(`${BASE_URL}/api/navigate`)).json();
    expect(data2.url).toBeUndefined();
  });

  test('/api/navigate returns empty when nothing pending', async ({ request }) => {
    // Clear any pending nav first
    await request.get(`${BASE_URL}/api/navigate`);

    const data = await (await request.get(`${BASE_URL}/api/navigate`)).json();
    expect(data.url).toBeUndefined();
  });
});
