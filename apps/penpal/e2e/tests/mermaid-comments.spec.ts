import { test, expect } from '@playwright/test';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

const BASE_URL = 'http://localhost:18923';

let tmpDir: string;
let projectUrl: string;
let projectName: string;
let filePath: string;

// E-PENPAL-SVG-DRAG: verifies mermaid diagram drag selection, SVG extraction, and highlighting.
test.describe('mermaid diagram commenting', () => {
  test.beforeAll(async ({ request }) => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'penpal-mermaid-e2e-'));
    const thoughtsDir = path.join(tmpDir, 'thoughts');
    fs.mkdirSync(thoughtsDir);
    const mdFile = path.join(thoughtsDir, 'diagram-doc.md');
    fs.writeFileSync(
      mdFile,
      [
        '# Diagram Test',
        '',
        'Some intro text before the diagram.',
        '',
        '```mermaid',
        'graph TD',
        '  A[Start] --> B[Middle]',
        '  B --> C[End]',
        '```',
        '',
        'Text after the diagram.',
        '',
      ].join('\n'),
    );

    const openRes = await request.post(`${BASE_URL}/api/open`, {
      data: { path: tmpDir },
    });
    expect(openRes.ok()).toBeTruthy();
    const openData = await openRes.json();
    projectUrl = openData.url;
    projectName = projectUrl.replace('/project/', '');
    filePath = 'thoughts/diagram-doc.md';
  });

  test.afterAll(async ({ request }) => {
    await request.delete(`${BASE_URL}/api/projects`, {
      data: { path: tmpDir },
    });
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  // E-PENPAL-MD-RENDER: verifies mermaid container renders with data-source-line attribute and SVG.
  test('mermaid diagram renders with data-source-line', async ({ page }) => {
    await page.goto(`/file/${projectName}/${filePath}`);
    const container = page.locator('.mermaid-container');
    await expect(container).toBeVisible({ timeout: 10000 });

    // Verify data-source-line was computed from raw markdown
    const sourceLine = await container.getAttribute('data-source-line');
    expect(sourceLine).toBeTruthy();
    expect(Number(sourceLine)).toBeGreaterThan(0);

    // Verify SVG rendered inside
    const svg = container.locator('svg');
    await expect(svg).toBeVisible();
  });

  // E-PENPAL-SVG-DRAG: verifies drag creates pending highlight rect and comment form.
  // E-PENPAL-SVG-EXTRACT: verifies SVG snippet is extracted and shown in comment form.
  test('drag on mermaid diagram creates SVG selection and comment', async ({ page }) => {
    await page.goto(`/file/${projectName}/${filePath}`);

    // Wait for mermaid to render
    const container = page.locator('.mermaid-container');
    await expect(container).toBeVisible({ timeout: 10000 });
    const svg = container.locator('svg');
    await expect(svg).toBeVisible();

    // No comments initially
    await expect(page.locator('.no-comments')).toContainText('No comments yet');

    // Perform a drag on the SVG to create a selection rectangle
    const svgBox = await svg.boundingBox();
    expect(svgBox).toBeTruthy();

    // Drag from upper-left area to lower-right area of the SVG
    const startX = svgBox!.x + svgBox!.width * 0.2;
    const startY = svgBox!.y + svgBox!.height * 0.2;
    const endX = svgBox!.x + svgBox!.width * 0.8;
    const endY = svgBox!.y + svgBox!.height * 0.8;

    await page.mouse.move(startX, startY);
    await page.mouse.down();
    // Move in steps to exceed the 5px drag threshold
    await page.mouse.move(startX + 10, startY + 10, { steps: 2 });
    await page.mouse.move(endX, endY, { steps: 5 });

    // Verify the pending highlight rect appeared in the SVG
    const pendingRect = svg.locator('.penpal-pending-svg-highlight');
    await expect(pendingRect).toBeVisible();

    await page.mouse.up();

    // Comment form should appear with SVG preview (not text quote)
    const form = page.locator('.new-thread-form');
    await expect(form).toBeVisible({ timeout: 5000 });
    // Should show SVG snippet, not plain text
    const quotedSvg = form.locator('.quoted-svg');
    await expect(quotedSvg).toBeVisible();
    await expect(quotedSvg.locator('svg')).toBeVisible();
    // Should NOT have a plain text quote
    await expect(form.locator('.quoted-text')).toHaveCount(0);

    // Fill in and submit the comment
    await form.locator('#new-thread-author').fill('diagram-tester');
    await form.locator('#new-thread-body').fill('This part of the diagram needs work.');
    await form.locator('.btn-submit').click();

    // Thread card should appear with SVG anchor (not text)
    const threadCard = page.locator('.thread-card').first();
    await expect(threadCard).toBeVisible({ timeout: 5000 });
    const svgAnchor = threadCard.locator('.thread-anchor-svg');
    await expect(svgAnchor).toBeVisible();
    await expect(svgAnchor.locator('svg')).toBeVisible();

    // Verify the comment content
    await expect(threadCard.locator('.comment-author')).toContainText('diagram-tester');
    await expect(threadCard.locator('.comment-body')).toContainText(
      'This part of the diagram needs work.',
    );
  });

  // E-PENPAL-SVG-HIGHLIGHT: verifies clicking a thread card applies and removes SVG highlight overlay.
  test('clicking thread card highlights diagram region', async ({ page, request }) => {
    await page.goto(`/file/${projectName}/${filePath}`);

    // Wait for mermaid to render
    const container = page.locator('.mermaid-container');
    await expect(container).toBeVisible({ timeout: 10000 });
    const svg = container.locator('svg');
    await expect(svg).toBeVisible();

    // Create a thread via API with svgRect
    const sourceLine = await container.getAttribute('data-source-line');
    const createRes = await request.post(`${BASE_URL}/api/threads`, {
      data: {
        project: projectName,
        path: filePath,
        anchor: {
          selectedText: '[Diagram selection]',
          svgSnippet: '<svg viewBox="10 10 100 80" width="150" height="120"><rect x="10" y="10" width="100" height="80" fill="none" stroke="black"/></svg>',
          svgRect: { x: 10, y: 10, width: 100, height: 80 },
          startLine: Number(sourceLine),
        },
        author: 'api-tester',
        role: 'human',
        body: 'API-created diagram comment.',
      },
    });
    expect(createRes.ok()).toBeTruthy();
    const createdThread = await createRes.json();
    const threadId = createdThread.id;

    // Reload to pick up the new thread
    await page.reload();
    await expect(container).toBeVisible({ timeout: 10000 });
    await expect(svg).toBeVisible();

    // Thread card should show SVG anchor
    const card = page.locator(`.thread-card[data-thread-id="${threadId}"]`);
    await expect(card.locator('.thread-anchor-svg')).toBeVisible({ timeout: 5000 });

    // Wait for persistent SVG highlight to appear (applied after mermaid async render)
    const persistentHighlight = svg.locator(`.penpal-svg-highlight[data-thread-id="${threadId}"]`);
    await expect(persistentHighlight).toBeAttached({ timeout: 10000 });

    // Click the thread card to activate it
    await card.click();

    // SVG highlight overlay should get the active class
    const highlight = svg.locator(`.penpal-svg-highlight.active[data-thread-id="${threadId}"]`);
    await expect(highlight).toBeAttached({ timeout: 3000 });

    // Highlight should lose active class after timeout (3s)
    await expect(highlight).not.toBeAttached({ timeout: 5000 });
  });

  // E-PENPAL-MD-RENDER: verifies normal text selection toolbar works alongside mermaid diagrams.
  test('normal text selection still works alongside diagrams', async ({ page }) => {
    await page.goto(`/file/${projectName}/${filePath}`);

    // Wait for mermaid to render
    await expect(page.locator('.mermaid-container')).toBeVisible({ timeout: 10000 });

    // Select text in a non-diagram paragraph
    const selectedText = 'intro text';
    await page.evaluate((text) => {
      const contentEl = document.getElementById('content')!;
      const walker = document.createTreeWalker(contentEl, NodeFilter.SHOW_TEXT);
      let node: Text | null;
      while ((node = walker.nextNode() as Text | null)) {
        const idx = node.nodeValue?.indexOf(text) ?? -1;
        if (idx >= 0) {
          const range = document.createRange();
          range.setStart(node, idx);
          range.setEnd(node, idx + text.length);
          const sel = window.getSelection()!;
          sel.removeAllRanges();
          sel.addRange(range);
          break;
        }
      }
    }, selectedText);

    // Trigger the toolbar
    await page.locator('#content').dispatchEvent('mouseup');
    const toolbar = page.locator('.selection-toolbar');
    await expect(toolbar).toBeVisible({ timeout: 5000 });
    await expect(toolbar.locator('button', { hasText: 'Comment' })).toBeVisible();
  });
});
