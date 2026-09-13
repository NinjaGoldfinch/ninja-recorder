import { defineConfig } from "vitest/config";

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
  test: {
    // `format.ts` is pure and needs nothing; `router.ts` toggles the `hidden`
    // attribute on real elements, which is exactly the behaviour worth
    // pinning before Svelte takes it over. jsdom rather than happy-dom
    // because `hidden` and `dataset` semantics are what is under test.
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
    // The dev portal is a separate entry point with its own lifecycle; WS4
    // leaves it on the vanilla stack (plan §9, Q6), so it is out of scope
    // here rather than untested by accident.
    exclude: ["src/dev/**", "node_modules/**", "dist/**"],
  },
});
