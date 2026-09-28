/**
 * Which item goes in which box, and whose rune page to draw.
 *
 * Pure, so the rules the library row follows (#341) are tested here rather
 * than by counting elements in a rendered row: the trinket is always the
 * fourth box, and an eighth box exists only for a bot laner.
 */

import type { Scoreboard, ScoreboardPlayer, ScoreboardRunes } from "../../types";

/**
 * The trinkets, for boards written before the scoreboard named the trinket
 * (#346). Every recording made until then has one, so this is not a corner
 * case: it is most of a library on the day the new row ships.
 *
 * A list rather than Data Dragon's `Trinket` tag because the icon set carries
 * art, not tags, and fetching the item catalogue for one fallback is more
 * than it is worth. It only has to recognise what a finished game ends with,
 * and that has been these four for years. A trinket missing from it costs one
 * box: the item is drawn with the rest of the build instead of in box four.
 */
const LEGACY_TRINKETS = new Set([3340, 3363, 3364, 3330]);

/** Box four, counting from zero: the top right of the 4×2 grid. */
export const TRINKET_BOX = 3;

/**
 * A player's build, one entry per box: an item id, or null for a box with
 * nothing in it.
 *
 * Seven boxes, or eight for a bot laner. Boxes 0-2 and 4-6 hold the six
 * inventory items in slot order, box 3 is the trinket, and box 7 is the role
 * quest's boots slot. **Every other role's role slot holds a quest token, not
 * an item**, so it is not drawn at all rather than drawn empty: an empty box
 * would read as a missing item.
 *
 * `position` is the player's own. Ours may come from the recording's `role`
 * when the scoreboard does not say, which is why it is a parameter.
 */
export function buildBoxes(
  player: ScoreboardPlayer | null,
  position: string | null | undefined,
): (number | null)[] {
  const items = player?.items ?? [];
  const trinket = trinketOf(player);

  // The trinket also sits in `items`, at the end, where both writers leave
  // it. Taken out once, from the end, so a second copy of the same id (not
  // that a game allows one) would still be drawn.
  const rest = [...items];
  if (trinket !== null) {
    const at = rest.lastIndexOf(trinket);
    if (at !== -1) rest.splice(at, 1);
  }
  const six = [...rest.slice(0, 6), ...Array<null>(Math.max(0, 6 - rest.length)).fill(null)];

  const boxes = [six[0], six[1], six[2], trinket, six[3], six[4], six[5]].map((id) => id ?? null);
  if (position === "Bottom") boxes.push(player?.role_item ?? null);
  return boxes;
}

/**
 * The trinket: the scoreboard's own answer where it has one, otherwise the
 * last item if it is one of the known trinkets.
 *
 * A board written after #346 with an empty trinket slot looks like an older
 * board here (neither has the key). The fallback then finds a real item last,
 * not a trinket, and answers null, which is right for both.
 */
function trinketOf(player: ScoreboardPlayer | null): number | null {
  if (!player) return null;
  if (player.trinket != null) return player.trinket;
  const last = player.items[player.items.length - 1];
  return last !== undefined && LEGACY_TRINKETS.has(last) ? last : null;
}

/**
 * A player's rune page. Each player carries their own since #346; boards
 * written before that have ours alone, as `our_runes`, and nobody else's.
 */
export function runesOf(
  board: Scoreboard | null,
  player: ScoreboardPlayer | null,
): ScoreboardRunes | null {
  if (!player) return null;
  if (player.runes) return player.runes;
  return player.is_us ? (board?.our_runes ?? null) : null;
}
