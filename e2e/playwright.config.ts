import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 30000,
  retries: 0,
  use: {
    baseURL: 'http://localhost:18923',
  },
  webServer: [
    {
      command: 'cd .. && go build -o penpal . && ./penpal -port 18923',
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
