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
};

export function setAboutLcu(line: string) {
  lcu = line;
}

export function setAboutGameState(line: string) {
  gameState = line;
}

export function setAboutLastFinalized(line: string) {
  lastFinalized = line;
}
