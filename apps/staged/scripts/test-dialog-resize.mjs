// Run from apps/staged: node scripts/test-dialog-resize.mjs
// Reuse the workspace's Playwright installation; no backend or real preferences are used.
// Install browsers with: pnpm --dir ../penpal/e2e exec playwright install chromium webkit
// Or point CHROMIUM_EXECUTABLE_PATH / WEBKIT_EXECUTABLE_PATH at existing browsers.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { after, before, describe, it } from 'node:test';
import { createServer } from 'vite';

const { chromium, webkit, expect } = createRequire(
  new URL('../../penpal/e2e/package.json', import.meta.url)
)('@playwright/test');
const root = fileURLToPath(new URL('../', import.meta.url));
const key = 'note-dialog-width';
let server;
let url;

before(async () => {
  server = await createServer({
    root,
    logLevel: 'error',
    server: { host: '127.0.0.1', port: 0, strictPort: false, forwardConsole: false },
    plugins: [
      {
        name: 'dialog-resize-probe',
        resolveId(id) {
          if (id === 'dialog-resize-probe') return '\0dialog-resize-probe';
        },
        load(id) {
          if (id !== '\0dialog-resize-probe') return;
          return `
            import '/src/app.css';
            import { mount } from 'svelte';
            import NoteModal from '/src/lib/features/notes/NoteModal.svelte';
            import { initPersistentStore } from '/src/lib/shared/persistentStore.ts';
            import { createDialogWidth } from '/src/lib/components/ui/dialog/dialogWidth.svelte.ts';
            await initPersistentStore();
            const width = createDialogWidth({ key: '${key}', minWidth: 700 });
            await width.ensureHydrated();
            mount(NoteModal, { target: document.body, props: {
              open: true, title: 'Resize regression', content: 'A note with a chat pane.',
              sessionId: 'resize-probe', onClose() {}
            }});
            window.probe = { width };
          `;
        },
        configureServer(vite) {
          vite.middlewares.use('/__dialog-resize', (_request, response) => {
            response.setHeader('Content-Type', 'text/html');
            response.end(
              '<!doctype html><html><body><script type="module" src="/@id/dialog-resize-probe"></script></body></html>'
            );
          });
        },
      },
    ],
  });
  await server.listen();
  url = `http://127.0.0.1:${server.httpServer.address().port}/__dialog-resize`;
});

after(async () => server?.close());

