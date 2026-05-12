import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

const port = parseInt(process.env.VITE_PORT || '5174', 10);
const packageJson = JSON.parse(
  readFileSync(resolve(import.meta.dirname, 'package.json'), 'utf8')
) as { version: string };
const webCertPath = process.env.STAGED_WEB_CERT_PATH;
const webKeyPath = process.env.STAGED_WEB_KEY_PATH;
const webHost = process.env.STAGED_WEB_HOST;

function requireWebPath(name: string, value: string | undefined): string {
  if (!value) {
    throw new Error(`${name} must be set to enable HTTPS web mode`);
  }
  return resolve(value);
}

const webHttps =
  webCertPath || webKeyPath
    ? {
        cert: readFileSync(requireWebPath('STAGED_WEB_CERT_PATH', webCertPath)),
        key: readFileSync(requireWebPath('STAGED_WEB_KEY_PATH', webKeyPath)),
      }
    : undefined;

// https://vite.dev/config/
export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(packageJson.version),
  },
  plugins: [svelte()],
  server: {
    // Network access (0.0.0.0) is enabled via `--host` in `just dev-web`.
    // Default `dev` stays on localhost to avoid exposing the dev server.
    port,
    strictPort: true,
    https: webHttps,
    allowedHosts: webHost ? [webHost] : undefined,
    proxy: {
      '/api': {
        target: `${webHttps ? 'https' : 'http'}://localhost:5175`,
        changeOrigin: true,
        secure: false,
        ws: true, // WebSocket proxy for /api/events
      },
    },
  },
});
