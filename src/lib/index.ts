/**
 * The Svelte 5 application — WS4, with its contract client from WS2.
 *
 * `src/main.ts` is still most of the frontend; WS4 is a strangler migration,
 * so this directory fills up view by view while `main.ts` shrinks, and the
 * two run side by side for the length of it. `router.ts` owns the join
 * (`mountApp` / `unmountApp`); see docs/frontend.md, "The Svelte seam".
 *
 * What lands here (implementation plan §3.4, §4.6):
 *
 * | Path | WS | What it becomes |
 * |---|---|---|
 * | `App.svelte` | WS4.1 | LANDED: the root, mounted into `#svelte-root` |
 * | `components/library/` | WS4.3 | LANDED: eight components replacing `library.ts`, which is deleted |
 * | `Settings.svelte`, `Review.svelte`, `Timeline.svelte` | WS4.4–4.5 | the views still replacing `settings.ts`, `update.ts`, `review.ts` |
 * | `contract/` | WS2.5 | GENERATED: `types.ts`, `client.ts`, `events.ts`, `index.ts`. Committed, CI-checked by `gen-contract --check` |
 * | `transport/` | WS2.6, WS3.6 | `invoke.ts` and `mock.ts` landed in WS2.6; `pipe.ts` is WS3.6 |
 * | `stores/` | WS4.3 | LANDED: `library.svelte.ts` and `icons.svelte.ts` |
 * | `library/`, `timeline/` | WS4.2 | LANDED: the pure logic, with its tests |
 * | `styles/tokens.css` | WS4.1 | LANDED: every custom property, moved out of `styles.css` unchanged |
 *
 * The stores are not yet driven by the daemon's event stream, which is what
 * §4.2 describes. `status.ts` still calls `refreshLibrary` when a
 * `library-changed` event arrives, exactly as it called `library.ts`'s. Moving
 * that subscription into the store belongs with the view that needs it.
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
