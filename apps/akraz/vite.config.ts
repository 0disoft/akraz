import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
  build: {
    target: ["es2022", "chrome120", "safari17"],
    sourcemap: false,
  },
});
