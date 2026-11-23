import { resolve } from 'node:path';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { configDefaults, defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: 'unit',
          include: [
            'src/**/*.test.ts',
            'scripts/**/*.test.ts',
            '../../packages/diff-viewer/src/**/*.test.ts',
          ],
          exclude: [...configDefaults.exclude, 'src/**/*.component.test.ts'],
          // Vitest empties every CSS module it is not told to process, a
          // `.css?raw` import included, so a source-text test would read the
          // stylesheets as ''. Raw imports are left to Vite, which hands
          // back the file's text.
          css: { include: [/\.css\?(?:.*&)?raw\b/] },
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
