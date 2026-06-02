import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

const port = parseInt(process.env.VITE_PORT || '5174', 10);
const packageJson = JSON.parse(
  readFileSync(resolve(import.meta.dirname, 'package.json'), 'utf8')
) as { version: string };

// https://vite.dev/config/
export default defineConfig({
  define: {
    __APP_VERSION__: JSON.stringify(packageJson.version),
  },
  plugins: [svelte(), tailwindcss()],
  resolve: {
    alias: {
      $lib: resolve(import.meta.dirname, 'src/lib'),
    },
  },
  server: {
    port,
    strictPort: true,
  },
});
