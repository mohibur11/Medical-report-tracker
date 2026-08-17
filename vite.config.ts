import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// Tauri drives this dev server, so the port is fixed and failure must be loud:
// if 1420 is taken, silently moving to 1421 leaves the desktop window pointed at
// nothing.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // The Rust side has its own watcher; double-watching target/ is expensive.
      ignored: ['**/src-tauri/**', '**/spikes/**'],
    },
  },
  build: {
    outDir: 'dist',
    // WebView2 on Windows 11 is evergreen Chromium — no legacy transpilation needed.
    target: 'chrome120',
    sourcemap: true,
  },
});
