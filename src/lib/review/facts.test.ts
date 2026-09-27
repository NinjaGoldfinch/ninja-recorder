import { describe, expect, it } from "vitest";
import type { RecordingRow } from "../../types";
import { reviewFacts } from "./facts";

const row = (over: Partial<RecordingRow> = {}): RecordingRow =>
  ({
    id: 1,
    path: "C:/vods/1.mp4",
    started_at: Date.UTC(2026, 8, 27, 0, 30),
    duration_s: 1515,
    game_id: 1,
    queue: 420,
    champion: "Viego",
    role: "JUNGLE",
    win: true,
    kda_k: 9,
    kda_d: 2,
    kda_a: 11,
    patch: null,
    pinned: false,
    size_bytes: 0,
    audio_tracks_json: null,
    game_mode: "CLASSIC",
    diagnostics_json: null,
    scoreboard_json: null,
    cs: 187,
    tier: "EMERALD",
    division: "II",
    lp_after: 38,
    ...over,
  }) as RecordingRow;

describe("reviewFacts", () => {
  it("lists everything known, in reading order", () => {
    const [when, ...rest] = reviewFacts(row());
    expect(when).toBeTruthy();
    expect(rest).toEqual(["25:15", "Ranked Solo", "9 / 2 / 11", "187 CS", "Emerald II 38 LP"]);
  });

  it("leaves out what it does not know, rather than showing a placeholder", () => {
    const facts = reviewFacts(
      row({
        duration_s: null,
        queue: null,
        game_mode: null,
        kda_k: null,
        cs: null,
        tier: null,
      }),
    );
    expect(facts).toHaveLength(1);
  });

  it("shows a rank without LP, and never LP without a rank", () => {
    expect(reviewFacts(row({ lp_after: null }))).toContain("Emerald II");
    expect(reviewFacts(row({ tier: null })).some((f) => f.includes("LP"))).toBe(false);
  });
});
