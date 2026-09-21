import { describe, expect, it } from "vitest";

/**
 * **Nothing in this frontend turns a string into markup.**
 *
 * `db::reconcile` imports whatever video file is dropped into the recordings
 * folder, so a filename is untrusted input, and `vodTitle` falls back to one.
 * v1 escaped by hand at every interpolation site; the migration's real safety
 * win was replacing that with Svelte's default text interpolation, which
 * cannot be forgotten. `{@html}` opts straight back out of it, and so does an
 * `innerHTML` assignment.
 *
 * `Row.test.ts` and `UpdateNotes.test.ts` prove the two most exposed
 * components render their inputs as text. This is the structural half: it
 * says no *future* component can reintroduce the hole, which is the part a
 * per-component test can never cover.
 *
 * If a case for `{@html}` ever arrives, it needs an entry in `ALLOWED` with a
 * reason, a sanitiser, and a test of its own. Adding one should be a
 * deliberate, reviewed act, which is exactly what failing this makes it.
 */

const svelte = import.meta.glob("./**/*.svelte", {
  query: "?raw",
  eager: true,
  import: "default",
}) as Record<string, string>;

const scripts = import.meta.glob("./**/*.ts", {
  query: "?raw",
  eager: true,
  import: "default",
}) as Record<string, string>;

/** Deliberate exceptions. Empty, and that is the point. */
const ALLOWED: string[] = [];

/**
 * Tests are exempt from the sink check, and only from that one.
 *
 * `main.boot.test.ts` loads the real `index.html` by assigning it, which is
 * how it proves the app boots. A test cannot introduce this vulnerability:
 * nothing it renders reaches a user, and none of it ships. The `{@html}`
 * check below stays unconditional, because components have no test variant
 * to hide in.
 */
const isTest = (path: string) => /\.test\.ts$/.test(path);

/**
 * Prose talks about these on purpose - the rule is explained where it
 * applies - so the comments come out before the search. Erring strict is
 * still right for a security guard: a false positive here is one comment to
 * reword, and a false negative is the hole reopened.
 */
function withoutComments(source: string): string {
  return source
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:])\/\/.*$/gm, "$1");
}

const MARKUP_SINKS = /\.(inner|outer)HTML\s*=|insertAdjacentHTML|document\.write/;

describe("no string becomes markup", () => {
  it("is actually looking at the tree", () => {
    // A glob that matches nothing passes every assertion below it forever.
    expect(Object.keys(svelte).length).toBeGreaterThan(40);
    expect(Object.keys(scripts).length).toBeGreaterThan(40);
    expect(Object.keys(svelte)).toContain("./lib/components/library/Row.svelte");
  });

  it("finds no {@html} in any component", () => {
    const offenders = Object.entries(svelte)
      .filter(([path]) => !ALLOWED.includes(path))
      .filter(([, source]) => /\{@html\b/.test(withoutComments(source)))
      .map(([path]) => path);

    expect(offenders, "{@html} bypasses the escaping the migration bought").toEqual([]);
  });

  it("finds no innerHTML, outerHTML, insertAdjacentHTML or document.write", () => {
    // The escape helpers went with `src/dev/ui.ts` in #72, so an assignment
    // here would not merely be discouraged: there is nothing left to escape
    // with.
    const offenders = [...Object.entries(svelte), ...Object.entries(scripts)]
      .filter(([path]) => !isTest(path) && !ALLOWED.includes(path))
      .filter(([, source]) => MARKUP_SINKS.test(withoutComments(source)))
      .map(([path]) => path);

    expect(offenders).toEqual([]);
  });

  it("would catch a component that reintroduced it", () => {
    // Proves the matcher, not the tree: without this, a typo in the regex
    // makes every assertion above vacuous.
    const stripped = withoutComments(`
      <!-- No {@html} here, says the comment. -->
      <script lang="ts">let value = "";</script>
      {@html value}
    `);
    expect(/\{@html\b/.test(stripped)).toBe(true);
    expect(MARKUP_SINKS.test("el.innerHTML = markup;")).toBe(true);
    expect(MARKUP_SINKS.test("node.insertAdjacentHTML('beforeend', s)")).toBe(true);
  });

  it("does not trip over prose that names them", () => {
    const stripped = withoutComments(`
      <!-- **No {@html} anywhere in this file**, and innerHTML is gone too. -->
      <script lang="ts">
      /* The grid was rebuilt with innerHTML on every change. */
      // {@html} opts back out of it.
      </script>
      <p>{title}</p>
    `);
    expect(/\{@html\b/.test(stripped)).toBe(false);
    expect(MARKUP_SINKS.test(stripped)).toBe(false);
  });
});
