import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";
import pkg from "./package.json";

/**
 * Frontend unit tests — WS5 task 5.5.
 *
 * v1 ships 12,797 lines of TypeScript and zero tests, against 24,822 lines of
 * Rust with 447. That asymmetry is most of why the frontend is the half being
 * replaced: there was no way to change it safely. This config is the first
 * half of fixing that, and WS4 depends on it — a strangler migration with no
 * tests on the code being strangled is just a rewrite with extra steps.
 *
 * Targets, in the order the plan (§4.7) names them: the pure modules first —
 * `format.ts` and `router.ts` — then the timeline clustering and marker
 * grouping once WS4 extracts them, then component tests in browser mode for
 * `Row.svelte` and `Filters.svelte`.
 *
 * Tests live beside their module as `<name>.test.ts` rather than in a
 * separate tree, so a file and its test move together during WS4 — and so a
 * module that loses its test is visible in the same directory listing.
 */
export default defineConfig({
  // The same plugin the app builds with, so a component under test is compiled
  // the way it ships, reading the same `svelte.config.js`. HMR turns itself
  // off outside `vite dev`, so there is nothing to pass here.
  plugins: [svelte()],

  // The same injection `vite.config.ts` makes. `About.svelte` reads it, so a
  // test that mounts the settings view fails at render without it, and a
  // hard-coded string here would be a second place for the version to live.
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },

  resolve: {
    // Without this, Node's own export conditions win and `svelte` resolves to
    // its server build, where `mount` throws. The failure names neither Svelte
    // nor this file, so it is worth the two lines to make impossible.
    conditions: ["browser"],
  },

  test: {
    // `format.ts` is pure and needs nothing; `router.ts` toggles the `hidden`
    // attribute on real elements, which is exactly the behaviour worth
    // pinning before Svelte takes it over. jsdom rather than happy-dom
    // because `hidden` and `dataset` semantics are what is under test.
    environment: "jsdom",
    // jsdom has no `matchMedia`, and `theme.ts` calls it at module scope, so
    // any test that reaches the settings view fails on import. See the file
    // for why the call is not the thing that moves.
    setupFiles: ["src/test-setup.ts"],
    include: ["src/**/*.test.ts"],
    // The dev portal is a separate entry point with its own lifecycle; WS4
    // leaves it on the vanilla stack (plan §9, Q6), so it is out of scope
    // here rather than untested by accident.
    exclude: ["src/dev/**", "node_modules/**", "dist/**"],

    // `npm run coverage`. Scoped to `src/lib/`, and that is the whole point:
    // measuring the vanilla modules would report a number dominated by
    // `review.ts` and `library.ts`, which are DOM wiring being strangled and
    // are not the thing WS4 is putting under test. What the percentage has to
    // mean is "the extracted logic is covered", so the denominator is the
    // extracted logic.
    //
    // Generated and stub files are excluded for the same reason: `contract/`
    // is emitted from Rust and CI-checked by `gen-contract --check`, and a
    // module whose whole body is `export {}` would otherwise report as a
    // perfect score and flatter the average.
    coverage: {
      provider: "v8",
      // No HTML report: it writes a few hundred files of vendored istanbul
      // assets for a number that fits on one line.
      reporter: ["text", "lcov"],
      include: ["src/lib/**"],
      exclude: [
        "**/*.test.ts",
        "src/lib/contract/**",
        "src/lib/index.ts",
        "src/lib/stores/index.ts",
      ],
      // WS4.2's exit criterion. A floor, not a target: it fails a change that
      // adds logic to `src/lib/` without tests, rather than asking anyone to
      // chase the last few percent.
      thresholds: { lines: 80 },
    },
  },
});
