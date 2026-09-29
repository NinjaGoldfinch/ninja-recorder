import { afterEach, describe, expect, it, vi } from "vitest";
import type { LcuStatus, RecordingRow } from "../../types";
import {
  clientPill,
  lastSavedLine,
  phaseLabel,
  phaseStep,
  rawPhase,
  recorderLine,
  splitRiotId,
  splitWords,
} from "./client";

/**
 * The app bar's client pill and its card. The raw LCU phase is never what the
 * person reads, and the recorder's state beats the client's phase.
 */

const lcu = (over: Partial<LcuStatus> = {}): LcuStatus => ({
  connected: true,
  phase: "Lobby",
  summoner: "NinjaGoldfinch#OCE",
  profile_icon_id: 29,
  error: null,
  ...over,
});

describe("clientPill", () => {
  it("says it is still checking before the first poll", () => {
    expect(clientPill(null, null, null)).toMatchObject({
      tone: "checking",
      label: "Checking client…",
    });
  });

  it("says the client is closed, and that it cannot reach it", () => {
    expect(clientPill(lcu({ connected: false }), "Idle", null)).toMatchObject({
      tone: "closed",
      label: "League closed",
    });
    expect(clientPill(lcu({ error: "refused" }), "Idle", null)).toMatchObject({
      tone: "error",
      label: "Can't reach client",
    });
  });

  it("words every phase rather than printing it", () => {
    const pill = (phase: string) => clientPill(lcu({ phase }), "ClientRunning", null).label;
    expect(pill("None")).toBe("Home");
    expect(pill("Matchmaking")).toBe("In queue");
    expect(pill("ReadyCheck")).toBe("Match found");
    expect(pill("ChampSelect")).toBe("Champion select");
    expect(pill("GameStart")).toBe("Loading in");
    expect(pill("EndOfGame")).toBe("Post-game");
    expect(pill("Reconnect")).toBe("Reconnecting");
  });

  it("adds the name after a short label, without the tag", () => {
    expect(clientPill(lcu({ phase: "Lobby" }), "ClientRunning", null)).toMatchObject({
      label: "In lobby",
      detail: "NinjaGoldfinch",
    });
    expect(clientPill(lcu({ phase: "ChampSelect" }), "WaitingForGame", null).detail).toBeNull();
  });

  it("lets the recorder take over while it records or saves", () => {
    expect(clientPill(lcu({ phase: "InProgress" }), "Recording", 872)).toEqual({
      tone: "recording",
      glyph: "dot",
      label: "Recording",
      detail: "14:32",
    });
    expect(clientPill(lcu({ phase: "InProgress" }), "Recording", null).detail).toBeNull();
    expect(clientPill(lcu({ phase: "EndOfGame" }), "Finalizing", null)).toMatchObject({
      tone: "saving",
      label: "Saving recording…",
    });
  });

  it("calls an in-game client the loading screen until capture starts", () => {
    expect(clientPill(lcu({ phase: "InProgress" }), "WaitingForGame", null).label).toBe(
      "Loading in",
    );
  });

  it("says Connected for a phase it has no words for", () => {
    const pill = clientPill(lcu({ phase: 'Unknown("Somewhere")' }), "ClientRunning", null);
    expect(pill.label).toBe("Connected");
  });

  it("says the status is unavailable when the poll failed", () => {
    expect(clientPill(lcu(), "Idle", null, true)).toMatchObject({
      tone: "error",
      label: "Status unavailable",
    });
  });
});

describe("phase words", () => {
  it("unwraps the daemon's Debug spelling of an unknown phase", () => {
    expect(rawPhase('Unknown("Brand New")')).toBe("Brand New");
    expect(rawPhase("Lobby")).toBe("Lobby");
    expect(rawPhase(null)).toBeNull();
  });

  it("splits a phase it does not know into words", () => {
    expect(splitWords("TerminatedInError")).toBe("Terminated in error");
    expect(splitWords("NEWPhaseKind")).toBe("New phase kind");
    expect(phaseLabel('Unknown("SomethingNew")')).toBe("Something new");
    expect(phaseLabel("ReadyCheck")).toBe("Accept or decline");
  });
});

