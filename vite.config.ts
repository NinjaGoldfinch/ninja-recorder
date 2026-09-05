import { defineConfig } from "vite";
import pkg from "./package.json";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // Surfaced in the settings "About" block so the version shown is the one
  // that was built, not a string someone has to remember to bump twice.
  // The dev portal (dev.html) is a second entry point, built only when
  // opted in — a plain `npm run build` must not be able to leak it into
  // `dist/`, since it pairs with Rust commands that are themselves
  // compiled out unless the `devtools` Cargo feature is on. The dev
  // server serves any root-level .html regardless, so `npm run dev`
  // needs no equivalent branch.
  build: {
    rollupOptions: {
      // @ts-expect-error process is a nodejs global
      input: process.env.NINJA_DEVTOOLS
        ? { main: "index.html", dev: "dev.html" }
        : { main: "index.html" },
    },
  },

  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  // 2. tauri expects a fixed port, fail if that port is not available
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
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
