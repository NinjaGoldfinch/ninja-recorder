/**
 * The Svelte 5 application — WS4, with its contract client from WS2.
 *
 * Empty on purpose. `src/main.ts` is still the whole frontend; WS4 is a
 * strangler migration, so this directory fills up view by view while
 * `main.ts` shrinks, and the two run side by side for the length of it.
 *
 * What lands here (implementation plan §3.4, §4.6):
 *
 * | Path | WS | What it becomes |
 * |---|---|---|
 * | `App.svelte`, `Library.svelte`, `Settings.svelte`, `Review.svelte`, `Timeline.svelte` | WS4.2–4.5 | the views, replacing `library.ts`, `settings.ts`, `update.ts`, `review.ts` |
 * | `contract/` | WS2.5 | GENERATED: `types.ts`, `client.ts`, `events.ts`, `index.ts`. Committed, CI-checked by `gen-contract --check` |
 * | `transport/` | WS2.6, WS3.6 | `invoke.ts` and `mock.ts` landed in WS2.6; `pipe.ts` is WS3.6 |
 * | `stores/` | WS4.1 | `$state` driven by the daemon's snapshot and event stream |
 * | `styles/tokens.css` | WS4.1 | design tokens, replacing the 2,013-line global `styles.css` |
 *
 * The player is migrated last and stays an imperative island: it owns real
 * DOM nodes because `<video>` currentTime is not state anything should be
 * diffing.
 *
 * **No `{@html}` on any recording-derived string.** `db::reconcile` imports
 * whatever video file the user drops into the folder, so a filename is
 * untrusted input. v1 guards it with `escapeHtml`/`escapeAttr` in `dom.ts`
 * (docs/frontend.md); Svelte's default text interpolation replaces both, and
 * `{@html}` opts straight back out of the thing that made the migration safe.
 */
export {};
