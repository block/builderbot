import { test, expect } from '@playwright/test';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { MCPClient } from '../helpers/mcp-client';
import { blockPendingNavigation } from '../helpers/fixtures';
import { generateMarkdownDocument } from '../helpers/markdown-generator';

const BASE_URL = 'http://localhost:18923';
test.use({ baseURL: 'http://localhost:18924' });

// Read configuration from environment
const ITERATION = parseInt(process.env.STABILITY_ITERATION ?? '0', 10);
const RESULTS_DIR =
  process.env.STABILITY_RESULTS_DIR ??
  path.join(__dirname, '..', 'anchor-stability', 'results');
const SEED = parseInt(
  process.env.STABILITY_SEED ?? String(42 + ITERATION * 1000),
  10,
);
const REPETITIVE = process.env.STABILITY_REPETITIVE !== '0'; // Default to repetitive (PENPAL-46)
const RESULTS_FILE = path.join(RESULTS_DIR, 'results.json');
const SCREENSHOTS_DIR = path.join(RESULTS_DIR, 'screenshots');
const NUM_TESTS = 10;

// ── Result types ──────────────────────────────────────────────────────

interface TestScores {
  initial: number;
  editBefore: number;
  editAfter: number;
  editWithin: number;
}

interface TestResult {
  testIndex: number;
  iteration: number;
  anchorType: string;
  selectionType: 'single-element' | 'cross-element'; // PENPAL-38
  sizeClass: 'small' | 'large'; // PENPAL-46
  selectedText: string;
  scores: TestScores;
  total: number;
  screenshots: Record<string, string>;
  timestamp: string;
  details: string;
  analysis?: string;
  durationMs: number;
  phaseDurations: Record<string, number>;
}

interface IterationResult {
  iteration: number;
  tests: TestResult[];
  totalScore: number;
  timestamp: string;
  status: 'running' | 'done';
  durationMs?: number;
  startedAt?: string;
}

interface Improvement {
  afterIteration: number;
  type: 'production' | 'test' | 'dashboard';
  description: string;
  linearIssue?: string;
}

interface AllResults {
  iterations: IterationResult[];
  improvements: Improvement[];
  currentIteration: number;
  currentTest: number;
}

// ── Helpers ───────────────────────────────────────────────────────────

function readResults(): AllResults {
  try {
    return JSON.parse(fs.readFileSync(RESULTS_FILE, 'utf-8'));
  } catch {
    return { iterations: [], improvements: [], currentIteration: ITERATION, currentTest: -1 };
  }
}

function writeResults(results: AllResults) {
  fs.mkdirSync(RESULTS_DIR, { recursive: true });
  fs.mkdirSync(SCREENSHOTS_DIR, { recursive: true });
  fs.writeFileSync(RESULTS_FILE, JSON.stringify(results, null, 2));
}

function ensureIteration(results: AllResults): IterationResult {
  while (results.iterations.length <= ITERATION) {
    const iter: IterationResult = {
      iteration: results.iterations.length,
      tests: [],
      totalScore: 0,
      timestamp: new Date().toISOString(),
      status: 'running',
      startedAt: new Date().toISOString(),
    };
    results.iterations.push(iter);
  }
  return results.iterations[ITERATION];
}

/** Insert 1-3 new paragraph lines at a random position above anchorLine. */
function editBefore(
  markdown: string,
  anchorLine: number,
  testIdx: number,
): string {
  const lines = markdown.split('\n');
  // Pick a line in the first half above the anchor (at least line 2 to avoid title)
  const insertAt = Math.max(2, Math.floor(Math.random() * Math.max(1, anchorLine - 2)));
  const newLines = [
    '',
    `Inserted paragraph before test ${testIdx} at position ${insertAt} during iteration ${ITERATION}. This text exists solely to shift subsequent line numbers downward.`,
    '',
  ];
  lines.splice(insertAt, 0, ...newLines);
  return lines.join('\n');
}

