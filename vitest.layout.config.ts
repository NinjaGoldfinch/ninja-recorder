import { svelte } from "@sveltejs/vite-plugin-svelte";
import { playwright } from "@vitest/browser-playwright";
import { defineConfig } from "vitest/config";
import pkg from "./package.json";

/**
 * The layout gate (#345): tests that need a browser to mean anything.
 *
 * Everything else runs in jsdom (`vitest.config.ts`), and jsdom does no
 * layout at all: every box is zero by zero. So no test there could have seen
 * #342, where library rows drew past their card at both ends of the window
 * range with every gate green. These run the real components with the real
 * stylesheet in Chromium, and measure.
 *
 * A config of its own rather than a second project in the main one, so that
 * `npx vitest run` keeps needing no browser. `npm run test:layout` runs this,
 * after `npx playwright install chromium`.
 */
export default defineConfig({
  plugins: [svelte()],

  // The same injection the app build and the unit config make.
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },

  test: {
    include: ["src/**/*.layout.test.ts"],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: "chromium" }],
      // The largest window a test asks for, so a width is never clamped
      // to whatever the runner's default happened to be.
      viewport: { width: 2560, height: 900 },
    },
  },
});
