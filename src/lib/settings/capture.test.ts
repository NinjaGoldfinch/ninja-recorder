import { describe, expect, it } from "vitest";
import type { CaptureBackendStatus } from "../contract/types";
import { automaticNote, refusalNote, softwareNote, unavailableNotes } from "./capture";

const NOT_BUILT = "the own capture backend is not in this build yet";

/** Today's build, as the daemon reports it. */
function today(over: Partial<CaptureBackendStatus> = {}): CaptureBackendStatus {
  return {
    configured: "libobs",
    automatic: false,
    active: "libobs (idle)",
    software_encoding: null,
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

describe("softwareNote", () => {
  it("is quiet while the backend encodes in hardware", () => {
    expect(softwareNote(today({ configured: "own", active: "own (ready: NVENC)" }))).toBeNull();
  });

  // DEVELOPMENT.md §2.4: the software fallback is allowed only if the user is
  // told, and the daemon's field is what says so, not the wording of `active`.
  it("says the recording costs more CPU when the daemon reports software encoding", () => {
    const note = softwareNote(
      today({
        configured: "own",
        active: "own (software encoding: H264 Encoder MFT, because no hardware GPU was found)",
        software_encoding: "no hardware GPU was found",
      }),
    );
    expect(note).toBe(
      "Recording is encoding video in software, because no hardware GPU was found. It uses " +
        "noticeably more CPU than a graphics card's encoder, which can cost frame rate in game.",
    );
  });

  // #296: forced on a devtools build, the notice blamed "no usable hardware
  // encoder" under an "In use now" line that said it was forced.
  it("gives the daemon's reason, the one the In use now line gives, not a fixed one", () => {
    const forced = "forced by NINJA_OWN_FORCE_SOFTWARE_ENCODER (devtools)";
    const note = softwareNote(
      today({
        configured: "own",
        active: `own (software encoding: H264 Encoder MFT, because ${forced})`,
        software_encoding: forced,
      }),
    );
    expect(note).toContain(`in software, because ${forced}. It uses`);
    expect(note).not.toContain("no usable hardware encoder");
  });

  it("is quiet for an empty reason, and ends one sentence with one full stop", () => {
    expect(softwareNote(today({ software_encoding: "" }))).toBeNull();
    expect(softwareNote(today({ software_encoding: "no GPU." }))).toContain("because no GPU. It");
  });
});

describe("automaticNote", () => {
  it("is quiet once a choice is saved", () => {
    expect(automaticNote(today())).toBeNull();
  });

  it("says Own is the automatic pick where it can be built", () => {
    const status = today({
      configured: "own",
      automatic: true,
      options: [
        { backend: "libobs", unavailable: null },
        { backend: "own", unavailable: null },
      ],
    });
    expect(automaticNote(status)).toBe("Automatic: Own, the default.");
  });

  // Below the floor with nothing saved: the daemon records on libobs, and the row
  // says so with the daemon's reason for Own.
  it("says libobs is the automatic pick, and why, where Own cannot be built", () => {
    expect(automaticNote(today({ automatic: true }))).toBe(
      `Automatic: libobs, because ${NOT_BUILT}.`,
    );
  });

  it("leaves it to the refusal when nothing can be built", () => {
    const status = today({
      configured: "own",
      automatic: true,
      options: [
        { backend: "libobs", unavailable: "the libobs worker is not beside the executable" },
        { backend: "own", unavailable: NOT_BUILT },
      ],
    });
    expect(automaticNote(status)).toBeNull();
    expect(refusalNote(status)).toContain("neither capture backend is available");
  });
});
