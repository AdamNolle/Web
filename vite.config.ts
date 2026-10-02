import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  test: {
    environment: 'jsdom',
    // The personal Windows VM serves several runners concurrently.
    maxWorkers: process.env.CI && process.platform === 'win32' ? 1 : undefined,
    setupFiles: ['./src/test/setup.ts'],
    css: true,
  },
});
