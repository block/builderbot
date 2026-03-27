import { defineConfig } from '@playwright/test';
import * as path from 'path';
import * as os from 'os';

// Use an isolated config file so e2e tests never touch the user's real config
const e2eConfig = path.join(os.tmpdir(), 'penpal-e2e-config.json');

export default defineConfig({
  testDir: './tests',
  timeout: 30000,
  retries: 0,
  use: {
    baseURL: 'http://localhost:18923',
  },
  webServer: [
    {
      command: `cd .. && go build -o penpal-server ./cmd/penpal-server && PENPAL_CONFIG=${e2eConfig} ./penpal-server -port 18923`,
      port: 18923,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: 'cd ../frontend && VITE_API_URL=http://localhost:18923 npx vite --port 18924 --strictPort',
      port: 18924,
      reuseExistingServer: !process.env.CI,
    },
  ],
  projects: [
    {
      name: 'default',
      use: { browserName: 'chromium' },
      testIgnore: /(?:sse-refresh|review-workflow|cli-open|mermaid-comments|find-in-page)\.spec\.ts$/,
    },
    {
      // Tests that call POST /api/open broadcast "navigate" SSE events to
      // all connected browsers, redirecting other tests' pages.  Running
      // them after the parallel phase avoids cross-test interference.
      name: 'has-projects',
      use: { browserName: 'chromium' },
      testMatch: /(?:cli-open|mermaid-comments|find-in-page)\.spec\.ts$/,
      dependencies: ['default'],
    },
    {
      // MCP + SSE review pipeline: runs alone so the Go server is idle.
      name: 'review',
      use: { browserName: 'chromium' },
      testMatch: /review-workflow\.spec\.ts$/,
      dependencies: ['has-projects'],
    },
    {
      // SSE timing tests assert sub-500ms watcher→browser delivery.
      // Runs last and alone so I/O contention is zero.
      name: 'sse',
      use: { browserName: 'chromium' },
      testMatch: /sse-refresh\.spec\.ts$/,
      dependencies: ['review'],
    },
  ],
});
