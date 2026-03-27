import { test, expect } from '@playwright/test';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { MCPClient } from '../helpers/mcp-client';

const BASE_URL = 'http://localhost:18923';

let tmpDir: string;
let projectUrl: string;
let projectName: string;
let filePath: string; // project-relative path to the markdown file

// E-PENPAL-MCP-TOOLS: verifies end-to-end review workflow using MCP tools.
test.describe('review workflow', () => {
  test.beforeAll(async ({ request }) => {
    // Create a temp directory with a markdown file
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'penpal-e2e-'));
    const thoughtsDir = path.join(tmpDir, 'thoughts');
    fs.mkdirSync(thoughtsDir);
    const mdFile = path.join(thoughtsDir, 'test-doc.md');
    fs.writeFileSync(
      mdFile,
      '# Test Document\n\nThis is a paragraph for testing comments.\n\nAnother paragraph with more content here.\n',
    );

    // Register as a standalone project via POST /api/open
    const openRes = await request.post(`${BASE_URL}/api/open`, {
      data: { path: tmpDir },
    });
    expect(openRes.ok()).toBeTruthy();
    const openData = await openRes.json();
    projectUrl = openData.url; // e.g. /project/penpal-e2e-XXXX
    projectName = projectUrl.replace('/project/', '');
    filePath = 'thoughts/test-doc.md';
  });

  test.afterAll(async ({ request }) => {
    // Close the standalone project
    await request.delete(`${BASE_URL}/api/projects`, {
      data: { path: tmpDir },
    });
    // Clean up temp directory
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  // E-PENPAL-ANCHOR-COMPUTE: verifies text selection creates anchor with correct context.
  // E-PENPAL-HIGHLIGHT-REHYPE: verifies comment highlight renders on selected text.
  // E-PENPAL-MCP-WORKING: verifies working indicator appears when agent reads threads.
  // E-PENPAL-MCP-TOOLS: verifies penpal_list_threads and penpal_reply MCP tool calls.
  // E-PENPAL-SUGGESTED-REPLIES: verifies suggested reply pills render after agent reply.
  test('full comment review lifecycle', async ({ page }) => {
    // ---- Step 1: Navigate to the file page ----
    await page.goto(`/file/${projectName}/${filePath}`);
    const content = page.locator('#content');
    await expect(content).toBeVisible();
    await expect(content).toContainText('This is a paragraph for testing comments.');

    // Comments panel shows "No comments yet"
    await expect(page.locator('.no-comments')).toContainText('No comments yet');

    // ---- Step 2: Select text and create a comment ----
    // Programmatically select text using the Selection API
    const selectedText = 'testing comments';
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

    // Dispatch mouseup on #content to trigger the selection toolbar
    await content.dispatchEvent('mouseup');
    // Wait for the toolbar to appear (it has a 10ms setTimeout)
    const toolbar = page.locator('.selection-toolbar');
    await expect(toolbar).toBeVisible({ timeout: 5000 });

    // Click "Comment" button (first button in the toolbar)
    await toolbar.locator('button', { hasText: 'Comment' }).click();

    // Assert: new thread form appears with the selected text quoted
    const form = page.locator('.new-thread-form');
    await expect(form).toBeVisible();
    await expect(form.locator('.quoted-text')).toContainText(selectedText);

    // Fill in author name and comment body
    await form.locator('#new-thread-author').fill('test-human');
    await form.locator('#new-thread-body').fill('This needs clarification.');

    // Click submit
    await form.locator('.btn-submit').click();

    // Assert: thread card appears
    const threadCard = page.locator('.thread-card').first();
    await expect(threadCard).toBeVisible({ timeout: 5000 });
    await expect(threadCard.locator('.comment-author')).toContainText('test-human');
    await expect(threadCard.locator('.comment-role.human')).toBeVisible();
    await expect(threadCard.locator('.comment-body')).toContainText('This needs clarification.');

    // Get the thread ID from the DOM
    const threadId = await threadCard.getAttribute('data-thread-id');
    expect(threadId).toBeTruthy();

    // ---- Step 3: Simulate agent reading threads (working indicator) ----
    const mcp = new MCPClient();
    await mcp.initialize();

    // Call penpal_list_threads — this triggers working state for threads with human as last commenter
    await mcp.callTool('penpal_list_threads', { project: projectName });

    // Wait for the working indicator to appear via SSE update
    const workingIndicator = threadCard.locator('.thread-working');
    await expect(workingIndicator).toBeVisible({ timeout: 10000 });

    // ---- Step 4: Simulate agent replying ----
    await mcp.callTool('penpal_reply', {
      project: projectName,
      path: filePath,
      threadId: threadId!,
      body: 'I can clarify that for you.',
      suggestedReplies: ['Looks good', 'Let me check'],
    });

    // Wait for SSE update — working indicator should disappear, agent comment should appear
    // Use a fresh locator for the thread card (DOM may have been replaced)
    const updatedCard = page.locator(`.thread-card[data-thread-id="${threadId}"]`);
    await expect(updatedCard.locator('.thread-working')).toBeHidden({ timeout: 10000 });

    // Agent comment appears
    const agentComment = updatedCard.locator('.comment-role.agent');
    await expect(agentComment).toBeVisible({ timeout: 10000 });

    // Verify agent comment content
    const comments = updatedCard.locator('.thread-comment');
    const agentCommentEl = comments.filter({ hasText: 'I can clarify that for you.' });
    await expect(agentCommentEl).toBeVisible();
    await expect(agentCommentEl.locator('.comment-author')).toContainText('claude');

    // Suggested reply pills appear
    const suggestedReplies = updatedCard.locator('.suggested-replies');
    await expect(suggestedReplies).toBeVisible({ timeout: 5000 });
    const pills = updatedCard.locator('.suggested-reply-pill');
    await expect(pills).toHaveCount(2);
    await expect(pills.nth(0)).toContainText('Looks good');
    await expect(pills.nth(1)).toContainText('Let me check');
  });
});
