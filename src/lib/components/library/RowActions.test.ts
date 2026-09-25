import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RecordingRow } from "../../../types";
import RowActions from "./RowActions.svelte";

/**
 * The two-step delete.
 *
 * It is a confirmation on the button itself rather than a modal: cheaper, and
 * it keeps the destructive action next to the thing it destroys. In
 * `library.ts` the armed flag could not live on the button, because the grid
 * was rebuilt with `innerHTML` underneath it; this is the behaviour that
 * arrangement existed to produce, tested now that it is `$state` beside the
 * element.
 */

const row = { id: 1, pinned: false } as RecordingRow;

let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

function render(props: Partial<Record<string, unknown>> = {}) {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(RowActions, {
    target: host,
    props: {
      row,
      onreview: () => {},
      onpin: () => {},
      ondelete: () => {},
      oninspect: () => {},
      showInspect: false,
      ...props,
    },
  });
  return host;
}

const deleteButton = (el: HTMLElement) =>
  el.querySelector<HTMLButtonElement>('[aria-label="Delete recording"]');

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
  vi.useRealTimers();
});

describe("the delete button", () => {
  it("arms on the first click rather than deleting", async () => {
    const ondelete = vi.fn();
    const el = render({ ondelete });

    deleteButton(el)?.click();
    await Promise.resolve();

    expect(ondelete).not.toHaveBeenCalled();
    expect(deleteButton(el)?.textContent).toContain("Delete?");
  });

  it("deletes on the second", async () => {
    const ondelete = vi.fn();
    const el = render({ ondelete });

    deleteButton(el)?.click();
    await Promise.resolve();
    deleteButton(el)?.click();
    await Promise.resolve();

    expect(ondelete).toHaveBeenCalledOnce();
  });

  it("disarms itself after four seconds", async () => {
    vi.useFakeTimers();
    const ondelete = vi.fn();
    const el = render({ ondelete });

    deleteButton(el)?.click();
    await Promise.resolve();
    expect(deleteButton(el)?.textContent).toContain("Delete?");

    vi.advanceTimersByTime(4000);
    await Promise.resolve();
    expect(deleteButton(el)?.textContent).not.toContain("Delete?");
  });

  it("goes back to needing two clicks after it disarms", async () => {
    vi.useFakeTimers();
    const ondelete = vi.fn();
    const el = render({ ondelete });

    deleteButton(el)?.click();
    await Promise.resolve();
    vi.advanceTimersByTime(4000);
    await Promise.resolve();

    deleteButton(el)?.click();
    await Promise.resolve();
    expect(ondelete).not.toHaveBeenCalled();
  });
});

describe("the pin button", () => {
  it("reports the recording's state and asks to flip it", async () => {
    const onpin = vi.fn();
    const el = render({ onpin });
    const pin = el.querySelector<HTMLButtonElement>(".pin-btn");
    expect(pin?.getAttribute("aria-pressed")).toBe("false");

    pin?.click();
    await Promise.resolve();
    expect(onpin).toHaveBeenCalledOnce();
  });
});

describe("the review button", () => {
  it("opens the review form for its row", () => {
    const onreview = vi.fn();
    const el = render({ onreview });
    el.querySelector<HTMLButtonElement>('[aria-label="Review this game"]')?.click();
    expect(onreview).toHaveBeenCalledWith(row);
  });
});
