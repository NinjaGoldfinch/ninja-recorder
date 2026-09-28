/**
 * What a marker looks like, and what it says.
 *
 * Moved out of `review.ts` by WS4.5. It was already pure and carried most of
 * its reasoning in comments; what it did not have was a test, because it sat
 * beside the function that assigned its output to `innerHTML`.
 *
 * `MARKER_PRIORITY`, which decides whose icon a *cluster* shows, is in
 * `clusters.ts` with the clustering that needs it.
 */

import type { MarkerRow } from "../../types";

export interface MarkerStyle {
  icon: string;
  label: string;
  color: string;
}

export const MARKER_STYLE: Record<string, MarkerStyle> = {
  kill: { icon: "⚔️", label: "Kill", color: "#43a047" },
  death: { icon: "💀", label: "Death", color: "#e53935" },
  assist: { icon: "🤝", label: "Assist", color: "#1e88e5" },
  dragon: { icon: "🐉", label: "Dragon", color: "#8e24aa" },
  baron: { icon: "👑", label: "Baron", color: "#6d4c41" },
  herald: { icon: "🦅", label: "Herald", color: "#00897b" },
  voidgrubs: { icon: "🪱", label: "Voidgrubs", color: "#c0ca33" },
  turret: { icon: "🏰", label: "Turret", color: "#fb8c00" },
  inhibitor: { icon: "💠", label: "Inhibitor", color: "#5e35b1" },
  ace: { icon: "⭐", label: "Ace", color: "#fdd835" },
  multikill: { icon: "🔥", label: "Multikill", color: "#f4511e" },
  first_blood: { icon: "🩸", label: "First Blood", color: "#d81b60" },
};

/**
 * A kind with no entry still gets a glyph.
 *
 * `kind` is a TEXT column and a newer build can write one this one has never
 * heard of; a marker that vanished would be worse than a grey dot.
 */
export function markerStyle(marker: MarkerRow): MarkerStyle {
  return MARKER_STYLE[marker.kind] ?? { icon: "●", label: marker.kind, color: "#999" };
}

/**
 * Only ever appended when the flag is explicitly true.
 *
 * It is absent on every marker recorded before steals were captured, and
 * `undefined` is not "it wasn't stolen", it is "nobody asked".
 */
function stolen(payload: Record<string, unknown>): string {
  return payload.stolen === true ? " (stolen)" : "";
}

/**
 * 2 through 5 have names everyone uses; anything beyond that is either a
 * pentakill already or a game mode where counting up is the wrong answer.
 */
const MULTIKILL_NAMES: Record<number, string> = {
  2: "Double Kill",
  3: "Triple Kill",
  4: "Quadra Kill",
  5: "Penta Kill",
};

export function multikillLabel(streak: unknown): string {
  if (typeof streak !== "number") return "Multikill";
  return MULTIKILL_NAMES[streak] ?? `${streak}× Multikill`;
}

/**
 * Elder is the one dragon type worth keeping.
 *
 * The elemental drakes are interchangeable to somebody scrubbing a VOD: the
 * moment is "we took a dragon", and which one it was does not change what is
 * on screen. Elder is a different objective. It usually decides the game, and
 * it is a thing you would go looking for by name.
 */
export function isElder(payload: Record<string, unknown>): boolean {
  const type = payload.dragon_type;
  return typeof type === "string" && type.trim().toLowerCase() === "elder";
}

/**
 * Kinds still recorded but never drawn: the announcer's lines.
 *
 * A multikill, first blood and an ace are each the same moment as a kill the
 * review already shows, so a row or a glyph for one is a second copy of that
 * kill in a louder voice. They stay in the database, since collecting them
 * costs nothing and a later view might want them; the review leaves them out
 * of the timeline, the Events tab and `[`/`]` alike, so the three never
 * disagree about what a marker is.
 */
export const HIDDEN_KINDS: ReadonlySet<string> = new Set(["multikill", "first_blood", "ace"]);

export function isShown(marker: MarkerRow): boolean {
  return !HIDDEN_KINDS.has(marker.kind);
}

