/**
 * What a panel is given, and how one panel hands off to another - #72.
 *
 * `main.ts` passed a `PanelContext` object with `env`, `refresh()`,
 * `navigate()` and a `payload` getter that cleared on read. Most of that was
 * machinery for re-running a `draw()`: `refresh` existed because a panel could
 * not react to its own writes, and `payload` cleared on read because it was a
 * module-level variable that outlived the panel it was meant for.
 *
 * What is left is the part that was never about rendering: moving between
 * panels, and carrying one argument when you do.
 */

import { getContext, setContext } from "svelte";
import type { DevEnvInfo } from "../../dev/types";

const KEY = Symbol("dev-portal");

export interface DevContext {
  readonly env: DevEnvInfo | null;
  /**
   * Switches panels.
   *
   * **`handoff` does not go in the URL**, and that is not a style preference:
   * the one caller is Fixtures sending a fixture body to Simulate, and the
   * captured `eog-stats-block` is 99 KB of nested JSON. `main.ts` kept it in a
   * module variable for the same reason; what is new is that the two kinds of
   * payload are now named differently instead of sharing one slot.
   *
   * The other kind is in the hash: `#/library/12` is what lets the main
   * window's rows open the portal *on* a recording. That one is `route.payload`
   * and is small by construction.
   */
  navigate(id: string, handoff?: string): void;
  /** Whatever `navigate` handed over, consumed on read. */
  takeHandoff(): string | null;
  /** Asks the shared confirm dialog. Resolves false on any dismissal. */
  confirm(options: import("./ui-types").ConfirmOptions): Promise<boolean>;
}

export function setDevContext(ctx: DevContext) {
  setContext(KEY, ctx);
}

export function devContext(): DevContext {
  return getContext<DevContext>(KEY);
}
