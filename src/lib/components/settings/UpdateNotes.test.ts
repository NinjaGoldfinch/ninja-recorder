import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import UpdateNotes from "./UpdateNotes.svelte";

/**
 * **`latest.json` is not covered by the update signature** - only the
 * installer it points at is - so everything this component renders is remote
 * text the app did not write.
 *
 * `update.ts` built these notes as DOM nodes with `createElement` and
 * `textContent` for exactly that reason. This is the test that fails if an
 * `{@html}` is ever introduced in its place.
 */

let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

function render(notes: string | null): HTMLElement {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(UpdateNotes, { target: host, props: { notes } });
  return host;
}

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
});

describe("UpdateNotes", () => {
  it("renders nothing when there are no notes", () => {
    expect(render(null).querySelector(".update-notes")).toBeNull();
    expect(render("").querySelector(".update-notes")).toBeNull();
  });

  it("renders bullets as a list", () => {
    const el = render("- one\n- two");
    expect(el.querySelectorAll("li")).toHaveLength(2);
    expect(el.querySelectorAll("li")[0].textContent?.trim()).toBe("one");
  });

  it("renders emphasis as a real element, from the parsed structure", () => {
    const el = render("**Full changelog**: https://x");
    expect(el.querySelector("strong")?.textContent).toBe("Full changelog");
    expect(el.querySelector("p")?.textContent).toContain(": https://x");
  });

  it("drops the heading CI writes", () => {
    expect(render("## What's changed\n- a").textContent).not.toContain("What's changed");
  });

  it("renders markup in the manifest as text", () => {
    // The guarantee. A manifest is fetched over HTTPS and is *not* signed, so
    // this is the one path where remote text reaches the DOM.
    const nasty = "<img src=x onerror=alert(1)>";
    const el = render(`- ${nasty}`);

    expect(el.querySelectorAll("img")).toHaveLength(0);
    expect(el.querySelector("li")?.textContent?.trim()).toBe(nasty);
  });

  it("does not let a manifest inject an element through emphasis either", () => {
    const el = render("**<script>alert(1)</script>**");
    expect(el.querySelectorAll("script")).toHaveLength(0);
    expect(el.querySelector("strong")?.textContent).toBe("<script>alert(1)</script>");
  });
});
