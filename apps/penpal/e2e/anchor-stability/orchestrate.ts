#!/usr/bin/env npx tsx
/**
 * Anchor Stability Test Orchestrator
 *
 * Drives the test-improve loop:
 * 1. Start dashboard server
 * 2. For each iteration (up to 3):
 *    a. Run anchor stability Playwright tests
 *    b. Score the results
 *    c. If score < 69 and not last iteration, pause for improvements
 * 3. Print final summary
 *
 * Usage:
 *   npx tsx e2e/anchor-stability/orchestrate.ts [--manual]
 *
 * Options:
 *   --manual  Pause between iterations for manual code improvements
 */

import { execSync, spawn, type ChildProcess } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import * as readline from 'readline';

const E2E_DIR = path.resolve(__dirname, '..');
const RESULTS_DIR = path.resolve(__dirname, 'results');
const RESULTS_FILE = path.join(RESULTS_DIR, 'results.json');
const SCREENSHOTS_DIR = path.join(RESULTS_DIR, 'screenshots');
const MAX_ITERATIONS = 3;
const PASS_THRESHOLD = 69;
const MANUAL_MODE = process.argv.includes('--manual');

interface AllResults {
  iterations: Array<{
    iteration: number;
    tests: Array<{ total: number; scores: Record<string, number> }>;
    totalScore: number;
    status: string;
  }>;
  improvements: Array<{
    afterIteration: number;
    type: 'production' | 'test' | 'dashboard';
    description: string;
    linearIssue?: string;
  }>;
  currentIteration: number;
  currentTest: number;
}

function readResults(): AllResults {
  try {
    return JSON.parse(fs.readFileSync(RESULTS_FILE, 'utf-8'));
  } catch {
    return { iterations: [], currentIteration: 0, currentTest: -1 };
  }
}

function waitForEnter(prompt: string): Promise<void> {
  return new Promise((resolve) => {
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
    });
    rl.question(prompt, () => {
      rl.close();
      resolve();
    });
  });
}

function startDashboard(): ChildProcess {
  const child = spawn('npx', ['tsx', path.resolve(__dirname, 'serve-dashboard.ts')], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env },
  });
  child.stdout?.on('data', (d) => process.stdout.write(d));
  child.stderr?.on('data', (d) => process.stderr.write(d));
  return child;
}

function runPlaywrightTests(iteration: number): boolean {
  try {
    execSync(
      `npx playwright test --project=anchor-stability`,
      {
        cwd: E2E_DIR,
        env: {
          ...process.env,
          STABILITY_ITERATION: String(iteration),
          STABILITY_RESULTS_DIR: RESULTS_DIR,
        },
        stdio: 'inherit',
        timeout: 300_000, // 5 minutes
      },
    );
    return true;
  } catch {
    // Playwright exits non-zero if tests fail, which is expected
    return false;
  }
}

async function main() {
  console.log('=== Anchor Stability Test Orchestrator ===\n');

  // Ensure clean results directory
  fs.mkdirSync(SCREENSHOTS_DIR, { recursive: true });
  fs.writeFileSync(
    RESULTS_FILE,
    JSON.stringify({ iterations: [], currentIteration: 0, currentTest: -1 }, null, 2),
  );

  // Start dashboard
  const dashboard = startDashboard();
  console.log('Dashboard starting at http://localhost:18950\n');

  // Give the server a moment to start
  await new Promise((r) => setTimeout(r, 1000));

  try {
    for (let iter = 0; iter < MAX_ITERATIONS; iter++) {
      console.log(`\n${'='.repeat(60)}`);
      console.log(`  ITERATION ${iter + 1}/${MAX_ITERATIONS}`);
      console.log(`${'='.repeat(60)}\n`);

      runPlaywrightTests(iter);

      // Read and display results
      const results = readResults();
      const iterResult = results.iterations[iter];

      if (!iterResult) {
        console.error(`No results found for iteration ${iter}`);
        continue;
      }

      const score = iterResult.totalScore;
      console.log(`\n  Score: ${score}/${MAX_ITERATIONS * 7 * 10}`);
      console.log(`  ${score >= PASS_THRESHOLD ? 'PASS' : 'FAIL'} (threshold: ${PASS_THRESHOLD})\n`);

      // Print per-test breakdown
      console.log('  Test breakdown:');
      for (const test of iterResult.tests) {
        const s = test.scores;
        const phases = [
          `init:${s.initial}/2`,
          `before:${s.editBefore}/2`,
          `after:${s.editAfter}/2`,
          `within:${s.editWithin}/1`,
        ].join('  ');
        console.log(`    Test ${(test as { testIndex?: number }).testIndex ?? '?'}: ${test.total}/7  [${phases}]`);
      }

      if (score >= PASS_THRESHOLD) {
        console.log(`\n  Score meets threshold (${PASS_THRESHOLD}). Test passed!`);
        if (iter < MAX_ITERATIONS - 1) {
          console.log('  Continuing to next iteration for comparison...\n');
        }
      } else if (iter < MAX_ITERATIONS - 1) {
        console.log(`\n  Score below ${PASS_THRESHOLD}. Improvements needed.`);

        if (MANUAL_MODE) {
          console.log('  Make code improvements, rebuild the Go server, then press Enter.');
          await waitForEnter('  Press Enter to continue to next iteration... ');
        } else {
          console.log('  Pausing for improvements. Run with --manual for interactive mode.');
          console.log('  Or make improvements and re-run the orchestrator.\n');
          break;
        }
      } else {
        console.log(`\n  Final iteration complete. Score: ${score}/${MAX_ITERATIONS * 7 * 10}`);
      }
    }
  } finally {
    // Print final summary
    const finalResults = readResults();
    console.log(`\n${'='.repeat(60)}`);
    console.log('  FINAL SUMMARY');
    console.log(`${'='.repeat(60)}`);
    for (const iter of finalResults.iterations) {
      const status = iter.totalScore >= PASS_THRESHOLD ? 'PASS' : 'FAIL';
      console.log(`  Iteration ${iter.iteration + 1}: ${iter.totalScore}/70 [${status}]`);
    }
    console.log(`\n  Dashboard: http://localhost:18950`);
    console.log('  Press Ctrl+C to stop the dashboard server.\n');

    // Keep dashboard running until user kills it
    await new Promise<void>((resolve) => {
      process.on('SIGINT', () => {
        dashboard.kill();
        resolve();
      });
    });
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
