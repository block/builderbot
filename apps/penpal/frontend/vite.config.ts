import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ command }) => ({
  // In dev (Vite serves at /), base is '/'. For builds served by Go at /app/,
  // default to '/app/'. Tauri builds override with VITE_BASE='/'.
  base: command === 'serve' ? '/' : (process.env.VITE_BASE ?? '/app/'),
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      '/api': 'http://localhost:8082',
      '/events': {
        target: 'http://localhost:8082',
        configure: (proxy) => {
          proxy.on('proxyRes', (proxyRes) => {
            proxyRes.headers['cache-control'] = 'no-cache';
            proxyRes.headers['x-accel-buffering'] = 'no';
          });
        },
      },
      '/mcp': 'http://localhost:8082',
      '/static': 'http://localhost:8082',
    },
  },
  build: {
    target: ['chrome105', 'firefox121', 'safari15'],
    outDir: 'dist',
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: './src/test/setup.ts',
  },
}));
