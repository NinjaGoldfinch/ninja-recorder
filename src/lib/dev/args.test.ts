import { describe, expect, it } from "vitest";
import type { PortalArg } from "../../dev/commands.generated";
import { collectArgs, defaultFor } from "./args";

const arg = (over: Partial<PortalArg>): PortalArg => ({
  name: "x",
  kind: "string",
  default: "",
  help: "",
  optional: false,
  ...over,
});

describe("collectArgs", () => {
  it("passes a string through, trimmed", () => {
    const out = collectArgs([arg({ name: "path" })], { path: "  C:/a.mp4  " });
    expect(out).toEqual({ ok: true, args: { path: "C:/a.mp4" } });
  });

  it("coerces a number", () => {
    const out = collectArgs([arg({ name: "id", kind: "number" })], { id: "42" });
    expect(out.ok && out.args.id).toBe(42);
  });

  it("refuses a number that is not one", () => {
    const out = collectArgs([arg({ name: "id", kind: "number" })], { id: "abc" });
    expect(out).toEqual({ ok: false, error: "id is not a number" });
  });

  it("parses JSON", () => {
    const out = collectArgs([arg({ name: "policy", kind: "json" })], { policy: '{"a":1}' });
    expect(out.ok && out.args.policy).toEqual({ a: 1 });
  });

  it("names the field and the reason when JSON will not parse", () => {
    const out = collectArgs([arg({ name: "policy", kind: "json" })], { policy: "{" });
    expect(out.ok).toBe(false);
    expect(!out.ok && out.error).toContain("policy is not valid JSON");
  });

  it("reads a checkbox", () => {
    expect(collectArgs([arg({ name: "force", kind: "boolean" })], { force: "true" })).toEqual({
      ok: true,
      args: { force: true },
    });
    expect(collectArgs([arg({ name: "force", kind: "boolean" })], { force: "false" })).toEqual({
      ok: true,
      args: { force: false },
    });
  });

  describe("an omitted optional", () => {
    it("is absent, not null", () => {
      // Several commands distinguish "not given" from an explicit null.
      // Sending null for an empty box would turn every optional field into a
      // delete.
      const out = collectArgs([arg({ name: "note", optional: true })], { note: "" });
      expect(out.ok && out.args).toEqual({});
      expect(out.ok && "note" in out.args).toBe(false);
    });

    it("is still absent when the field was never rendered", () => {
      const out = collectArgs([arg({ name: "note", optional: true })], {});
      expect(out.ok && out.args).toEqual({});
    });
  });

  it("refuses an empty required field, naming it", () => {
    const out = collectArgs([arg({ name: "path" })], { path: "   " });
    expect(out).toEqual({ ok: false, error: "path is required" });
  });

  it("stops at the first problem rather than reporting the last", () => {
    const out = collectArgs([arg({ name: "a" }), arg({ name: "b", kind: "number" })], {
      a: "",
      b: "nope",
    });
    expect(!out.ok && out.error).toContain("a is required");
  });

  it("falls back to a spec's default when the field is untouched", () => {
    const out = collectArgs([arg({ name: "which", default: "fixtures" })], {});
    expect(out.ok && out.args.which).toBe("fixtures");
  });

  it("takes no arguments as an empty payload rather than refusing", () => {
    expect(collectArgs([], {})).toEqual({ ok: true, args: {} });
  });
});

describe("defaultFor", () => {
  it("is empty for a field the generator gave no default", () => {
    expect(defaultFor(arg({}))).toBe("");
  });

  it("is false for a checkbox with no default, so the box starts unticked", () => {
    expect(defaultFor(arg({ kind: "boolean" }))).toBe("false");
  });

  it("is the declared default otherwise", () => {
    expect(defaultFor(arg({ kind: "number", default: "5" }))).toBe("5");
    expect(defaultFor(arg({ kind: "boolean", default: "true" }))).toBe("true");
  });
});
