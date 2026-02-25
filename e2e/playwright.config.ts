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
      command: `cd .. && go build -o penpal . && PENPAL_CONFIG=${e2eConfig} ./penpal -port 18923`,
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
    { name: 'chromium', use: { browserName: 'chromium' } },
  ],
});
