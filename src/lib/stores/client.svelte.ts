/**
 * What the poll last heard from the League client and the recorder, raw.
 *
 * `status.svelte.ts` owns the poll and writes here; the app bar's client pill
 * and its hover card read, and `lib/shell/client.ts` turns the values into
 * words. Raw rather than pre-worded because the pill and the card say
 * different things about the same answer, and wording it twice in the poll
 * would put two views' copy in a module that has none.
 *
 * `null` is "not asked yet", which is different from "not running" and is
 * what lets the pill say it is still checking.
 */

import type { GameState, LcuStatus, SupervisorStatus } from "../../types";

let lcu = $state<LcuStatus | null>(null);
let game = $state<GameState | null>(null);
let elapsed = $state<number | null>(null);
let failed = $state(false);

export const client = {
  get lcu() {
    return lcu;
  },
  get game() {
    return game;
  },
  /** Seconds since capture began, from the supervisor; null when not recording. */
  get elapsed() {
    return elapsed;
  },
  /** The last `game_state_status` call failed. */
  get failed() {
    return failed;
  },
};

export function setClientLcu(status: LcuStatus) {
  lcu = status;
}

export function setClientGame(status: SupervisorStatus) {
  game = status.state;
  elapsed = status.recording_elapsed_s;
  failed = false;
}

export function setClientFailed() {
  failed = true;
}
