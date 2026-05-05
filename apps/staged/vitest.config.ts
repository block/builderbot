import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['src/**/*.test.ts', '../../packages/diff-viewer/src/**/*.test.ts'],
  },
});
