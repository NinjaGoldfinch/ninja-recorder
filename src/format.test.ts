import { describe, expect, it } from "vitest";
import {
  basename,
  formatBytes,
  formatClock,
  formatKda,
  formatRelative,
  formatSpan,
  formatTime,
  gameModeLabel,
  kdaRatio,
  lpLabel,
  patchLabel,
  queueLabel,
  queueOrModeLabel,
  rankLabel,
  vodHeading,
  vodTitle,
} from "./format";
import type { RecordingRow } from "./types";

/**
 * First tests in this repository's frontend — WS5 task 5.5.
 *
 * `format.ts` is the right place to start because every function in it is
 * pure and every one of them is a product decision: what a card says when
 * the LCU never answered, what a deathless game's KDA ratio is, whether an
 * undecided game is allowed to look like a loss. Those answers are in the
 * module's comments today and nowhere else, so the tests below are mostly
 * transcriptions of them — which is the point. WS4 rewrites the views around
 * this module; these say what it must keep meaning while that happens.
 *
 * Locale- and clock-dependent output (`formatDateTime`, the far end of
 * `formatRelative`) is asserted on structure rather than on an exact string:
 * a test that pins `"13 Sep, 08:24"` pins the runner's locale, not the code.
 */

function row(over: Partial<RecordingRow> = {}): RecordingRow {
  return {
    id: 1,
    path: "C:\\Users\\ninja\\recordings\\2026-09-13_ranked.mp4",
    started_at: 0,
    duration_s: null,
    game_id: null,
    queue: null,
    champion: null,
    role: null,
    win: null,
    kda_k: null,
    kda_d: null,
    kda_a: null,
    patch: null,
    pinned: false,
    size_bytes: 0,
    audio_tracks_json: null,
    game_mode: null,
    ...over,
  } as RecordingRow;
}

describe("formatBytes", () => {
  it("switches to GB at exactly 1 GB, not before", () => {
    expect(formatBytes(1024 ** 3 - 1)).toBe("1024 MB");
    expect(formatBytes(1024 ** 3)).toBe("1.0 GB");
  });

  it("rounds MB to whole numbers and GB to one decimal", () => {
    expect(formatBytes(1_500_000)).toBe("1 MB");
    expect(formatBytes(3.2 * 1024 ** 3)).toBe("3.2 GB");
    // `toFixed` rounds a half away from zero, so 3.25 GB reads as 3.3.
    // Pinned because it is the kind of thing a reimplementation gets wrong
    // in the other direction without anybody noticing.
    expect(formatBytes(3.25 * 1024 ** 3)).toBe("3.3 GB");
  });
});

describe("clock formatting", () => {
  it("formatTime never grows an hours field", () => {
    expect(formatTime(0)).toBe("0:00");
    expect(formatTime(65)).toBe("1:05");
    // 90 minutes as minutes, deliberately: this is a position within a VOD.
    expect(formatTime(5400)).toBe("90:00");
  });

  it("formatClock adds hours only when there are some", () => {
    expect(formatClock(1934)).toBe("32:14");
    expect(formatClock(3661)).toBe("1:01:01");
  });

  it("formatSpan drops to hours, because cumulative time is read not timed", () => {
    expect(formatSpan(59)).toBe("0m");
    expect(formatSpan(2 * 3600 + 30 * 60)).toBe("2h 30m");
  });

  it("truncates rather than rounds, so a game never gains a second", () => {
    expect(formatClock(59.9)).toBe("0:59");
    expect(formatTime(119.9)).toBe("1:59");
  });
});

describe("formatRelative", () => {
  it("counts back in the largest unit that still fits", () => {
    const now = Date.now();
    expect(formatRelative(now - 30_000)).toMatch(/second/);
    expect(formatRelative(now - 16 * 60_000)).toMatch(/minute/);
    expect(formatRelative(now - 5 * 3_600_000)).toMatch(/hour/);
    expect(formatRelative(now - 3 * 86_400_000)).toMatch(/day/);
  });

  it("hands back to an absolute date past a week", () => {
    // Nobody counts weeks, so "6 weeks ago" is worse than a date.
    const old = formatRelative(Date.now() - 30 * 86_400_000);
    expect(old).not.toMatch(/ago/);
  });

  it("never says a recording that already exists is in the future", () => {
    // A clock that disagrees with the file's timestamp must not produce
    // "in 3 minutes" on a file sitting on disk.
    const skewed = formatRelative(Date.now() + 3 * 60_000);
    expect(skewed).not.toMatch(/\bin\b/);
  });
});

