import { describe, expect, it } from "vitest";
import type { AudioLayout } from "../../types";
import { hasStems, parseAudioLayout, videoErrorReport } from "./playback";

describe("parseAudioLayout", () => {
  it("reads a layout the recorder wrote", () => {
    const layout = parseAudioLayout(JSON.stringify({ tracks: [{ label: "Everything" }] }));
    expect(layout?.tracks).toHaveLength(1);
  });

  it("is null for a recording we did not make", () => {
    // A rescan-imported file, or a VOD from before multi-track audio existed.
    expect(parseAudioLayout(null)).toBeNull();
  });

  it("treats an unreadable layout as an unknown one", () => {
    // The picker hides rather than guessing at a file's contents.
    expect(parseAudioLayout("{")).toBeNull();
    expect(parseAudioLayout(JSON.stringify({ tracks: "not an array" }))).toBeNull();
    expect(parseAudioLayout(JSON.stringify({}))).toBeNull();
  });
});

describe("hasStems", () => {
  it("needs two tracks to be worth offering", () => {
    // One track is nothing to choose between.
    expect(hasStems(null)).toBe(false);
    expect(hasStems({ tracks: [{ label: "Game" }] } as AudioLayout)).toBe(false);
    expect(hasStems({ tracks: [{ label: "A" }, { label: "B" }] } as AudioLayout)).toBe(true);
  });
});

describe("videoErrorReport", () => {
  it("names Matroska as the cause for an .mkv, whatever the code", () => {
    // WebView2 has no Matroska demuxer at all, so an .mkv fails regardless of
    // how valid its contents are. Most likely to bite someone testing with an
    // OBS recording, since .mkv is OBS's crash-safe default.
    const report = videoErrorReport(4, null, "file:///a.mkv", "C:/vods/a.mkv");
    expect(report.message).toContain("Matroska");
    expect(report.message).toContain("ffmpeg -i in.mkv -c copy out.mp4");
  });

  it("is case-insensitive about the extension", () => {
    expect(videoErrorReport(3, null, "x", "C:/vods/A.MKV").message).toContain("Matroska");
  });

  it("suspects HEVC for a decode failure on an mp4", () => {
    // The single most common real-world cause: many capture tools default to
    // H.265, and WebView2 cannot decode it without a codec pack.
    for (const code of [3, 4]) {
      const report = videoErrorReport(code, null, "x", "C:/vods/a.mp4");
      expect(report.message).toContain("H.265/HEVC");
    }
  });

  it("does not suspect HEVC for a network failure", () => {
    const report = videoErrorReport(2, null, "x", "C:/vods/a.mp4");
    expect(report.message).not.toContain("HEVC");
    expect(report.message).toContain("Network error");
  });

  it("names the code it does not recognise rather than staying silent", () => {
    expect(videoErrorReport(99, null, "x", "C:/vods/a.mp4").message).toContain("error code 99");
  });

  it("falls back when there is no error object at all", () => {
    expect(videoErrorReport(null, null, "x", null).message).toBe(
      "This recording's video couldn't be played.",
    );
  });

  it("puts the technical half in the detail, with the source", () => {
    // What someone reporting it needs, kept out of the sentence they read.
    const report = videoErrorReport(3, "PIPELINE_ERROR_DECODE", "file:///a.mp4", "C:/a.mp4");
    expect(report.detail).toContain("PIPELINE_ERROR_DECODE");
    expect(report.detail).toContain("src: file:///a.mp4");
  });

  it("still reports the source when the error carries no message", () => {
    expect(videoErrorReport(4, null, "file:///a.mp4", null).detail).toBe("src: file:///a.mp4");
  });
});
