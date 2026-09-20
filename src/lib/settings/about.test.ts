import { describe, expect, it } from "vitest";
import type { LcuStatus, SupervisorStatus } from "../../types";
import { finalizedLine, gamePill, lcuLine, lcuPill } from "./about";

describe("lcuLine", () => {
  it("reports an error over everything else", () => {
    expect(lcuLine({ error: "boom", connected: true } as LcuStatus)).toBe("Error: boom");
  });

  it("says the client is not running", () => {
    expect(lcuLine({ connected: false } as LcuStatus)).toBe("Not running (no lockfile found).");
  });

  it("names who is signed in, and the phase", () => {
    expect(lcuLine({ connected: true, summoner: "Ninja", phase: "Lobby" } as LcuStatus)).toBe(
      "Connected as Ninja \u2014 phase Lobby.",
    );
  });

  it("falls back on falsiness, not nullishness", () => {
    // The LCU hands back an empty string, not null, when it has no Riot ID
    // for us yet. `??` would print "Connected as .".
    expect(lcuLine({ connected: true, summoner: "" } as LcuStatus)).toContain("signed in");
  });

  it("says the phase is unknown rather than printing undefined", () => {
    expect(lcuLine({ connected: true, summoner: "Ninja" } as LcuStatus)).toContain("phase ?");
  });
});

describe("lcuPill", () => {
  it("is the short form of the same answer", () => {
    expect(lcuPill({ error: "x" } as LcuStatus).state).toBe("error");
    expect(lcuPill({ connected: false } as LcuStatus).state).toBe("offline");
    expect(lcuPill({ connected: true, summoner: "Ninja" } as LcuStatus)).toEqual({
      state: "online",
      copy: "Ninja",
    });
  });
});

describe("gamePill", () => {
  it("names each state", () => {
    expect(gamePill("Idle", null).copy).toBe("Idle");
    expect(gamePill("ClientRunning", null).copy).toBe("Waiting for a game");
    expect(gamePill("Finalizing", null).copy).toBe("Saving\u2026");
  });

  it("shows the elapsed time only while recording", () => {
    expect(gamePill("Recording", 90).copy).toContain("1:30");
    expect(gamePill("Idle", 90).copy).toBe("Idle");
  });

  it("omits the clock when there is no elapsed time yet", () => {
    expect(gamePill("Recording", null).copy).toBe("Recording");
  });
});

describe("finalizedLine", () => {
  it("says none yet", () => {
    expect(finalizedLine({ last_finalized: null } as SupervisorStatus)).toBe("None yet.");
  });

  it("names the file, the markers and the row", () => {
    const line = finalizedLine({
      last_finalized: { path: "C:/a.mp4", markers: [1, 2], recording_id: 7 },
    } as unknown as SupervisorStatus);
    expect(line).toBe("C:/a.mp4 (2 markers, db id 7)");
  });

  it("says so loudly when the file exists but its row does not", () => {
    // A null recording_id means the recording succeeded and the DB write did
    // not, which is worth saying rather than rendering as a blank.
    const line = finalizedLine({
      last_finalized: { path: "C:/a.mp4", markers: [], recording_id: null },
    } as unknown as SupervisorStatus);
    expect(line).toContain("DB WRITE FAILED");
  });
});