describe("splitRiotId", () => {
  it("splits at the last #, so the tag can be drawn quieter", () => {
    expect(splitRiotId("NinjaGoldfinch#OCE")).toEqual({ name: "NinjaGoldfinch", tag: "OCE" });
    expect(splitRiotId("Legacy Name")).toEqual({ name: "Legacy Name", tag: null });
  });

  it("treats the LCU's empty string as nobody", () => {
    expect(splitRiotId("")).toBeNull();
    expect(splitRiotId(null)).toBeNull();
  });
});

describe("phaseStep", () => {
  it("lights the step the client is on", () => {
    expect(phaseStep(lcu({ phase: "Lobby" }))).toBe(0);
    expect(phaseStep(lcu({ phase: "ReadyCheck" }))).toBe(1);
    expect(phaseStep(lcu({ phase: "ChampSelect" }))).toBe(2);
    expect(phaseStep(lcu({ phase: "InProgress" }))).toBe(3);
    expect(phaseStep(lcu({ phase: "WaitingForStats" }))).toBe(4);
  });

  it("leaves the track out at home, and with no client", () => {
    expect(phaseStep(lcu({ phase: "None" }))).toBeNull();
    expect(phaseStep(lcu({ connected: false }))).toBeNull();
    expect(phaseStep(null)).toBeNull();
  });
});

describe("recorderLine", () => {
  const fine = { problem: null, software: null };

  it("says Ready when there is nothing to say", () => {
    expect(recorderLine("ClientRunning", null, "connected", fine)).toEqual({
      tone: "ok",
      text: "Ready",
      title: null,
    });
    // Not read yet is not a problem.
    expect(recorderLine("Idle", null, "connected", null).text).toBe("Ready");
  });

  it("names what is wrong, with the reason on hover", () => {
    expect(
      recorderLine("Idle", null, "connected", { problem: "no worker", software: null }),
    ).toMatchObject({ tone: "warn", text: "Can't record", title: "no worker" });
    expect(
      recorderLine("Idle", null, "connected", { problem: null, software: "no GPU encoder" }),
    ).toMatchObject({ tone: "warn", text: "Software encoding" });
  });

  it("says when this window cannot see the recorder at all", () => {
    expect(recorderLine("Idle", null, "reconnecting", fine).text).toBe("Not running");
    expect(recorderLine("Idle", null, "skewed", fine).text).toBe("Restart needed");
  });

  it("shows recording and saving over everything but a lost daemon", () => {
    expect(recorderLine("Recording", 90, "connected", { problem: "x", software: null })).toEqual({
      tone: "recording",
      text: "Recording · 1:30",
      title: null,
    });
    expect(recorderLine("Finalizing", null, "connected", fine).text).toBe("Saving…");
  });
});

describe("lastSavedLine", () => {
  afterEach(() => vi.useRealTimers());

  const row = (over: Partial<RecordingRow>) =>
    ({ started_at: 0, champion: "Viego", win: true, ...over }) as RecordingRow;

  it("describes the newest recording by champion, result and age", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 29, 12));
    const hoursAgo = (h: number) => Date.now() - h * 3_600_000;
    const line = lastSavedLine([
      row({ started_at: hoursAgo(48), champion: "Ahri" }),
      row({ started_at: hoursAgo(21), champion: "Viego", win: false }),
    ]);
    expect(line).toMatch(/^Viego · Loss · /);
    expect(line).toContain("21");
  });

  it("leaves out a result it does not know, and names an unknown champion plainly", () => {
    expect(lastSavedLine([row({ champion: null, win: null })])).toMatch(/^Recording · /);
  });

  it("is null for an empty library", () => {
    expect(lastSavedLine([])).toBeNull();
  });
});
