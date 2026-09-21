import { type Component, createRawSnippet, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import Card from "./Card.svelte";
import DevToasts from "./DevToasts.svelte";
import KeyValues from "./KeyValues.svelte";
import Output from "./Output.svelte";
import PanelHead from "./PanelHead.svelte";
import Pill from "./Pill.svelte";

/**
 * The portal's presentational vocabulary.
 *
 * Small, but not trivial: each of these was a string-concatenating function in
 * `ui.ts` that escaped its own inputs, and the inputs are backend data. What
 * is asserted here is the handful of decisions inside them, plus that nothing
 * they render is parsed as markup.
 */

let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

/**
 * A shared mount helper. The props bag is loose on purpose: each of these
 * components has its own shape, and naming six of them to type one test
 * helper is more ceremony than the helper is worth.
 */
function render(
  // biome-ignore lint/suspicious/noExplicitAny: see above.
  component: Component<any>,
  props: Record<string, unknown>,
) {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(component, { target: host, props });
  return host;
}

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
});

describe("Pill", () => {
  it("shows a label and a value", () => {
    const el = render(Pill, { label: "state", value: "Recording" });
    expect(el.textContent).toContain("state");
    expect(el.querySelector("b")?.textContent).toBe("Recording");
  });

  it("carries the tone as a class, and nothing when there is none", () => {
    expect(
      render(Pill, { label: "free", value: "1 GB", tone: "danger" }).querySelector(".pill-danger"),
    ).not.toBeNull();
    expect(
      render(Pill, { label: "free", value: "1 GB" }).querySelector('[class*="pill-"]'),
    ).toBeNull();
  });

  it("marks a live value, for one that is actively changing", () => {
    expect(
      render(Pill, { label: "state", value: "x", live: true }).querySelector(".pill-live"),
    ).not.toBeNull();
  });
});

describe("Card", () => {
  /** `children` is required, and a card with no body is not a case. */
  const body = createRawSnippet(() => ({ render: () => "<p>body</p>" }));

  it("omits the heading entirely when there is no title", () => {
    const el = render(Card, { title: "", children: body });
    expect(el.querySelector("h2")).toBeNull();
    expect(el.textContent).toContain("body");
  });

  it("marks a raw title, for a path or a command name", () => {
    // The default treatment uppercases, which mangles a literal system string.
    expect(
      render(Card, { title: "C:/vods", raw: true, children: body }).querySelector("h2")?.className,
    ).toContain("raw");
    expect(
      render(Card, { title: "Backend", children: body }).querySelector("h2")?.className,
    ).not.toContain("raw");
  });
});

describe("PanelHead", () => {
  it("renders the title and its explanation as text", () => {
    const el = render(PanelHead, { title: "Recorder", description: "<b>not markup</b>" });
    expect(el.querySelector("h1")?.textContent).toBe("Recorder");
    expect(el.querySelectorAll("b")).toHaveLength(0);
  });
});

describe("KeyValues", () => {
  it("renders a term and a definition per pair", () => {
    const el = render(KeyValues, {
      pairs: [
        ["Active", "stub"],
        ["Platform", "windows/x86_64"],
      ],
    });
    expect(el.querySelectorAll("dt")).toHaveLength(2);
    expect(el.querySelectorAll("dd")[0].textContent?.trim()).toBe("stub");
  });

  it("dashes an absent value rather than leaving the line empty", () => {
    // A row that has nothing to say still occupies its line, so the list does
    // not change shape between renders.
    const el = render(KeyValues, {
      pairs: [
        ["Output", null],
        ["Free", undefined],
        ["Path", ""],
      ],
    });
    expect(el.querySelectorAll("dd .hint")).toHaveLength(3);
  });

  it("does not treat zero as absent", () => {
    const el = render(KeyValues, { pairs: [["Markers", "0"]] });
    expect(el.querySelector("dd .hint")).toBeNull();
    expect(el.querySelector("dd")?.textContent?.trim()).toBe("0");
  });

  it("renders a value that looks like markup as text", () => {
    const el = render(KeyValues, { pairs: [["Path", "<img src=x onerror=alert(1)>"]] });
    expect(el.querySelectorAll("img")).toHaveLength(0);
    expect(el.querySelector("dd")?.textContent?.trim()).toBe("<img src=x onerror=alert(1)>");
  });
});

describe("Output", () => {
  it("pretty-prints an object", () => {
    const el = render(Output, { value: { ok: true, count: 2 } });
    expect(el.querySelector("pre")?.textContent).toContain('"count": 2');
  });

  it("passes a string through unquoted", () => {
    // Command results are often already a sentence; JSON-quoting them would
    // add noise to the thing being read.
    expect(render(Output, { value: "Recording started." }).textContent).toBe("Recording started.");
  });

  it("marks an error", () => {
    expect(
      render(Output, { value: "boom", isError: true }).querySelector(".output-error"),
    ).not.toBeNull();
  });

  it("renders backend output that looks like markup as text", () => {
    const el = render(Output, { value: "<script>alert(1)</script>" });
    expect(el.querySelectorAll("script")).toHaveLength(0);
    expect(el.textContent).toBe("<script>alert(1)</script>");
  });

  it("survives a value JSON cannot represent", () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(() => render(Output, { value: cyclic })).not.toThrow();
  });
});

describe("DevToasts", () => {
  it("renders nothing when the stack is empty", async () => {
    const { clearDevToasts } = await import("../../stores/devToast.svelte");
    clearDevToasts();
    expect(render(DevToasts, {}).querySelector(".toast-stack")).toBeNull();
  });

  it("stacks rather than replacing, so a batch stays readable", async () => {
    const { devToast, clearDevToasts } = await import("../../stores/devToast.svelte");
    clearDevToasts();

    const el = render(DevToasts, {});
    devToast("Seeded 5");
    devToast("Seeded 10", "ok");
    await Promise.resolve();

    expect(el.querySelectorAll(".toast")).toHaveLength(2);
    expect(el.querySelectorAll(".toast.ok")).toHaveLength(1);
    clearDevToasts();
  });
});
