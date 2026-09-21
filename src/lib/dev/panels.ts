/**
 * The portal's panel registry, and the hash route that reaches one.
 *
 * Pure, so the routing can be tested without a portal. `main.ts` parsed the
 * hash inside the function that also mounted a panel and cleared a payload,
 * which is why its one real subtlety had a comment and no test.
 */

export interface PanelMeta {
  id: string;
  title: string;
  icon: string;
  group: string;
}

/** Order is the sidebar's order; `group` breaks it into labelled sections. */
export const PANELS: readonly PanelMeta[] = [
  { id: "overview", title: "Overview", icon: "◉", group: "Status" },
  { id: "recorder", title: "Recorder", icon: "⏺", group: "Status" },
  { id: "simulate", title: "Simulate", icon: "▶", group: "Status" },
  { id: "database", title: "Database", icon: "▦", group: "Data" },
  { id: "seed", title: "Seed", icon: "✦", group: "Data" },
  { id: "retention", title: "Retention", icon: "⚖", group: "Data" },
  { id: "fixtures", title: "Fixtures", icon: "❑", group: "Data" },
  { id: "commands", title: "Commands", icon: "⌘", group: "Tools" },
  { id: "library", title: "Library", icon: "▤", group: "Tools" },
  { id: "diagnostics", title: "Diagnostics", icon: "◍", group: "Tools" },
  { id: "log", title: "Log", icon: "☰", group: "Tools" },
];

/**
 * What every panel component is handed.
 *
 * One shape for all eleven, because `DevApp` picks the component out of a map
 * and cannot vary the props per entry. Only Library reads it; the rest declare
 * nothing and are assignable all the same.
 */
export interface PanelProps {
  payload?: string | null;
}

export interface Route {
  panel: PanelMeta;
  /**
   * The one path segment after the panel id, or null.
   *
   * `#/library/12` is what lets the main window's rows open the portal *on* a
   * recording rather than at an empty list.
   */
  payload: string | null;
}

/**
 * Which panel a hash names, and what it carries.
 *
 * **An unknown id falls back to the first panel and carries nothing.** The
 * payload is dropped rather than passed on: `#/overview/12` would otherwise
 * hand `12` to whichever panel is opened next, because Overview consumes
 * nothing and the value would still be sitting there.
 */
export function routeFor(hash: string): Route {
  const [id, arg] = hash.replace(/^#\/?/, "").split("/");
  const panel = PANELS.find((p) => p.id === id);
  if (!panel) return { panel: PANELS[0], payload: null };
  return { panel, payload: arg || null };
}

/** The sidebar, as groups in order, each with its panels. */
export function navGroups(): { group: string; panels: PanelMeta[] }[] {
  const groups: { group: string; panels: PanelMeta[] }[] = [];
  for (const panel of PANELS) {
    const last = groups[groups.length - 1];
    if (last?.group === panel.group) last.panels.push(panel);
    else groups.push({ group: panel.group, panels: [panel] });
  }
  return groups;
}
