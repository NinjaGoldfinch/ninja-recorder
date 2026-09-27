/**
 * Whether the review rail is open beside the player, remembered across VODs
 * and restarts.
 *
 * A layout preference of this window's and nothing else's, so it lives in
 * `localStorage` rather than beside the real preferences in SQLite: nothing
 * outside the frontend reads it, and losing it costs one click.
 */

const RAIL_KEY = "nr.review.railOpen";

/** Never throws: blocked or corrupt storage opens the rail, the default. */
export function railOpenSaved(): boolean {
  try {
    return localStorage.getItem(RAIL_KEY) !== "closed";
  } catch {
    return true;
  }
}

export function saveRailOpen(open: boolean): void {
  try {
    localStorage.setItem(RAIL_KEY, open ? "open" : "closed");
  } catch {
    // Out of quota or blocked. The rail still toggles; it just is not
    // remembered.
  }
}
