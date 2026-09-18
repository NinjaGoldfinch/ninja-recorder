import { describe, expect, it } from "vitest";
import type { MarkerRow } from "../../types";
import {
  CLUSTER_PX,
  clusterCentre,
  clusterMarkers,
  leadMarker,
  MARKER_PRIORITY,
  markerRank,
} from "./clusters";

/**
 * Clustering decides what the timeline looks like around a teamfight, which
 * is the part of a VOD anyone actually goes back for. It was inline in
 * `renderGlyphs`, between a `getBoundingClientRect` and an `innerHTML`
 * assignment, until WS4.2.
 */

function marker(video_time_s: number, kind = "kill", id = 0): MarkerRow {
  return {
    id,
    recording_id: 1,
    game_time_s: video_time_s,
    video_time_s,
    kind,
    payload_json: "{}",
  };
}

/** One pixel per second, so the fixtures read as the distances they are. */
const perSecond = (m: MarkerRow) => m.video_time_s;

describe("clusterMarkers", () => {
  it("leaves well-separated markers alone", () => {
    const markers = [marker(0), marker(100), marker(200)];
    expect(clusterMarkers(markers, perSecond, 10)).toEqual([
      [markers[0]],
      [markers[1]],
      [markers[2]],
    ]);
  });

  it("collapses markers closer than the threshold", () => {
    const markers = [marker(0), marker(5), marker(9)];
    const clusters = clusterMarkers(markers, perSecond, 10);
    expect(clusters).toHaveLength(1);
    expect(clusters[0]).toHaveLength(3);
  });

  it("includes a marker exactly at the threshold", () => {
    expect(clusterMarkers([marker(0), marker(10)], perSecond, 10)).toHaveLength(1);
    expect(clusterMarkers([marker(0), marker(10.01)], perSecond, 10)).toHaveLength(2);
  });

  it("measures from where the cluster started, not from the previous marker", () => {
    // The property that stops a dense run chaining into one cluster spanning
    // the whole track. Each marker is 6 apart, so a previous-marker
    // comparison would swallow all four; against the cluster's own origin,
    // 12 and 18 start new ones.
    const markers = [marker(0), marker(6), marker(12), marker(18)];
    const clusters = clusterMarkers(markers, perSecond, 10);
    expect(clusters.map((c) => c.length)).toEqual([2, 2]);
  });

  it("returns nothing for no markers", () => {
    expect(clusterMarkers([], perSecond, 10)).toEqual([]);
  });

  it("calls the position function once per marker", () => {
    let calls = 0;
    const counted = (m: MarkerRow) => {
      calls += 1;
      return m.video_time_s;
    };
    clusterMarkers([marker(0), marker(5), marker(100)], counted, 10);
    expect(calls).toBe(3);
  });

  it("defaults to the shipped threshold", () => {
    // Guards against the default drifting away from the constant the comment
    // in `clusters.ts` argues for.
    expect(clusterMarkers([marker(0), marker(CLUSTER_PX - 1)], perSecond)).toHaveLength(1);
    expect(clusterMarkers([marker(0), marker(CLUSTER_PX + 1)], perSecond)).toHaveLength(2);
  });
});

describe("markerRank", () => {
  it("orders by the priority list", () => {
    expect(markerRank("multikill")).toBeLessThan(markerRank("kill"));
    expect(markerRank("kill")).toBeLessThan(markerRank("assist"));
  });

  it("sorts an unknown kind last rather than first", () => {
    // `kind` is a TEXT column. An unrecognised value must not win a cluster
    // and hide the death that is actually in it.
    expect(markerRank("something_new")).toBe(MARKER_PRIORITY.length);
    expect(markerRank("something_new")).toBeGreaterThan(markerRank("assist"));
  });
});

describe("leadMarker", () => {
  it("picks the highest-priority marker in the cluster", () => {
    const cluster = [marker(0, "assist"), marker(1, "death"), marker(2, "turret")];
    expect(leadMarker(cluster).kind).toBe("death");
  });

  it("does not reorder the cluster it was given", () => {
    // Load-bearing: the glyph's position comes from `cluster[0]`, so sorting
    // in place would move it to whichever marker happened to win the icon.
    const cluster = [marker(0, "assist"), marker(1, "death")];
    leadMarker(cluster);
    expect(cluster[0].kind).toBe("assist");
  });

  it("handles a cluster of one", () => {
    expect(leadMarker([marker(5, "turret")]).kind).toBe("turret");
  });

  it("falls back to the first when nothing is recognised", () => {
    const cluster = [marker(0, "a"), marker(1, "b")];
    expect(leadMarker(cluster).kind).toBe("a");
  });
});

describe("clusterCentre", () => {
  it("is the mean video time", () => {
    expect(clusterCentre([marker(10), marker(20), marker(30)])).toBe(20);
  });

  it("is the marker's own time for a cluster of one", () => {
    expect(clusterCentre([marker(42)])).toBe(42);
  });
});
