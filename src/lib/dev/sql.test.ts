import { beforeEach, describe, expect, it } from "vitest";
import {
  allSnippets,
  DEFAULT_SNIPPETS,
  parseCell,
  SNIPPET_KEY,
  savedSnippets,
  saveSnippet,
} from "./sql";

describe("parseCell", () => {
  it("makes an empty field NULL", () => {
    expect(parseCell("")).toBeNull();
    expect(parseCell("   ")).toBeNull();
  });

  it("reads the two booleans SQLite stores as 0 and 1", () => {
    expect(parseCell("true")).toBe(true);
    expect(parseCell("false")).toBe(false);
  });

  it("reads integers and reals", () => {
    expect(parseCell("42")).toBe(42);
    expect(parseCell("-7")).toBe(-7);
    expect(parseCell("1.5")).toBe(1.5);
  });

  it("does not read a number-like string that is not a number", () => {
    // A path or a version is text; a run of digits is a number, leading
    // zeroes and all, because SQLite would store it as one anyway.
    expect(parseCell("1.2.3")).toBe("1.2.3");
    expect(parseCell("1e5")).toBe("1e5");
    expect(parseCell("007")).toBe(7);
  });

  it("parses an object or array", () => {
    expect(parseCell('{"a":1}')).toEqual({ a: 1 });
    expect(parseCell("[1,2]")).toEqual([1, 2]);
  });

  it("falls back to text when an object literal will not parse", () => {
    // A column holding a string that starts with `{` is legal, and the
    // editor is not the thing that should refuse it.
    expect(parseCell("{not json")).toBe("{not json");
  });

  it("leaves ordinary text alone, trimmed", () => {
    expect(parseCell("  Ahri  ")).toBe("Ahri");
  });

  it("treats a quoted number as text, since JSON is only tried for {} and []", () => {
    expect(parseCell('"42"')).toBe('"42"');
  });
});

describe("snippets", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("start as the built-in set", () => {
    expect(savedSnippets()).toEqual([]);
    expect(allSnippets()).toEqual(DEFAULT_SNIPPETS);
  });

  it("append after the built-ins", () => {
    saveSnippet("Mine", "SELECT 1");
    expect(savedSnippets()).toEqual([["Mine", "SELECT 1"]]);
    expect(allSnippets()).toHaveLength(DEFAULT_SNIPPETS.length + 1);
    const all = allSnippets();
    expect(all[all.length - 1]).toEqual(["Mine", "SELECT 1"]);
  });

  it("survive a corrupt store rather than taking the panel with them", () => {
    localStorage.setItem(SNIPPET_KEY, "{not json");
    expect(savedSnippets()).toEqual([]);
    expect(allSnippets()).toEqual(DEFAULT_SNIPPETS);
  });

  it("drop entries that are not a name and a query", () => {
    localStorage.setItem(SNIPPET_KEY, JSON.stringify([["ok", "SELECT 1"], "nope", [1, 2]]));
    expect(savedSnippets()).toEqual([["ok", "SELECT 1"]]);
  });

  it("ignore a store holding something that is not a list", () => {
    localStorage.setItem(SNIPPET_KEY, JSON.stringify({ a: 1 }));
    expect(savedSnippets()).toEqual([]);
  });
});