describe("queue and mode labels", () => {
  it("names the queues it knows", () => {
    expect(queueLabel(420)).toBe("Ranked Solo");
    expect(queueLabel(450)).toBe("ARAM");
  });

  it("names ARAM Mayhem, which a recording on real hardware reported as 2400", () => {
    expect(queueLabel(2400)).toBe("ARAM Mayhem");
  });

  it("shows an unknown queue id rather than guessing a name", () => {
    expect(queueLabel(9999)).toBe("Queue 9999");
  });

  it("distinguishes absent from unknown", () => {
    expect(queueLabel(null)).toBeNull();
    expect(gameModeLabel(null)).toBeNull();
    expect(gameModeLabel("   ")).toBeNull();
  });

  it("normalises the mode string before looking it up", () => {
    expect(gameModeLabel(" practicetool ")).toBe("Practice Tool");
  });

  it("prefers the real queue id over the live mode string", () => {
    // `CLASSIC` cannot tell blind from draft from ranked, so a row that has
    // the LCU's answer must not fall back to the map name.
    expect(queueOrModeLabel(row({ queue: 420, game_mode: "CLASSIC" }))).toBe("Ranked Solo");
    expect(queueOrModeLabel(row({ queue: null, game_mode: "CLASSIC" }))).toBe("Summoner's Rift");
    expect(queueOrModeLabel(row())).toBeNull();
  });
});

describe("patchLabel", () => {
  it("shows the two-part form and keeps the build for the hover", () => {
    expect(patchLabel("15.3.412.9873")).toBe("15.3");
  });

  it("passes through anything it cannot split", () => {
    expect(patchLabel("15")).toBe("15");
    expect(patchLabel(null)).toBeNull();
  });
});

describe("titles and headings", () => {
  it("degrades champion → mode → filename, and is never empty", () => {
    expect(vodTitle(row({ champion: "Viego" }))).toBe("Viego");
    expect(vodTitle(row({ game_mode: "ARAM" }))).toBe("ARAM");
    expect(vodTitle(row())).toBe("2026-09-13_ranked.mp4");
  });

  it("adds each half of the heading only when it is known", () => {
    const viego = row({ champion: "Viego" });
    expect(vodHeading(viego)).toBe("Viego");
    expect(vodHeading(viego, "Darius")).toBe("Viego vs Darius");
    expect(vodHeading(row({ champion: "Viego", win: true }), "Darius")).toBe(
      "Viego vs Darius — Win",
    );
    expect(vodHeading(row({ champion: "Viego", win: false }))).toBe("Viego — Loss");
  });

  it("says nothing about an undecided game rather than implying a result", () => {
    expect(vodHeading(row({ champion: "Viego", win: null }))).toBe("Viego");
  });
});

describe("KDA", () => {
  it("needs all three numbers or reports none", () => {
    expect(formatKda(7, 2, 5)).toBe("7 / 2 / 5");
    expect(formatKda(7, null, 5)).toBeNull();
    expect(kdaRatio(7, null, 5)).toBeNull();
  });

  it("names a deathless game instead of dividing by zero", () => {
    expect(kdaRatio(7, 0, 5)).toBe("Perfect KDA");
  });

  it("has nothing to say about 0/0/0", () => {
    expect(kdaRatio(0, 0, 0)).toBeNull();
  });

  it("computes the ratio to two places", () => {
    expect(kdaRatio(7, 2, 5)).toBe("6.00 KDA");
  });
});

describe("basename", () => {
  it("handles both separators, because the path comes off a Windows row", () => {
    expect(basename("C:\\r\\a.mp4")).toBe("a.mp4");
    expect(basename("/home/n/a.mp4")).toBe("a.mp4");
    expect(basename("a.mp4")).toBe("a.mp4");
  });
});

describe("rank and LP", () => {
  it("title-cases the client's shouting", () => {
    expect(rankLabel("EMERALD", "III")).toBe("Emerald III");
  });

  it("omits the division where divisions do not exist", () => {
    expect(rankLabel("MASTER", null)).toBe("Master");
    expect(rankLabel("MASTER", "  ")).toBe("Master");
  });

  it("has no rank rather than a blank one", () => {
    expect(rankLabel(null, "I")).toBeNull();
    expect(rankLabel("  ", "I")).toBeNull();
  });

  it("shows zero LP, because it is a real standing", () => {
    expect(lpLabel(0)).toBe("0 LP");
    expect(lpLabel(null)).toBeNull();
  });
});
