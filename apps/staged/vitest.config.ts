import { resolve } from 'node:path';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { configDefaults, defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: 'unit',
          include: ['src/**/*.test.ts', '../../packages/diff-viewer/src/**/*.test.ts'],
          exclude: [...configDefaults.exclude, 'src/**/*.component.test.ts'],
        },
      },
      {
        plugins: [svelte()],
        resolve: {
          alias: {
            $lib: resolve(import.meta.dirname, 'src/lib'),
          },
          // Resolve svelte to its client build so `mount` works under jsdom.
          conditions: ['browser'],
        },
        test: {
          name: 'component',
          include: ['src/**/*.component.test.ts'],
          environment: 'jsdom',
          server: {
            deps: {
              // Svelte libraries ship uncompiled .svelte files; they must go
              // through the svelte plugin rather than be externalized to node.
              inline: ['@lucide/svelte', 'bits-ui'],
            },
          },
        },
      },
    ],
  },
});
