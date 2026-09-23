import { describe, expect, it } from "vitest";
import type { CaptureBackendStatus } from "../contract/types";
import { refusalNote, unavailableNotes } from "./capture";

const NOT_BUILT = "the own capture backend is not in this build yet";

/** Today's build, as the daemon reports it. */
function today(over: Partial<CaptureBackendStatus> = {}): CaptureBackendStatus {
  return {
    configured: "libobs",
    active: "libobs (idle)",
    options: [
      { backend: "libobs", unavailable: null },
      { backend: "own", unavailable: NOT_BUILT },
    ],
    ...over,
  };
}

describe("unavailableNotes", () => {
  it("names each backend that cannot be built, with the daemon's reason", () => {
    expect(unavailableNotes(today())).toEqual([`Own isn't available: ${NOT_BUILT}.`]);
  });

  it("says nothing when both can be built", () => {
    const status = today({
      options: [
        { backend: "libobs", unavailable: null },
        { backend: "own", unavailable: null },
      ],
    });
    expect(unavailableNotes(status)).toEqual([]);
  });
});

describe("refusalNote", () => {
  it("is quiet while the saved backend is one this build can construct", () => {
    expect(refusalNote(today())).toBeNull();
  });

  // A downgrade from a build that had the own backend leaves this behind, and
  // the daemon then records nothing. The row has to say so.
  it("warns that nothing will be recorded when the saved backend is unavailable", () => {
    const note = refusalNote(today({ configured: "own", active: `unavailable (${NOT_BUILT})` }));
    expect(note).toContain("Nothing will be recorded");
    expect(note).toContain(NOT_BUILT);
  });
});
