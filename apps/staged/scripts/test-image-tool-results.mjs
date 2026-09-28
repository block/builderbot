// Run from apps/staged: node scripts/test-image-tool-results.mjs
// Optionally set SCREENSHOT_DIR and CHROMIUM_EXECUTABLE_PATH / WEBKIT_EXECUTABLE_PATH.
// Reuse the workspace's Playwright installation without a backend or session writes.
import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { after, before, describe, it } from 'node:test';
import { createServer } from 'vite';

const { chromium, webkit, expect } = createRequire(
  new URL('../../penpal/e2e/package.json', import.meta.url)
)('@playwright/test');
const root = fileURLToPath(new URL('../', import.meta.url));
const png = (await readFile(resolve(root, 'public/icons/staged-clear-128.png'))).toString('base64');
const image = { type: 'image', mimeType: 'image/png', data: png };
const summary = 'Screenshot captured';
const body = 'Image preview loaded successfully.\nDimensions: 128 x 128 pixels.';
const content = [image, { type: 'text', text: 'Preview ready' }];
const scenarios = {
  body: { status: 200, body },
  text: { status: 200, text: body },
  'nested-body': { content: [image], response: { status: 200, body } },
  'nested-text': { content: [image], response: { status: 200, text: body } },
  content: { status: 200, content },
  'nested-content': { response: { status: 200, content } },
  'same-summary': { status: 200, body: summary },
  'plain-content': { status: 200, content: { ok: true, count: 2 } },
};
let server;
let url;

before(async () => {
  server = await createServer({
    root,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, forwardConsole: false },
    plugins: [
      {
        name: 'image-results-probe',
        resolveId(id) {
          if (id === 'image-results-probe') return '\0image-results-probe';
        },
        load(id) {
          if (id !== '\0image-results-probe') return;
          return `
            import '/src/app.css';
            import { mount } from 'svelte';
            import ToolCallCard from '/src/lib/features/sessions/tool-calls/ToolCallCard.svelte';
            const image = ${JSON.stringify(image)};
            const scenarios = ${JSON.stringify(scenarios)};
            const scenario = new URLSearchParams(location.search).get('case') || 'body';
            const base = {
              key: 'network', call: { id: 1, role: 'tool_call', content: 'Browser capture',
                sessionId: 'image-probe', createdAt: 1 },
              result: null, verb: 'Fetched', detail: 'Image preview', status: 'completed',
              statusLabel: 'Succeeded', statusTone: 'success', toolCallId: 'image-probe',
              toolKind: 'network', rawInput: { method: 'GET', url: 'https://example.test/preview' },
              rawOutput: scenarios[scenario],
              content: [
                { type: 'content', content: { type: 'text', text: ${JSON.stringify(summary)} } },
                { type: 'content', content: image }
              ],
              isPikchrDiagramTool: false, innerSessionId: null, pikchrRenderSource: null
            };
            const read = { ...base, key: 'read', verb: 'Read', detail: 'staged-clear-128.png',
              toolKind: 'read', rawInput: { path: 'public/icons/staged-clear-128.png' },
              content: undefined,
              rawOutput: { body: 'Image loaded: Staged application icon.', content: [image] }
            };
            for (const item of [read, base]) {
              const target = document.createElement('section');
              target.dataset.tool = item.key;
              document.querySelector('main').append(target);
              mount(ToolCallCard, { target, props: {
                item, expanded: true, slideDuration: 0, onToggle() {}
              }});
            }
          `;
        },
        configureServer(vite) {
          vite.middlewares.use('/__image-results', (_request, response) => {
            response.setHeader('Content-Type', 'text/html');
            response.end(`<!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1">
              <style>body { margin: 0; } main { max-width: 760px; margin: 0 auto; padding: 24px; }
              h1 { font-size: 18px; font-weight: 600; margin: 0 0 24px; }
              main > section + section { margin-top: 24px; }</style></head>
              <body><main><h1>Image tool results</h1></main>
              <script type="module" src="/@id/image-results-probe"></script></body></html>`);
          });
        },
      },
    ],
  });
  await server.listen();
  url = `http://127.0.0.1:${server.httpServer.address().port}/__image-results`;
});

after(async () => server?.close());

for (const [name, engine] of Object.entries({ chromium, webkit })) {
  describe(name, { timeout: 120_000 }, () => {
    let browser;
    let page;
    const errors = [];
    before(async () => {
      browser = await engine.launch({
        executablePath: process.env[`${name.toUpperCase()}_EXECUTABLE_PATH`],
      });
      page = await browser.newPage({ viewport: { width: 1000, height: 800 } });
      page.on('pageerror', (error) => errors.push(error.message));
    });
    after(async () => browser?.close());

    for (const [scenario, output] of Object.entries(scenarios)) {
      it(`preserves the ${scenario} response alongside its image and summary`, async () => {
        await page.goto(`${url}?case=${scenario}`);
        const read = page.locator('[data-tool="read"]');
        await expect(read.locator('pre')).toHaveText('Image loaded: Staged application icon.');
        const network = page.locator('[data-tool="network"]');
        const expectedBody =
          output.body ??
          output.text ??
          output.response?.body ??
          output.response?.text ??
          (scenario === 'plain-content'
            ? JSON.stringify(output.content, null, 2)
            : 'Preview ready');
        await expect(network.locator('pre').first()).toHaveText(expectedBody);
        await expect(network.locator('pre')).toHaveCount(scenario === 'same-summary' ? 1 : 2);
        await expect(network.getByText(summary, { exact: true })).toHaveCount(1);
        await expect(page.locator('.image-preview img')).toHaveCount(2);
        await expect
          .poll(() =>
            page
              .locator('.image-preview img')
              .evaluateAll((images) =>
                images.every((image) => image.complete && image.naturalWidth === 128)
              )
          )
          .toBe(true);
        assert.ok(!(await page.locator('main').innerText()).includes(png));
        assert.deepEqual(errors, []);
      });
    }

    it('renders desktop and mobile previews and opens the full image', async () => {
      await page.goto(url);
      await expect(page.locator('.image-preview img')).toHaveCount(2);
      await expect
        .poll(() =>
          page
            .locator('.image-preview img')
            .evaluateAll((images) =>
              images.every((image) => image.complete && image.naturalWidth === 128)
            )
        )
        .toBe(true);
      for (const viewport of [
        { width: 1000, height: 800 },
        { width: 390, height: 844 },
      ]) {
        await page.setViewportSize(viewport);
        assert.equal(
          await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
          true
        );
        if (process.env.SCREENSHOT_DIR) {
          await mkdir(process.env.SCREENSHOT_DIR, { recursive: true });
          await page.screenshot({
            path: resolve(process.env.SCREENSHOT_DIR, `${name}-${viewport.width}.png`),
            fullPage: true,
          });
        }
      }
      await page.getByTitle('Expand image', { exact: true }).first().click();
      await expect(page.getByRole('dialog')).toBeVisible();
      await expect(page.getByRole('dialog').getByRole('img')).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(page.getByRole('dialog')).toHaveCount(0);
      assert.deepEqual(errors, []);
    });
  });
}
