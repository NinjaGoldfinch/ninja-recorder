import { describe, expect, it } from "vitest";
import type { UpdateStatus } from "../../types";
import { updateRow } from "./update";

describe("updateRow", () => {
  it("says it is checking", () => {
    expect(updateRow({ kind: "checking" } as UpdateStatus).text).toBe("Checking\u2026");
  });

  it("does not claim a build with no updater is up to date", () => {
    // This build will never find out. A devtools bundle and anything built
    // off Windows both land here, and "Up to date." would be a claim neither
    // can support.
    const row = updateRow({ kind: "unsupported" } as UpdateStatus);
    expect(row.text).toBe("Updates are not available in this build.");
    expect(row.offering).toBe(false);
  });

  it("says up to date", () => {
    expect(updateRow({ kind: "upToDate" } as UpdateStatus).text).toBe("Up to date.");
  });

  it("prints a failure verbatim", () => {
    // Rust sends a whole sentence, which is what lets a failed check and a
    // failed install share this one state without the UI guessing which it is
    // looking at.
    const row = updateRow({ kind: "failed", error: "Download failed: 404" } as UpdateStatus);
    expect(row.text).toBe("Download failed: 404");
    expect(row.offering).toBe(false);
  });

  it("offers an installable update, with its notes", () => {
    const row = updateRow({
      kind: "available",
      installable: true,
      offer: { version: "2.1.0", notes: "- a thing" },
    } as UpdateStatus);
    expect(row.text).toBe("Version 2.1.0 is available.");
    expect(row.offering).toBe(true);
    expect(row.installable).toBe(true);
    expect(row.notes).toBe("- a thing");
  });

  it("says why an offered update cannot be installed", () => {
    // A game started. The offer stands; the button does not.
    const row = updateRow({
      kind: "available",
      installable: false,
      blockedReason: "a recording is in progress",
      offer: { version: "2.1.0", notes: null },
    } as UpdateStatus);
    expect(row.text).toBe(
      "Version 2.1.0 is available. Cannot install while a recording is in progress.",
    );
    expect(row.offering).toBe(true);
    expect(row.installable).toBe(false);
  });

  it("falls back to a generic reason rather than saying nothing", () => {
    const row = updateRow({
      kind: "available",
      installable: false,
      offer: { version: "2.1.0", notes: null },
    } as UpdateStatus);
    expect(row.text).toContain("the app is busy");
  });
});
