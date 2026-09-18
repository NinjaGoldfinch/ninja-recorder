<!--
  The Svelte root — WS4 task 4.1.

  Empty on purpose, and the emptiness is the exit criterion: it mounts beside
  the vanilla views and unmounts again without any of them noticing. Until
  WS4.3 that is the whole of its job. What it proves is that the seam exists
  — the compiler runs, `svelte-check` sees the file, and `router.ts` can bring
  a component up and take it down — so the first real view lands as a view
  and not as a toolchain change wearing one.

  Views arrive here one at a time (implementation plan §4.6): `Library` in
  WS4.3, `Settings` and `Update` in WS4.4, `Review` and `Timeline` in WS4.5.
  Each one deletes its vanilla counterpart and the markup `index.html` holds
  for it in the same commit, so the two frontends never both own a view.

  It imports no stylesheet. `styles/tokens.css` is pulled in by `styles.css`,
  which `index.html` loads as a `<link>` before first paint — a token block
  arriving later, over a JS import, is the theme flash the boot script in
  that file exists to prevent. WS4.6 re-homes the import when it deletes the
  stylesheet; until then there is exactly one loader of the tokens.

  **No `{@html}` on any recording-derived string.** `db::reconcile` imports
  whatever video file the user drops into the folder, so a filename is
  untrusted input. Default interpolation is what replaces v1's `escapeHtml`
  and `escapeAttr`; `{@html}` opts straight back out of it.
-->

<!--
  Empty, and it has to be here. `svelte2tsx` emits a typed component for a
  file with a script block and an untyped one for a file without, so deleting
  these two lines turns `import App from "./lib/App.svelte"` in `main.ts`
  back into an implicit `any` and fails `svelte-check` — from the importer,
  with no mention of this file. Give it a body rather than removing it.
-->
<script lang="ts"></script>
