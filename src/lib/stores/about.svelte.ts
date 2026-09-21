/**
 * The three live lines in Settings → About - WS4 task 4.4.
 *
 * `status.ts` owns the poll timer and always has. What changed is where its
 * answers go: it used to write them straight into elements inside the settings
 * markup, so a module that owns the app bar's pills also owned three rows of a
 * view it has nothing to do with. Now it publishes here and `About.svelte`
 * reads.
 *
 * Deliberately not merged into `settings.svelte.ts`. These are pushed by a
 * poll on its own schedule, not read when the view opens, and keeping them
 * apart is what stops the settings store growing a lifecycle it does not have.
 */

let lcu = $state("—");
let gameState = $state("—");
let lastFinalized = $state("—");

/**
 * The two pills in the app bar.
 *
 * They are the short form of the same answers, and they live here for the same
 * reason: `status.ts` polls for them on a schedule of its own, and WS4.6 took
 * its elements away. `state` is the `data-state` attribute the stylesheet
 * colours on.
 */
let lcuPillState = $state({ state: "unknown", copy: "Checking client\u2026" });
let gamePillState = $state({ state: "idle", copy: "Idle" });

export const about = {
  get lcu() {
    return lcu;
  },
  get gameState() {
    return gameState;
  },
  get lastFinalized() {
    return lastFinalized;
  },
  get lcuPill() {
    return lcuPillState;
  },
  get gamePill() {
    return gamePillState;
  },
};

export function setLcuPill(pill: { state: string; copy: string }) {
  lcuPillState = pill;
}

export function setGamePill(pill: { state: string; copy: string }) {
  gamePillState = pill;
}

export function setAboutLcu(line: string) {
  lcu = line;
}

export function setAboutGameState(line: string) {
  gameState = line;
}

export function setAboutLastFinalized(line: string) {
  lastFinalized = line;
}