export function markerLabel(m: MarkerRow): string {
  let payload: Record<string, unknown> = {};
  try {
    payload = JSON.parse(m.payload_json);
  } catch {
    // Malformed payload: fall back to just the kind below.
  }
  const str = (key: string) => (typeof payload[key] === "string" ? (payload[key] as string) : "?");
  // The champion, not the player: a Riot ID is somebody else's name, and the
  // champion is what was on screen. Recorded beside the name since the kill
  // labels changed, so a marker from before then has only the name, and a
  // turret or a minion has no champion at all; both fall back to the name.
  const who = (key: string) => {
    const champion = payload[`${key}_champion`];
    return typeof champion === "string" && champion !== "" ? champion : str(key);
  };

  switch (m.kind) {
    // The three that name somebody: who you killed, who killed you, and whose
    // kill you helped with. That champion is the whole content of the marker
    // and it is never yours, so it stays.
    case "kill":
      return `Killed ${who("victim")}`;
    case "death":
      return `Killed by ${who("killer")}`;
    case "assist":
      return `${who("killer")} killed ${who("victim")}`;

    // The objectives name nobody. `classify_event` only writes one of these
    // when you took part (`took_part()` gates every branch), so the killer is
    // you or an ally you assisted, and printing it told you either your own
    // champion's name or a detail you were not scrubbing for. Same for the
    // elemental type: it says which drake, not which moment, and "Fire Dragon
    // — Shyvana" is four words to say "Dragon".
    case "dragon":
      return `${isElder(payload) ? "Elder Dragon" : "Dragon"}${stolen(payload)}`;
    case "baron":
      return `Baron${stolen(payload)}`;
    case "herald":
      return `Herald${stolen(payload)}`;
    case "voidgrubs":
      return `Voidgrubs${stolen(payload)}`;
    case "turret":
      return "Turret";
    case "inhibitor":
      return "Inhibitor";

    // `Ace` is only recorded when you landed the closing kill, so the acing
    // team was always yours; `FirstBlood` only when you got it. Both suffixes
    // were constants dressed as data.
    case "ace":
      return "Ace";
    case "first_blood":
      return "First Blood";

    case "multikill":
      return multikillLabel(payload.kill_streak);
    default:
      return m.kind;
  }
}

/**
 * Markers the recording actually contains, and markers it does not.
 *
 * **A crashed recording ends before the game does, and its markers do not
 * know that.** `video_time_s` is written live and resolved against whatever
 * alignment was known at that moment, falling back to 1:1 when none has been
 * proven yet. The finalize re-resolves every marker against the final
 * alignment, which is why `finish_recording` deletes and re-inserts them. A
 * killed daemon never reaches the finalize, so the provisional times are the
 * ones that survive, and some of them name moments past the end of the file.
 *
 * They are kept rather than dropped: the events happened, and for a game whose
 * finalize never ran they are the only record that they did (#150). What they
 * cannot do is claim a position on a timeline that does not reach them.
 *
 * **The split is also the evidence.** A marker beyond the footage proves the
 * alignment was never resolved, because a resolved one cannot point past the
 * end of its own recording. So nothing needs to read a "was recovered" flag,
 * and there is no schema change here: the condition that hides the marker is
 * the same condition that justifies saying the times are approximate.
 */
export interface MarkerFootage {
  /** In the file, and safe to draw and seek to. */
  inside: MarkerRow[];
  /** Real events the file does not reach. Listed, never drawn. */
  beyond: MarkerRow[];
}

export function splitByFootage(markers: readonly MarkerRow[], durationS: number): MarkerFootage {
  // An unknown duration is not zero footage: before `loadedmetadata` lands,
  // every marker would be "beyond" and the whole timeline would empty itself
  // for a frame. Treat it as "cannot tell yet" and draw everything, which is
  // what happened before this function existed.
  if (!Number.isFinite(durationS) || durationS <= 0) {
    return { inside: [...markers], beyond: [] };
  }

  const inside: MarkerRow[] = [];
  const beyond: MarkerRow[] = [];
  for (const marker of markers) {
    if (marker.video_time_s > durationS) beyond.push(marker);
    else inside.push(marker);
  }
  return { inside, beyond };
}
