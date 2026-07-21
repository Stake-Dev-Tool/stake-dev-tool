import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  server: {
    port: 5190,
    strictPort: true,
    // Proxy the API to the local axum server so the browser only ever talks to
    // one origin (localhost:5190). That keeps the session cookie same-origin in
    // dev exactly as it will be in production (server serves both UI and /api).
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8080',
        changeOrigin: false
      }
    }
  }
});
