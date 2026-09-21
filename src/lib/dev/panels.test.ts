import { describe, expect, it } from "vitest";
import { navGroups, PANELS, routeFor } from "./panels";

describe("routeFor", () => {
  it("finds the panel a hash names", () => {
    expect(routeFor("#/database").panel.id).toBe("database");
    expect(routeFor("#/log").panel.id).toBe("log");
  });

  it("carries one segment as the payload", () => {
    // `#/library/12` is what lets the main window's rows open the portal *on*
    // a recording rather than at an empty list.
    const route = routeFor("#/library/12");
    expect(route.panel.id).toBe("library");
    expect(route.payload).toBe("12");
  });

  it("falls back to the first panel for an empty or unknown hash", () => {
    expect(routeFor("").panel.id).toBe(PANELS[0].id);
    expect(routeFor("#/").panel.id).toBe(PANELS[0].id);
    expect(routeFor("#/not-a-panel").panel.id).toBe(PANELS[0].id);
  });

  it("drops the payload when it falls back", () => {
    // Otherwise `#/overview/12` hands `12` to whichever panel is opened next,
    // because the fallback consumes nothing and the value stays set.
    expect(routeFor("#/not-a-panel/12").payload).toBeNull();
  });

  it("treats a trailing slash as no payload", () => {
    expect(routeFor("#/library/").payload).toBeNull();
  });

  it("ignores anything past the first segment", () => {
    expect(routeFor("#/library/12/extra").payload).toBe("12");
  });
});

describe("navGroups", () => {
  it("keeps the registry's order and breaks it into labelled runs", () => {
    const groups = navGroups();
    expect(groups.map((g) => g.group)).toEqual(["Status", "Data", "Tools"]);
    expect(groups[0].panels.map((p) => p.id)).toEqual(["overview", "recorder", "simulate"]);
  });

  it("lists every panel exactly once", () => {
    const listed = navGroups().flatMap((g) => g.panels.map((p) => p.id));
    expect(listed).toHaveLength(PANELS.length);
    expect(new Set(listed).size).toBe(PANELS.length);
  });
});

describe("the registry", () => {
  it("has no duplicate ids, which the router resolves by", () => {
    const ids = PANELS.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});