/** Insert 1-3 new paragraph lines at a random position below anchorLine. */
function editAfter(
  markdown: string,
  anchorLine: number,
  testIdx: number,
): string {
  const lines = markdown.split('\n');
  const insertAt = Math.min(
    lines.length,
    anchorLine + 2 + Math.floor(Math.random() * 5),
  );
  const newLines = [
    '',
    `Inserted paragraph after test ${testIdx} at position ${insertAt} during iteration ${ITERATION}. This validates that edits below an anchor do not disturb it.`,
    '',
  ];
  lines.splice(insertAt, 0, ...newLines);
  return lines.join('\n');
}

/** Modify the line containing the anchored text. */
function editWithin(
  markdown: string,
  anchorLine: number,
  selectedText: string,
  testIdx: number,
): string {
  const lines = markdown.split('\n');
  const lineIdx = anchorLine - 1;
  if (lineIdx >= 0 && lineIdx < lines.length) {
    // Append text to the line (don't replace the selectedText entirely)
    lines[lineIdx] = lines[lineIdx] + ` [edit-${testIdx}]`;
  }
  return lines.join('\n');
}

/** Pick a random span of the document for anchoring (PENPAL-46).
 *  Size classes: "small" (30-100 chars) and "large" (20-30% of document).
 *  Start and end positions are random, ignoring markdown element boundaries.
 *  Returns { text, isCrossElement, sizeClass, anchorType, startLine }. */
