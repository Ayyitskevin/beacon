import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

const host = process.env.TAURI_DEV_HOST;

// Tauri's current Vite integration keeps a fixed dev port and does not clear Rust errors.
// Source: https://v2.tauri.app/start/frontend/vite/#example-configuration
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: "./src/test/setup.ts",
    css: true,
    // Bundling UI libraries makes jsdom exercise the same interop path as the browser.
    // Source: https://v4.vitest.dev/config/deps.html#deps-optimizer
    deps: {
      optimizer: {
        client: {
          enabled: true,
          include: ["@fluentui/react-components"],
        },
      },
    },
  },
});
