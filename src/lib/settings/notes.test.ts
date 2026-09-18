import { describe, expect, it } from "vitest";
import { inlineSpans, parseNotes } from "./notes";

/**
 * `latest.json` is fetched over HTTPS but is **not covered by the update
 * signature** - only the installer it points at is - so every string here is
 * remote text this app did not write. That is the reason these notes have
 * always been built as nodes rather than markup, and the reason the last test
 * in this file exists.
 */

describe("inlineSpans", () => {
  it("leaves a plain line alone", () => {
    expect(inlineSpans("hello")).toEqual([{ text: "hello", strong: false }]);
  });

  it("picks out emphasis", () => {
    // The one piece of inline markdown the notes actually contain: GitHub's
    // generated notes end with `**Full changelog**: <url>`.
    expect(inlineSpans("**Full changelog**: https://x")).toEqual([
      { text: "Full changelog", strong: true },
      { text: ": https://x", strong: false },
    ]);
  });

  it("handles emphasis in the middle", () => {
    expect(inlineSpans("a **b** c")).toEqual([
      { text: "a ", strong: false },
      { text: "b", strong: true },
      { text: " c", strong: false },
    ]);
  });

  it("shows an unclosed marker as written rather than half-parsing it", () => {
    expect(inlineSpans("**not closed")).toEqual([{ text: "**not closed", strong: false }]);
  });

  it("is non-greedy across two runs", () => {
    expect(
      inlineSpans("**a** and **b**")
        .filter((s) => s.strong)
        .map((s) => s.text),
    ).toEqual(["a", "b"]);
  });

  it("returns nothing for an empty line", () => {
    expect(inlineSpans("")).toEqual([]);
  });
});

describe("parseNotes", () => {
  it("returns nothing when there are no notes", () => {
    expect(parseNotes(null)).toEqual([]);
    expect(parseNotes("")).toEqual([]);
    expect(parseNotes("   \n\n  ")).toEqual([]);
  });

  it("drops the heading, because the row is already labelled", () => {
    expect(parseNotes("## What's changed\n- a")).toEqual([
      { kind: "list", items: [[{ text: "a", strong: false }]] },
    ]);
  });

  it("collects consecutive bullets into one list", () => {
    const blocks = parseNotes("- one\n- two\n- three");
    expect(blocks).toHaveLength(1);
    expect(blocks[0].kind).toBe("list");
    expect(blocks[0].kind === "list" && blocks[0].items).toHaveLength(3);
  });

  it("ends the list at a non-bullet, and starts a new one after", () => {
    const blocks = parseNotes("- one\nsomething\n- two");
    expect(blocks.map((b) => b.kind)).toEqual(["list", "para", "list"]);
  });

  it("shows an unrecognised line rather than swallowing it", () => {
    // The format is what CI writes today; a line that is not a bullet is
    // still something the user is entitled to read.
    expect(parseNotes("just a sentence")).toEqual([
      { kind: "para", spans: [{ text: "just a sentence", strong: false }] },
    ]);
  });

  it("parses the shape CI actually writes", () => {
    const blocks = parseNotes(
      "## What's changed\n- fix: a thing\n- feat: another\n\n**Full changelog**: https://x",
    );
    expect(blocks.map((b) => b.kind)).toEqual(["list", "para"]);
    expect(blocks[0].kind === "list" && blocks[0].items).toHaveLength(2);
  });

  it("carries markup through as text, never as structure", () => {
    // The guarantee. `latest.json` is not signed, so a manifest could carry
    // this; it has to come out the other side as a string a template will
    // interpolate, not as anything a renderer would act on.
    const blocks = parseNotes("- <img src=x onerror=alert(1)>");
    expect(blocks).toEqual([
      { kind: "list", items: [[{ text: "<img src=x onerror=alert(1)>", strong: false }]] },
    ]);
  });
});
