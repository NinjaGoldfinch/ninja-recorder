/**
 * The Svelte 5 application — WS4, with its contract client from WS2.
 *
 * Every view lives here since WS4.5. `main.ts` is down to a composition
 * root that mounts `App.svelte` and wires the handful of things that are not
 * views; `router.ts` owns the join (`mountApp` / `unmountApp`). See
 * docs/frontend.md, "The Svelte seam".
 *
 * What lands here (implementation plan §3.4, §4.6):
 *
 * | Path | WS | What it becomes |
 * |---|---|---|
 * | `App.svelte` | WS4.1 | LANDED: the root, mounted into `#svelte-root` |
 * | `components/library/` | WS4.3 | LANDED: eight components replacing `library.ts`, which is deleted |
 * | `components/settings/` | WS4.4 | LANDED: nine components replacing `settings.ts` and `update.ts`, both deleted |
 * | `components/review/` | WS4.5 | LANDED: five components replacing `review.ts`, which is deleted. `Review` is an imperative island |
 * | `contract/` | WS2.5 | GENERATED: `types.ts`, `client.ts`, `events.ts`, `index.ts`. Committed, CI-checked by `gen-contract --check` |
 * | `transport/` | WS2.6, WS3.6 | `invoke.ts` and `mock.ts` landed in WS2.6; `pipe.ts` is WS3.6 |
 * | `stores/` | WS4.3–4.5 | LANDED: `library`, `icons`, `settings`, `update`, `about`, `review` |
 * | `library/`, `timeline/`, `settings/`, `review/` | WS4.2–4.5 | LANDED: the pure logic, with its tests |
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
