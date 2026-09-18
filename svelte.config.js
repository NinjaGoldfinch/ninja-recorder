import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/**
 * Svelte 5 — WS4 task 4.1.
 *
 * Read by three things that must agree: the Vite plugin, `svelte-check`, and
 * the editor extension. A component that compiles under `npm run dev` but
 * fails the type gate is almost always this file disagreeing with itself,
 * which is why the config is one object rather than options passed at each
 * call site.
 *
 * `vitePreprocess` is here for `<script lang="ts">` and nothing else. The
 * components carry plain CSS, so there is no style preprocessor to configure
 * and none should be added: `styles/tokens.css` plus a scoped `<style>` block
 * per component is the whole styling story (implementation plan §4.6).
 */
export default {
  preprocess: vitePreprocess(),

  compilerOptions: {
    // Runes are not the default in Svelte 5 — a component without a `$state`
    // or `$props` in it compiles in legacy mode, where `export let` is a prop
    // and a bare `let` is reactive. Half the tree inferring one mode and half
    // the other is the migration hazard this setting removes: WS4 is written
    // in runes throughout, so the mode is declared once here rather than
    // being an emergent property of what each file happens to contain.
    runes: true,
  },
};
