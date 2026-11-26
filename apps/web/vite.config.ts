/// <reference types="node" />
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';

for (const key of ['VITE_CUBIE_PUBLIC_URL', 'VITE_CUBIE_LEGACY_URL']) {
  const value = (process.env[key] ?? '').replace(/\/$/, '');
  if (value) {
    const url = new URL(value);
    if (url.protocol !== 'https:' || url.origin !== value) {
      throw new Error(`${key} must be an HTTPS origin`);
    }
  }
  process.env[key] = value;
}

export default defineConfig({
  plugins: [svelte()],
  build: {
    rolldownOptions: {
      input: ['index.html', 'legacy-entry.html'].map(page => fileURLToPath(new URL(page, import.meta.url))),
    },
  },
  server: {
    host: '127.0.0.1',
    port: 5173,
    // CUBIE_API lets a second dev stack (e.g. for automated tests) run alongside.
    proxy: { '/api': process.env.CUBIE_API ?? 'http://127.0.0.1:8080' },
  },
});