function pickRandomSelection(
  rawMarkdown: string,
  rng: () => number,
): { text: string; isCrossElement: boolean; sizeClass: 'small' | 'large'; anchorType: string; startLine: number } {
  const isLarge = rng() < 0.5;
  const docLength = rawMarkdown.length;

  let targetLength: number;
  if (isLarge) {
    // Large: 20-30% of document
    targetLength = Math.floor(docLength * (0.20 + rng() * 0.10));
  } else {
    // Small: 30-100 chars
    targetLength = 30 + Math.floor(rng() * 70);
  }

  // Pick random start position, ensuring at least 15 chars remain on the
  // starting line so the rehype prefix fallback has enough text to match.
  const margin = Math.min(30, Math.floor(docLength * 0.02));
  const maxStart = Math.max(margin, docLength - targetLength - margin);
  let startPos = margin + Math.floor(rng() * Math.max(1, maxStart - margin));

  // Snap start forward if too close to end of line (avoids tiny prefix in cross-element)
  const nextNewline = rawMarkdown.indexOf('\n', startPos);
  if (nextNewline !== -1 && nextNewline - startPos < 15) {
    // Skip past the newline and any blank lines / block markers to the next content
    let scan = nextNewline + 1;
    while (scan < docLength && /[\n\s]/.test(rawMarkdown[scan])) scan++;
    // Also skip past block markers on the new line
    const lineStart = rawMarkdown.lastIndexOf('\n', scan - 1) + 1;
    const lineContent = rawMarkdown.slice(lineStart, lineStart + 20);
    const markerMatch = lineContent.match(/^(?:#{1,6} |- |\* |\d+\. |> |- \[[ x]\] |```)/);
    if (markerMatch) scan = lineStart + markerMatch[0].length;
    if (scan < docLength - targetLength) startPos = scan;
  }

  const endPos = Math.min(startPos + targetLength, docLength - 1);

  const text = rawMarkdown.slice(startPos, endPos);

  // Compute start line (1-based) from character offset
  const startLine = rawMarkdown.slice(0, startPos).split('\n').length;

  // Determine anchor type from the line at the start position
  const lines = rawMarkdown.split('\n');
  const startLineContent = lines[startLine - 1] || '';
  let anchorType = 'paragraph';
  if (/^#{1,6} /.test(startLineContent)) anchorType = 'heading';
  else if (/^[-*] /.test(startLineContent)) anchorType = 'listItem';
  else if (/^> /.test(startLineContent)) anchorType = 'blockquote';
  else if (/^```/.test(startLineContent)) anchorType = 'codeBlock';
  else if (/^\|/.test(startLineContent)) anchorType = 'table';
  else if (/^\d+\. /.test(startLineContent)) anchorType = 'listItem';

  return {
    text,
    isCrossElement: text.includes('\n'),
    sizeClass: isLarge ? 'large' : 'small',
    anchorType,
    startLine,
  };
}

/** Normalize text the same way the rehype plugin does: strip formatting chars and collapse whitespace. */
function normalizeForComparison(s: string): string {
  return s.replace(/[*_`]/g, '').replace(/\s+/g, ' ').trim();
}

/** Check that the highlight's visible text overlaps with the expected selectedText.
 *  Returns true if the mark text is a substring of the normalized selectedText
 *  or vice versa (cross-element selections may only highlight a prefix). */
async function verifyHighlightText(
  page: import('@playwright/test').Page,
  highlightSelector: string,
  selectedText: string,
): Promise<{ correct: boolean; markText: string }> {
  const markText = await page.locator(highlightSelector).first()
    .textContent()
    .catch(() => null);
  if (!markText) return { correct: false, markText: '' };
  const normMark = normalizeForComparison(markText);
  const normSelected = normalizeForComparison(selectedText);
  // The mark text should be a meaningful substring of the selectedText (or contain it).
  // For cross-element fallback, the mark covers only a prefix, so check both directions.
  const correct = normSelected.includes(normMark) || normMark.includes(normSelected);
  return { correct, markText: normMark.slice(0, 80) };
}

// ── Test suite ────────────────────────────────────────────────────────

let tmpDir: string;
let projectName: string;
let filePath: string;
let absFilePath: string;
let mcp: MCPClient;
let doc: ReturnType<typeof generateMarkdownDocument>;

// Simple seeded PRNG for test-level randomness (same algorithm as generator)
let testRng: () => number;
function createTestRng(seed: number) {
  let s = seed | 0;
  return () => {
    s = (s + 0x6d2b79f5) | 0;
    let t = Math.imul(s ^ (s >>> 15), 1 | s);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

test.describe.configure({ mode: 'serial' });
test.describe(`anchor stability - iteration ${ITERATION}`, () => {
  test.beforeAll(async ({ request }) => {
    // Generate the document (PENPAL-41: repetitive mode for duplicate content testing)
    doc = generateMarkdownDocument(SEED, { repetitive: REPETITIVE });
    testRng = createTestRng(SEED + 999);

    // Create temp project
    tmpDir = fs.mkdtempSync(
      path.join(os.tmpdir(), `penpal-anchor-stability-${ITERATION}-`),
    );
    const thoughtsDir = path.join(tmpDir, 'thoughts');
    fs.mkdirSync(thoughtsDir);
    filePath = 'thoughts/stability-test.md';
    absFilePath = path.join(tmpDir, filePath);
    fs.writeFileSync(absFilePath, doc.markdown);

    // Register project
    const openRes = await request.post(`${BASE_URL}/api/open`, {
      data: { path: tmpDir },
    });
    expect(openRes.ok()).toBeTruthy();
    const openData = await openRes.json();
    projectName = openData.url.replace('/project/', '');

    // Clear pending navigation
    await request.get(`${BASE_URL}/api/navigate`);

    // Init MCP client
    mcp = new MCPClient();
    await mcp.initialize();

    // Ensure results dir exists
    fs.mkdirSync(SCREENSHOTS_DIR, { recursive: true });
  });

  test.afterAll(async ({ request }) => {
    // Mark iteration as done with timing
    const results = readResults();
    const iter = ensureIteration(results);
    iter.status = 'done';
    iter.totalScore = iter.tests.reduce((sum, t) => sum + t.total, 0);
    if (iter.startedAt) {
      iter.durationMs = Date.now() - new Date(iter.startedAt).getTime();
    }
    writeResults(results);

    // Cleanup
    await request.delete(`${BASE_URL}/api/projects`, {
      data: { path: tmpDir },
    });
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  for (let testIdx = 0; testIdx < NUM_TESTS; testIdx++) {
    test(`test ${testIdx + 1}/${NUM_TESTS}`, async ({ page }) => {
      const testStart = Date.now();
      const phaseDurations: Record<string, number> = {};
      await blockPendingNavigation(page);

      // Reset file to original content at the start of each test to avoid
      // accumulated [edit-N] modifications from previous tests.
      fs.writeFileSync(absFilePath, doc.markdown);

      // ── Pick a random document span (PENPAL-46) ──────────────────
      const selection = pickRandomSelection(doc.markdown, testRng);
      const selectedText = selection.text;
      const selectionType = selection.isCrossElement ? 'cross-element' as const : 'single-element' as const;
      const sizeClass = selection.sizeClass;

      const scores: TestScores = {
        initial: 0,
        editBefore: 0,
        editAfter: 0,
        editWithin: 0,
      };
      const screenshots: Record<string, string> = {};
      const details: string[] = [];

      // ── Step 1: Create thread via MCP ──────────────────────────
      let threadId: string | null = null;
      try {
        const result = (await mcp.callTool('penpal_create_thread', {
          project: projectName,
          path: filePath,
          selectedText,
          body: `Stability test ${testIdx + 1}: anchored to "${selectedText.slice(0, 30)}..."`,
        })) as { content?: Array<{ text?: string }> };

        const text = result?.content?.[0]?.text;
        if (text) {
          const parsed = JSON.parse(text);
          threadId = parsed.id;
        }
      } catch (err) {
        details.push(`Thread creation failed: ${err}`);
      }

      if (!threadId) {
        details.push('No thread ID — skipping all validations');
        const testResult = buildResult(testIdx, selection.anchorType, selectedText, selectionType, sizeClass, scores, screenshots, details, Date.now() - testStart, phaseDurations, doc.markdown);
        saveTestResult(testResult);
        return;
      }

      // ── Step 2: Navigate and validate initial highlight ────────
      let phaseStart = Date.now();
      const focusReady = page.waitForResponse(
        (resp) => resp.url().includes('/api/focus') && resp.ok(),
      );
      await page.goto(`/file/${projectName}/${filePath}`);
      const content = page.locator('#content');
      await expect(content).toBeVisible({ timeout: 10000 });
      await focusReady;

      const highlightSelector = `.comment-highlight[data-thread-id="${threadId}"]`;
      const highlightLocator = page.locator(highlightSelector).first();

      // Wait for threads API response before checking highlight
      await page.waitForResponse(
        (resp) => resp.url().includes('/api/threads') && resp.ok(),
        { timeout: 5000 },
      ).catch(() => {});

      const initialVisible = await expect(highlightLocator)
        .toBeVisible({ timeout: 5000 })
        .then(() => true)
        .catch(() => false);

      if (initialVisible) {
        const { correct, markText } = await verifyHighlightText(page, highlightSelector, selectedText);
        if (correct) {
          scores.initial = 2;
          details.push('Initial highlight: PASS');
        } else {
          details.push(`Initial highlight: FAIL — visible but wrong text. mark="${markText}"`);
        }
      } else {
        // Enhanced diagnostics for initial highlight failures
        const mdIdx = doc.markdown.indexOf(selectedText);
        const occurrences = doc.markdown.split(selectedText).length - 1;
        const allHighlights = await page.locator('.comment-highlight').count();
        const threadHighlight = await page.locator(highlightSelector).count();
        details.push(`Initial highlight: FAIL — highlight not visible after creation. ` +
          `textInMarkdown: ${mdIdx >= 0 ? `yes@${mdIdx}` : 'NO'}, occurrences: ${occurrences}, ` +
          `allHighlights: ${allHighlights}, threadHighlight: ${threadHighlight}`);
      }
      phaseDurations.initial = Date.now() - phaseStart;

      // Scroll to highlight before screenshot (PENPAL-24)
      await scrollToHighlight(page, threadId);
      const ssInitial = `iter${ITERATION}_test${testIdx}_initial.png`;
      await page.screenshot({ path: path.join(SCREENSHOTS_DIR, ssInitial) });
      screenshots.initial = ssInitial;

      // ── Step 3: Edit BEFORE anchor ─────────────────────────────
      phaseStart = Date.now();
      // Snapshot the baseline — each phase resets to this (PENPAL-22)
      const baselineMarkdown = fs.readFileSync(absFilePath, 'utf-8');
      const anchorLine = await getAnchorLine(page, threadId);

      const afterEditBefore = editBefore(baselineMarkdown, anchorLine, testIdx);
      fs.writeFileSync(absFilePath, afterEditBefore);
      await page.waitForTimeout(300);

      const editBeforeVisible = await expect(highlightLocator)
        .toBeVisible({ timeout: 3000 })
        .then(() => true)
        .catch(() => false);

      if (editBeforeVisible) {
        const { correct, markText } = await verifyHighlightText(page, highlightSelector, selectedText);
        if (correct) {
          scores.editBefore = 2;
          details.push('Edit before: PASS');
        } else {
          details.push(`Edit before: FAIL — visible but wrong text. mark="${markText}"`);
        }
      } else {
        details.push('Edit before: FAIL — highlight disappeared after inserting lines above');
      }
      phaseDurations.editBefore = Date.now() - phaseStart;

      await scrollToHighlight(page, threadId);
      const ssBefore = `iter${ITERATION}_test${testIdx}_editBefore.png`;
      await page.screenshot({ path: path.join(SCREENSHOTS_DIR, ssBefore) });
      screenshots.editBefore = ssBefore;

      // ── Step 4: Edit AFTER anchor ──────────────────────────────
      phaseStart = Date.now();
      fs.writeFileSync(absFilePath, baselineMarkdown);
      await page.waitForTimeout(300);

      const afterEditAfter = editAfter(baselineMarkdown, anchorLine, testIdx);
      fs.writeFileSync(absFilePath, afterEditAfter);
      await page.waitForTimeout(300);

      const editAfterVisible = await expect(highlightLocator)
        .toBeVisible({ timeout: 3000 })
        .then(() => true)
        .catch(() => false);

      if (editAfterVisible) {
        const { correct, markText } = await verifyHighlightText(page, highlightSelector, selectedText);
        if (correct) {
          scores.editAfter = 2;
          details.push('Edit after: PASS');
        } else {
          details.push(`Edit after: FAIL — visible but wrong text. mark="${markText}"`);
        }
      } else {
        details.push('Edit after: FAIL — highlight disappeared after inserting lines below');
      }
      phaseDurations.editAfter = Date.now() - phaseStart;

      await scrollToHighlight(page, threadId);
      const ssAfter = `iter${ITERATION}_test${testIdx}_editAfter.png`;
      await page.screenshot({ path: path.join(SCREENSHOTS_DIR, ssAfter) });
      screenshots.editAfter = ssAfter;

      // ── Step 5: Edit WITHIN anchor ─────────────────────────────
      phaseStart = Date.now();
      fs.writeFileSync(absFilePath, baselineMarkdown);
      await page.waitForTimeout(300);

      const afterEditWithin = editWithin(baselineMarkdown, anchorLine, selectedText, testIdx);
      fs.writeFileSync(absFilePath, afterEditWithin);
      await page.waitForTimeout(300);

      const editWithinVisible = await expect(highlightLocator)
        .toBeVisible({ timeout: 3000 })
        .then(() => true)
        .catch(() => false);

      if (editWithinVisible) {
        // Skip text verification — the edit intentionally modified the anchor line content
        scores.editWithin = 1;
        details.push('Edit within: PASS');
      } else {
        details.push('Edit within: FAIL — highlight disappeared after modifying anchored text');
      }
      phaseDurations.editWithin = Date.now() - phaseStart;

      await scrollToHighlight(page, threadId);
      const ssWithin = `iter${ITERATION}_test${testIdx}_editWithin.png`;
      await page.screenshot({ path: path.join(SCREENSHOTS_DIR, ssWithin) });
      screenshots.editWithin = ssWithin;

      // ── Save results ───────────────────────────────────────────
      const testResult = buildResult(testIdx, selection.anchorType, selectedText, selectionType, sizeClass, scores, screenshots, details, Date.now() - testStart, phaseDurations, doc.markdown);
      saveTestResult(testResult);
    });
  }
});

// ── Utility functions ─────────────────────────────────────────────────

function buildResult(
  testIdx: number,
  anchorType: string,
  selectedText: string,
  selectionType: 'single-element' | 'cross-element',
  sizeClass: 'small' | 'large',
  scores: TestScores,
  screenshots: Record<string, string>,
  details: string[],
  durationMs: number,
  phaseDurations: Record<string, number>,
  rawMarkdown: string,
): TestResult {
  const total =
    scores.initial + scores.editBefore + scores.editAfter + scores.editWithin;
  const result: TestResult = {
    testIndex: testIdx,
    iteration: ITERATION,
    anchorType,
    selectionType,
    sizeClass,
    selectedText: selectedText.slice(0, 80),
    scores,
    total,
    screenshots,
    timestamp: new Date().toISOString(),
    details: details.join('\n'),
    durationMs,
    phaseDurations,
  };
  // Analyze imperfect tests (PENPAL-35)
  if (total < 7) {
    result.analysis = analyzeFailure(scores, selectedText, anchorType, rawMarkdown);
  }
  return result;
}

/** Produce a brief analysis of why a test scored less than perfect (PENPAL-35). */
function analyzeFailure(
  scores: TestScores,
  selectedText: string,
  anchorType: string,
  rawMarkdown: string,
): string {
  const reasons: string[] = [];

  const isCrossElement = selectedText.includes('\n');
  const hasMarkdownFormatting = /[*_`]/.test(selectedText);
  const foundInMarkdown = rawMarkdown.includes(selectedText);

  if (!foundInMarkdown) {
    reasons.push('selectedText not found in raw markdown — server-side ResolveAnchor will fail.');
    if (isCrossElement) {
      reasons.push('This is a cross-element selection (contains newlines). The text between elements may not match exactly.');
    }
    if (hasMarkdownFormatting) {
      reasons.push('selectedText contains inline formatting (*, _, `). If extracted from rendered text, the raw markdown won\'t match.');
    }
  }

  if (scores.initial === 0) {
    if (foundInMarkdown) {
      reasons.push('Initial highlight failed despite text being in markdown. Possible causes: rehype plugin didn\'t find the text in rendered elements, timing issue, or startLine mismatch.');
      if (isCrossElement) {
        reasons.push('Cross-element selection — rehype plugin may not highlight text spanning multiple block elements.');
      }
      if (hasMarkdownFormatting) {
        reasons.push('Inline formatting in selectedText may not match stripped text in rendered HAST nodes.');
      }
    }
  } else {
    if (scores.editBefore === 0) {
      reasons.push('Edit-before failed — lines inserted above anchor may have caused startLine drift or text matching failure.');
    }
    if (scores.editAfter === 0) {
      reasons.push('Edit-after failed — unusual, as edits below should not affect the anchor.');
    }
    if (scores.editWithin === 0) {
      reasons.push('Edit-within failed — expected, as modifying the anchored line changes the text that selectedText must match.');
    }
  }

  return reasons.join(' ') || 'Unknown failure cause.';
}

function saveTestResult(testResult: TestResult) {
  const results = readResults();
  const iter = ensureIteration(results);
  iter.tests.push(testResult);
  iter.totalScore = iter.tests.reduce((sum, t) => sum + t.total, 0);
  results.currentIteration = ITERATION;
  results.currentTest = testResult.testIndex;
  writeResults(results);
}

/** Scroll the page so the highlight is visible before taking a screenshot (PENPAL-24). */
async function scrollToHighlight(page: import('@playwright/test').Page, threadId: string) {
  await page.evaluate((tid) => {
    const mark = document.querySelector(`.comment-highlight[data-thread-id="${tid}"]`);
    if (mark) {
      mark.scrollIntoView({ block: 'center', behavior: 'instant' });
    }
  }, threadId);
}

/** Try to get the anchor's resolved line from the page's rendered data-source-line attributes. */
async function getAnchorLine(page: import('@playwright/test').Page, threadId: string): Promise<number> {
  try {
    const line = await page.evaluate((tid) => {
      const mark = document.querySelector(`.comment-highlight[data-thread-id="${tid}"]`);
      if (!mark) return -1;
      // Walk up to find the nearest element with data-source-line
      let el: Element | null = mark;
      while (el) {
        const attr = el.getAttribute?.('data-source-line');
        if (attr) return parseInt(attr, 10);
        el = el.parentElement;
      }
      return -1;
    }, threadId);
    return line;
  } catch {
    return -1;
  }
}
