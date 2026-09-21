import { type ComponentProps, createRawSnippet, mount, unmount } from "svelte";
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

/**
 * One renderer per component, rather than one helper taking a loose props bag.
 *
 * The shared version needed `Component<any>` and a suppression to go with it,
 * and a generic cannot replace it: `Props` appears only in a contravariant
 * position on `Component`, so it is never inferred from the component argument
 * and falls back to the constraint. Six one-line renderers cost less than that
 * and buy what the loose version gave away, which is that every call below is
 * checked against the props the component actually declares.
 */
const hosts: HTMLElement[] = [];
const mounted: Record<string, unknown>[] = [];

function attach<T extends Record<string, unknown>>(host: HTMLElement, instance: T): HTMLElement {
  hosts.push(host);
  mounted.push(instance);
  return host;
}

function target(): HTMLElement {
  const host = document.createElement("div");
  document.body.append(host);
  return host;
}

const pill = (props: ComponentProps<typeof Pill>) => {
  const host = target();
  return attach(host, mount(Pill, { target: host, props }));
};
const card = (props: ComponentProps<typeof Card>) => {
  const host = target();
  return attach(host, mount(Card, { target: host, props }));
};
const panelHead = (props: ComponentProps<typeof PanelHead>) => {
  const host = target();
  return attach(host, mount(PanelHead, { target: host, props }));
};
const keyValues = (props: ComponentProps<typeof KeyValues>) => {
  const host = target();
  return attach(host, mount(KeyValues, { target: host, props }));
};
const output = (props: ComponentProps<typeof Output>) => {
  const host = target();
  return attach(host, mount(Output, { target: host, props }));
};
/** No props at all, so it takes none: `ComponentProps` of a propless
 *  component is `Record<string, never>`, which `mount` will not accept. */
const devToasts = () => {
  const host = target();
  return attach(host, mount(DevToasts, { target: host }));
};

/** Every mount, not just the last one: several tests below render twice in a
 *  row to compare, and the old single-slot cleanup leaked the first. */
afterEach(async () => {
  for (const instance of mounted.splice(0)) await unmount(instance, { outro: false });
  for (const host of hosts.splice(0)) host.remove();
});

describe("Pill", () => {
  it("shows a label and a value", () => {
    const el = pill({ label: "state", value: "Recording" });
    expect(el.textContent).toContain("state");
    expect(el.querySelector("b")?.textContent).toBe("Recording");
  });

  it("carries the tone as a class, and nothing when there is none", () => {
    expect(
      pill({ label: "free", value: "1 GB", tone: "danger" }).querySelector(".pill-danger"),
    ).not.toBeNull();
    expect(pill({ label: "free", value: "1 GB" }).querySelector('[class*="pill-"]')).toBeNull();
  });

  it("marks a live value, for one that is actively changing", () => {
    expect(
      pill({ label: "state", value: "x", live: true }).querySelector(".pill-live"),
    ).not.toBeNull();
  });
});

describe("Card", () => {
  /** `children` is required, and a card with no body is not a case. */
  const body = createRawSnippet(() => ({ render: () => "<p>body</p>" }));

  it("omits the heading entirely when there is no title", () => {
    const el = card({ title: "", children: body });
    expect(el.querySelector("h2")).toBeNull();
    expect(el.textContent).toContain("body");
  });

  it("marks a raw title, for a path or a command name", () => {
    // The default treatment uppercases, which mangles a literal system string.
    expect(
      card({ title: "C:/vods", raw: true, children: body }).querySelector("h2")?.className,
    ).toContain("raw");
    expect(card({ title: "Backend", children: body }).querySelector("h2")?.className).not.toContain(
      "raw",
    );
  });
});

describe("PanelHead", () => {
  it("renders the title and its explanation as text", () => {
    const el = panelHead({ title: "Recorder", description: "<b>not markup</b>" });
    expect(el.querySelector("h1")?.textContent).toBe("Recorder");
    expect(el.querySelectorAll("b")).toHaveLength(0);
  });
});

describe("KeyValues", () => {
  it("renders a term and a definition per pair", () => {
    const el = keyValues({
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
    const el = keyValues({
      pairs: [
        ["Output", null],
        ["Free", undefined],
        ["Path", ""],
      ],
    });
    expect(el.querySelectorAll("dd .hint")).toHaveLength(3);
  });

  it("does not treat zero as absent", () => {
    const el = keyValues({ pairs: [["Markers", "0"]] });
    expect(el.querySelector("dd .hint")).toBeNull();
    expect(el.querySelector("dd")?.textContent?.trim()).toBe("0");
  });

  it("renders a value that looks like markup as text", () => {
    const el = keyValues({ pairs: [["Path", "<img src=x onerror=alert(1)>"]] });
    expect(el.querySelectorAll("img")).toHaveLength(0);
    expect(el.querySelector("dd")?.textContent?.trim()).toBe("<img src=x onerror=alert(1)>");
  });
});

describe("Output", () => {
  it("pretty-prints an object", () => {
    const el = output({ value: { ok: true, count: 2 } });
    expect(el.querySelector("pre")?.textContent).toContain('"count": 2');
  });

  it("passes a string through unquoted", () => {
    // Command results are often already a sentence; JSON-quoting them would
    // add noise to the thing being read.
    expect(output({ value: "Recording started." }).textContent).toBe("Recording started.");
  });

  it("marks an error", () => {
    expect(output({ value: "boom", isError: true }).querySelector(".output-error")).not.toBeNull();
  });

  it("renders backend output that looks like markup as text", () => {
    const el = output({ value: "<script>alert(1)</script>" });
    expect(el.querySelectorAll("script")).toHaveLength(0);
    expect(el.textContent).toBe("<script>alert(1)</script>");
  });

  it("survives a value JSON cannot represent", () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(() => output({ value: cyclic })).not.toThrow();
  });
});

describe("DevToasts", () => {
  it("renders nothing when the stack is empty", async () => {
    const { clearDevToasts } = await import("../../stores/devToast.svelte");
    clearDevToasts();
    expect(devToasts().querySelector(".toast-stack")).toBeNull();
  });

  it("stacks rather than replacing, so a batch stays readable", async () => {
    const { devToast, clearDevToasts } = await import("../../stores/devToast.svelte");
    clearDevToasts();

    const el = devToasts();
    devToast("Seeded 5");
    devToast("Seeded 10", "ok");
    await Promise.resolve();

    expect(el.querySelectorAll(".toast")).toHaveLength(2);
    expect(el.querySelectorAll(".toast.ok")).toHaveLength(1);
    clearDevToasts();
  });
});
