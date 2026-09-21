import { describe, expect, it } from "vitest";
import type { LogEntry } from "../../dev/ipc";
import { argSummary, LEVELS, levelTone, toggleLevel, toggleTag, visibleEntries } from "./log";

const entry = (over: Partial<LogEntry>): LogEntry =>
  ({ id: 1, at: 0, command: "dev_health", ms: 1, ok: true, ...over }) as LogEntry;

describe("toggleLevel", () => {
  it("turns one off", () => {
    expect(toggleLevel([...LEVELS], "DEBUG")).toEqual(["ERROR", "WARN", "INFO"]);
  });

  it("turns one back on", () => {
    expect(toggleLevel(["ERROR"], "WARN")).toEqual(["ERROR", "WARN"]);
  });

  it("refuses to turn the last one off", () => {
    // An empty set means "no filter" on the Rust side, so unticking
    // everything would show the whole file rather than nothing.
    expect(toggleLevel(["ERROR"], "ERROR")).toEqual(["ERROR"]);
  });

  it("does not mutate what it was given", () => {
    const levels = [...LEVELS];
    toggleLevel(levels, "DEBUG");
    expect(levels).toEqual([...LEVELS]);
  });
});

describe("toggleTag", () => {
  it("shows a hidden tag by removing it from the hide list", () => {
    expect(toggleTag(["live-poll", "libobs"], "libobs")).toEqual(["live-poll"]);
  });

  it("hides a shown one", () => {
    expect(toggleTag(["live-poll"], "lcu")).toEqual(["live-poll", "lcu"]);
  });

  it("can hide every tag, unlike levels", () => {
    // Hidden tags name what to remove, so an empty list is "hide nothing"
    // and a full one is a legitimate, if quiet, view.
    expect(toggleTag([], "lcu")).toEqual(["lcu"]);
  });
});

describe("levelTone", () => {
  it("marks errors and warnings", () => {
    expect(levelTone("ERROR")).toBe("err");
    expect(levelTone("WARN")).toBe("warn");
  });

  it("leaves everything else plain, including a missing level", () => {
    expect(levelTone("INFO")).toBe("");
    expect(levelTone("")).toBe("");
  });
});

describe("visibleEntries", () => {
  const rows = [
    entry({ id: 1, command: "dev_health" }),
    entry({ id: 2, command: "list_recordings", args: { limit: 50 } }),
    entry({ id: 3, command: "delete_recording", ok: false, error: "no such row" }),
  ];

  it("hides the 1 Hz polls by default", () => {
    const ids = visibleEntries(rows, { search: "", showPolled: false }).map((e) => e.id);
    expect(ids).toEqual([2, 3]);
  });

  it("shows them when asked", () => {
    expect(visibleEntries(rows, { search: "", showPolled: true })).toHaveLength(3);
  });

  it("searches the command name", () => {
    const out = visibleEntries(rows, { search: "list_rec", showPolled: true });
    expect(out.map((e) => e.id)).toEqual([2]);
  });

  it("searches the arguments", () => {
    const out = visibleEntries(rows, { search: "limit", showPolled: true });
    expect(out.map((e) => e.id)).toEqual([2]);
  });

  it("searches the error text, which is how you find a failure", () => {
    const out = visibleEntries(rows, { search: "no such row", showPolled: true });
    expect(out.map((e) => e.id)).toEqual([3]);
  });

  it("ignores case and surrounding space", () => {
    const out = visibleEntries(rows, { search: "  LIST_RECORDINGS ", showPolled: true });
    expect(out.map((e) => e.id)).toEqual([2]);
  });

  it("still hides polls when a search matches one", () => {
    const out = visibleEntries(rows, { search: "dev_health", showPolled: false });
    expect(out).toEqual([]);
  });
});

describe("argSummary", () => {
  it("is empty when there were no arguments", () => {
    expect(argSummary(undefined)).toBe("");
    expect(argSummary(null)).toBe("");
  });

  it("renders small arguments in full", () => {
    expect(argSummary({ id: 1 })).toBe('{"id":1}');
  });

  it("truncates a long one with an ellipsis", () => {
    const out = argSummary({ contents: "x".repeat(200) });
    expect(out).toHaveLength(91);
    expect(out.endsWith("…")).toBe(true);
  });
});