for (const [name, engine] of Object.entries({ chromium, webkit })) {
  describe(name, { timeout: 120_000 }, () => {
    let browser;
    let context;
    let page;
    let stored;
    let writes;
    let holdNextSave;
    let pendingSave;
    let errors;

    before(async () => {
      browser = await engine.launch({
        executablePath: process.env[`${name.toUpperCase()}_EXECUTABLE_PATH`],
      });
    });
    after(async () => browser?.close());

    async function open() {
      await context?.close();
      context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
      page = await context.newPage();
      stored = 700;
      writes = [];
      holdNextSave = false;
      pendingSave = null;
      errors = [];
      page.on('pageerror', (error) => errors.push(error.message));
      await page.route('**/api/invoke/*', async (route) => {
        const command = new URL(route.request().url()).pathname.split('/').pop();
        const args = route.request().postDataJSON();
        if (command === 'set_preference') {
          assert.equal(args.key, key);
          writes.push(args.value);
          if (holdNextSave) {
            holdNextSave = false;
            pendingSave = { route, value: args.value };
            return;
          }
          stored = args.value;
        }
        await route.fulfill({
          json: command === 'get_preference' ? stored : command === 'get_session' ? null : [],
        });
      });
      // The event stream is unrelated to these layout and HTTP ordering probes.
      await page.routeWebSocket('**/api/events*', () => {});
      await page.goto(url);
      await page.locator('[data-slot="dialog-resize-handle"]').waitFor();
      await page.evaluate(async () => {
        await Promise.all(document.getAnimations().map((animation) => animation.finished));
      });
      await page.locator('[data-slot="dialog-resize-handle"]').focus();
    }

    async function expectWidth(width, preference = width) {
      assert.equal(
        await page.locator('[data-slot="dialog-content"]').evaluate((el) => el.offsetWidth),
        width
      );
      assert.equal(await page.evaluate(() => window.probe.width.width), preference);
      assert.deepEqual(errors, []);
    }

    async function toggleDuringAnimation(label) {
      await page.getByRole('button', { name: label, exact: true }).click();
      const middle = await page.locator('[data-slot="dialog-content"]').evaluate((el) => {
        const animation = el.getAnimations().find((item) => item.transitionProperty === 'width');
        if (!animation) throw new Error('Chat toggle did not animate width');
        animation.pause();
        animation.currentTime = 35;
        return el.getBoundingClientRect().width;
      });
      assert.ok(middle > 700 && middle < 1080, `Expected intermediate chat width, got ${middle}`);
      await page.locator('[data-slot="dialog-resize-handle"]').focus();
    }

    it('keeps End at the maximum when ArrowRight follows 35ms later', async () => {
      await open();
      await page.keyboard.press('End');
      // Deliberately reproduce a second command inside the old 150ms transition.
      await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 35)));
      await page.keyboard.press('ArrowRight');
      await expectWidth(1536);
      await expect.poll(() => stored).toBe(1536);
      assert.deepEqual(writes, [1536]);
    });

    it('applies every rapid ArrowRight step without animation lag', async () => {
      await open();
      await page.evaluate(async () => {
        const handle = document.querySelector('[data-slot="dialog-resize-handle"]');
        for (let step = 0; step < 10; step++) {
          handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
          await new Promise((resolve) => setTimeout(resolve, 5));
        }
      });
      await expectWidth(860);
      await expect.poll(() => stored).toBe(860);
      assert.deepEqual(
        writes,
        Array.from({ length: 10 }, (_, step) => 716 + step * 16)
      );
      await page.keyboard.press('ArrowLeft');
      await expectWidth(844);
    });

    it('preserves chat animation and settles it before a keyboard command', async () => {
      await open();
      await toggleDuringAnimation('Show chat pane');
      await page.keyboard.press('ArrowRight');
      await expectWidth(1096, 716);
      await page.keyboard.press('Home');
      await expectWidth(1080, 700);
      await toggleDuringAnimation('Hide chat pane');
      await page.keyboard.press('ArrowRight');
      await expectWidth(716);
      await page.locator('[data-slot="dialog-resize-handle"]').dblclick();
      await expectWidth(700);
      await toggleDuringAnimation('Show chat pane');
      await page.keyboard.press('End');
      await expectWidth(1536, 1156);
    });

    it('settles chat animation before measuring a pointer gesture', async () => {
      await open();
      await toggleDuringAnimation('Show chat pane');
      const box = await page.locator('[data-slot="dialog-resize-handle"]').boundingBox();
      const x = box.x + box.width / 2;
      const y = box.y + box.height / 2;
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x + 20, y);
      await expectWidth(1120, 740);
      await page.mouse.up();
      await expect.poll(() => stored).toBe(740);
    });

    it('waits for a delayed resize save before persisting a reset, including reload', async () => {
      await open();
      holdNextSave = true;
      await page.keyboard.press('End');
      await expect.poll(() => pendingSave?.value).toBe(1536);
      await page.locator('[data-slot="dialog-resize-handle"]').dblclick();
      await expectWidth(700);
      assert.deepEqual(writes, [1536]);
      stored = pendingSave.value;
      await pendingSave.route.fulfill({ json: null });
      await expect.poll(() => stored).toBe(700);
      assert.deepEqual(writes, [1536, 700]);
      await page.reload();
      await page.locator('[data-slot="dialog-resize-handle"]').waitFor();
      await expectWidth(700);
    });

    it('persists the next choice after an HTTP save fails', async () => {
      await open();
      holdNextSave = true;
      await page.keyboard.press('ArrowRight');
      await expect.poll(() => pendingSave?.value).toBe(716);
      await page.keyboard.press('ArrowRight');
      await expectWidth(732);
      assert.deepEqual(writes, [716]);
      await pendingSave.route.fulfill({ status: 500, json: { error: 'Expected save failure' } });
      await expect.poll(() => stored).toBe(732);
      assert.deepEqual(writes, [716, 732]);
    });
  });
}
