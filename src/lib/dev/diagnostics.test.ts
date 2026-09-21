import { describe, expect, it } from "vitest";
import type { RecordingDiagnostics, RecordingRow } from "../../dev/types";
import { concerns, parseDiagnostics, pollingStoppedEarlyBy, recordingName } from "./diagnostics";

const diag = (over: Partial<RecordingDiagnostics> = {}): RecordingDiagnostics =>
  ({
    game_id: 1,
    queue_id: 420,
    is_custom: false,
    polls: 100,
    first_game_time_s: 0,
    last_game_time_s: 1500,
    ever_matched: true,
    alignment_offset_s: 10,
    backend: "libobs",
    markers: 12,
    samples: 1400,
    ...over,
  }) as RecordingDiagnostics;

const row = (over: Partial<RecordingRow> = {}): RecordingRow =>
  ({ id: 1, path: "C:/vods/a.mp4", champion: "Ahri", duration_s: 1510, ...over }) as RecordingRow;

describe("concerns", () => {
  it("says nothing when nothing looked wrong", () => {
    expect(concerns(row(), diag())).toEqual([]);
  });

  it("leads with a poller that never got a snapshot", () => {
    const out = concerns(row(), diag({ polls: 0, ever_matched: false }));
    expect(out[0]).toContain("never got a single snapshot");
  });

  it("does not also blame allPlayers when there were no polls at all", () => {
    // Two explanations for one cause read as two problems.
    const out = concerns(row(), diag({ polls: 0, ever_matched: false }));
    expect(out.filter((c) => c.includes("allPlayers"))).toHaveLength(0);
  });

  it("explains an empty card by the allPlayers match", () => {
    const out = concerns(row(), diag({ ever_matched: false }));
    expect(out[0]).toContain("allPlayers");
    expect(out[0]).toContain("no champion");
  });

  it("says a missing game id is permanent", () => {
    // Queue, role and patch come from the deferred LCU patch, which needs it.
    const out = concerns(row(), diag({ game_id: null }));
    expect(out.some((c) => c.includes("can never be filled in"))).toBe(true);
  });

  it("warns that markers may be misplaced when the clock never advanced", () => {
    const out = concerns(row(), diag({ alignment_offset_s: null }));
    expect(out.some((c) => c.includes("1:1 alignment"))).toBe(true);
  });

  it("does not warn about alignment when there were no polls", () => {
    // It would be a second sentence about the same absence.
    const out = concerns(row(), diag({ polls: 0, alignment_offset_s: null }));
    expect(out.some((c) => c.includes("1:1 alignment"))).toBe(false);
  });

  it("spots polling that died well before the recorder did", () => {
    // The #74 fingerprint.
    const out = concerns(
      row({ duration_s: 1600 }),
      diag({ last_game_time_s: 1500, alignment_offset_s: 10 }),
    );
    expect(out.some((c) => c.includes("Polling stopped about 90s"))).toBe(true);
  });

  it("ignores a gap inside the normal tail", () => {
    // A few seconds between the last poll and the stop is every recording.
    expect(concerns(row({ duration_s: 1515 }), diag())).toEqual([]);
  });

  it("reports a backend that named itself as failed", () => {
    const out = concerns(row(), diag({ backend: "libobs-failed" }));
    expect(out.some((c) => c.includes("libobs-failed"))).toBe(true);
  });

  it("can report more than one thing at once", () => {
    const out = concerns(row(), diag({ ever_matched: false, game_id: null }));
    expect(out).toHaveLength(2);
  });
});

describe("pollingStoppedEarlyBy", () => {
  it("is null when any of the three numbers is missing", () => {
    expect(pollingStoppedEarlyBy(row({ duration_s: null }), diag())).toBeNull();
    expect(pollingStoppedEarlyBy(row(), diag({ last_game_time_s: null }))).toBeNull();
    expect(pollingStoppedEarlyBy(row(), diag({ alignment_offset_s: null }))).toBeNull();
  });

  it("measures from where the last poll lands in the video", () => {
    expect(pollingStoppedEarlyBy(row({ duration_s: 1600 }), diag())).toBe(90);
  });
});

describe("parseDiagnostics", () => {
  it("is null for a recording that has none", () => {
    // Predates migration 7, or a rescan imported it from a file we did not
    // record, or serializing failed at finalize.
    expect(parseDiagnostics(row({ diagnostics_json: null }))).toBeNull();
  });

  it("is null rather than throwing on a malformed record", () => {
    expect(parseDiagnostics(row({ diagnostics_json: "{" }))).toBeNull();
  });

  it("reads a good one", () => {
    expect(parseDiagnostics(row({ diagnostics_json: '{"polls":3}' }))?.polls).toBe(3);
  });
});

describe("recordingName", () => {
  it("prefers the champion", () => {
    expect(recordingName(row({ champion: "Ahri" }))).toBe("Ahri");
  });

  it("falls back to the filename, on either separator", () => {
    expect(recordingName(row({ champion: null, path: "C:\\vods\\a.mp4" }))).toBe("a.mp4");
    expect(recordingName(row({ champion: null, path: "/home/vods/b.mp4" }))).toBe("b.mp4");
  });
});
