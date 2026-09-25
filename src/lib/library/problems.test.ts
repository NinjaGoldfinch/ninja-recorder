import { describe, expect, it } from "vitest";
import type { CaptureProblem } from "../contract/types";
import {
  type CaptureProblemsEvent,
  lossLabel,
  noticeFor,
  recordedWithout,
  sourceLabel,
  storedProblems,
} from "./problems";

/**
 * The words for a capture failure (#10), in the strip and on the recording.
 *
 * What these pin is that the reason, which is whatever a Windows call said,
 * reaches the person whole (the call and its HRESULT are what make a report
 * useful), and that a missing or malformed blob is silence rather than a row
 * that fails to render.
 */

const refused: CaptureProblem = {
  kind: "sourceFailed",
  source: "game",
  reason: "process-loopback activation for PID 4242 was refused: Access is denied. (0x80070005)",
};

function event(
  problems: CaptureProblem[],
  windowsBuild: number | null = 19045,
): CaptureProblemsEvent {
  return { type: "captureProblems", recordingId: 3, windowsBuild, problems };
}

describe("sourceLabel", () => {
  it("names each source as the notification does", () => {
    expect(sourceLabel("game")).toBe("game audio");
    expect(sourceLabel("microphone")).toBe("microphone audio");
    expect(sourceLabel("desktop")).toBe("desktop audio");
    expect(sourceLabel("Discord.exe")).toBe("Discord audio");
    expect(sourceLabel("tool.EXE")).toBe("tool audio");
    expect(sourceLabel("tool")).toBe("tool audio");
  });

  it("says what each kind of loss cost", () => {
    expect(lossLabel(refused)).toBe("game audio");
    expect(lossLabel({ kind: "sourceEnded", source: "microphone", reason: "x" })).toBe(
      "part of the microphone audio",
    );
    expect(lossLabel({ kind: "endedEarly", reason: "x" })).toBe("the end of the game");
  });
});

describe("noticeFor", () => {
  it("names the source, the failing call and the build, and asks for a report", () => {
    expect(noticeFor(event([refused]))).toEqual({
      kind: "warn",
      text:
        "The last recording was saved without game audio (process-loopback activation for " +
        "PID 4242 was refused: Access is denied. (0x80070005)). If this keeps happening, please " +
        "report it with your Windows version (Windows build 19045).",
    });
  });

  it("covers everything one recording lost in one notice", () => {
    const text = noticeFor(
      event([refused, { kind: "sourceEnded", source: "Discord.exe", reason: "gone." }], null),
    )?.text;
    expect(text).toContain("game audio (process-loopback");
    expect(text).toContain("; part of the Discord audio (gone)");
    expect(text).toContain("(an unknown Windows build)");
  });

  it("is an error when there is no recording at all", () => {
    const notice = noticeFor(
      event([{ kind: "notStarted", reason: "recorder backend error: no frame from WGC" }]),
    );
    expect(notice?.kind).toBe("error");
    expect(notice?.text).toMatch(/^This game was not recorded: recorder backend error: no frame/);
    const unsaved = noticeFor(event([{ kind: "notSaved", reason: "the worker died" }]));
    expect(unsaved?.text).toMatch(/^The last recording could not be saved: the worker died\./);
  });

  it("says nothing for nothing, or for a kind this build does not know", () => {
    expect(noticeFor(event([]))).toBeNull();
    const future = { kind: "somethingNew", reason: "x" } as unknown as CaptureProblem;
    expect(noticeFor(event([future]))).toBeNull();
  });
});

describe("recordedWithout", () => {
  const stored = (problems: unknown) =>
    JSON.stringify({ backend: "own (ready: x)", markers: 0, capture_problems: problems });

  it("gives a short line for the row and every reason for the review page", () => {
    expect(recordedWithout(stored([refused]))).toEqual({
      short: "Recorded without game audio",
      full:
        "Recorded without game audio (process-loopback activation for PID 4242 was refused: " +
        "Access is denied. (0x80070005)).",
    });
    const two = recordedWithout(
      stored([refused, { kind: "endedEarly", reason: "the GPU device was lost (0x887A0005)" }]),
    );
    expect(two?.short).toBe("Recorded without game audio and the end of the game");
    expect(two?.full).toContain("; the end of the game (the GPU device was lost (0x887A0005)).");
  });

  it("is nothing for a clean row, an old row, an imported row or a broken blob", () => {
    expect(recordedWithout(null)).toBeNull();
    expect(recordedWithout("")).toBeNull();
    expect(recordedWithout('{"backend":"libobs","markers":3}')).toBeNull();
    expect(recordedWithout(stored([]))).toBeNull();
    expect(recordedWithout("{not json")).toBeNull();
    expect(recordedWithout("null")).toBeNull();
    expect(recordedWithout(stored("not a list"))).toBeNull();
  });

  it("skips entries that are not problems rather than trusting them", () => {
    const mixed = stored([
      refused,
      { kind: "sourceFailed", reason: "no source" },
      { kind: "endedEarly" },
      "text",
      null,
    ]);
    expect(storedProblems(mixed)).toEqual([refused]);
  });
});
