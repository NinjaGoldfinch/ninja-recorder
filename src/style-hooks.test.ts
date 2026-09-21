import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import devHtml from "../dev.html?raw";
import indexHtml from "../index.html?raw";

/**
 * Read from disk rather than imported.
 *
 * Vitest runs with `css: false`, so every way of importing a stylesheet as
 * text - `?raw`, `?inline`, `import.meta.glob` - resolves to an empty string.
 * A guard reading an empty string passes forever, which is worse than not
 * having one. See `src/node-fs.d.ts` for why `fs` is declared rather than
 * installed.
 */
const appCss = readFileSync("src/lib/styles/app.css", "utf8");
const devCss = readFileSync("src/lib/styles/dev.css", "utf8");

/**
 * **Every id the stylesheets select has to exist in the markup.**
 *
 * This is the guard for a failure that has already shipped once. `app.css`
 * styles the review player through `#review-video`, and WS4.5 rebuilt that
 * element as a Svelte component holding a `bind:this` reference, which needs
 * no id. The attribute went, the rule stopped matching, and the `<video>` fell
 * back to its intrinsic size: a 1920-wide element inside a wrapper with
 * `overflow: hidden`, so the player rendered enormous and cropped. Fullscreen
 * still looked right, because there the wrapper fills the screen either way.
 *
 * Every gate was green. Nothing in this repo renders a pixel in CI, so the
 * only thing that could have caught it is the structural claim below: the
 * stylesheet and the markup disagree about an id.
 *
 * It says nothing about whether the page *looks* right, and cannot. What it
 * says is that a hook the stylesheet depends on still has something to hold.
 */

const components = import.meta.glob("./**/*.svelte", {
  query: "?raw",
  eager: true,
  import: "default",
}) as Record<string, string>;

/** Ids in the markup, from both entry documents and every component. */
const declared = new Set<string>();
for (const source of [indexHtml, devHtml, ...Object.values(components)]) {
  for (const match of source.matchAll(/\bid=["']([^"']+)["']/g)) {
    declared.add(match[1]);
  }
}

/**
 * Id selectors in a stylesheet.
 *
 * Hex colours are the obvious false positive and are filtered by shape rather
 * than by context: `#fff`, `#396cd8` and `#0e1016ff` are all valid ids as far
 * as a regex is concerned, and none of them is one here.
 */
function idSelectors(css: string): string[] {
  const withoutComments = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const found = new Set<string>();
  for (const match of withoutComments.matchAll(/#([a-zA-Z][\w-]*)/g)) {
    const name = match[1];
    if (/^([0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(name)) continue;
    found.add(name);
  }
  return [...found].sort();
}

describe("the stylesheets' hooks exist", () => {
  it("is reading real files", () => {
    // A `?raw` import that resolved to nothing would make every assertion
    // below pass forever.
    expect(appCss.length).toBeGreaterThan(1000);
    expect(devCss.length).toBeGreaterThan(1000);
    expect(declared.size).toBeGreaterThan(1);
    expect(declared.has("app-root")).toBe(true);
  });

  it("app.css selects no id the markup does not declare", () => {
    const orphans = idSelectors(appCss).filter((id) => !declared.has(id));
    expect(orphans, "a rule that matches nothing renders as a styling bug").toEqual([]);
  });

  it("dev.css selects no id the markup does not declare", () => {
    const orphans = idSelectors(devCss).filter((id) => !declared.has(id));
    expect(orphans).toEqual([]);
  });

  it("would have caught the player", () => {
    // Proves the matcher rather than the tree: `#review-video` is the exact
    // selector that broke, and a regex that stopped finding id selectors would
    // otherwise make the two assertions above vacuous.
    expect(idSelectors(appCss)).toContain("review-video");
    expect(idSelectors("#review-video { width: 100% }")).toEqual(["review-video"]);
  });

  it("does not mistake a colour for a hook", () => {
    expect(idSelectors(":root { --a: #fff; --b: #396cd8; --c: #0e1016ff; }")).toEqual([]);
  });

  it("ignores ids named only in a comment", () => {
    expect(idSelectors("/* #gone was deleted in WS4.6 */ .a { color: red }")).toEqual([]);
  });
});
