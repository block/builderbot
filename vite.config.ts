import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

const port = parseInt(process.env.VITE_PORT || '5174', 10)

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  server: {
    port,
    strictPort: true,
  },
})
