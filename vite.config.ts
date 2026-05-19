import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import pkg from "./package.json" with { type: "json" };

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  // Inject the version from package.json so the footer never drifts from
  // the bundled binary's version after a release bump.
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
