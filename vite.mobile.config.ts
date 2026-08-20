import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

/**
 * The phone app.
 *
 * A separate front end from the desktop one, sharing the same Rust backend and
 * therefore the same vault, the same database schema and the same Google Drive
 * mechanism. Only the screens differ — a desktop grid built for a mouse and a
 * 1280-pixel window is the wrong shape for a phone, and pretending otherwise is
 * what produced a title hidden behind the status bar.
 */
export default defineConfig({
  root: 'mobile',
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    // 1420 belongs to the desktop dev server; both can run at once.
    port: 1421,
    strictPort: true,
    // The phone connects over the network, not over loopback.
    host: '0.0.0.0',
    watch: { ignored: ['**/src-tauri/**', '**/spikes/**'] },
  },
  build: {
    outDir: '../dist-mobile',
    emptyOutDir: true,
    // Android System WebView tracks Chrome; this matches the oldest reasonable one.
    target: 'chrome110',
    sourcemap: true,
  },
});
