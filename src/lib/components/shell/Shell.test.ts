import { type Component, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import DaemonStrip from "./DaemonStrip.svelte";
import QuitDialog from "./QuitDialog.svelte";
import Toast from "./Toast.svelte";

/**
 * The three pieces of the shell that are not a view: they sit outside `main`
 * and are true of the whole window.
 */

let host: HTMLElement | null = null;
let instance: Record<string, unknown> | null = null;

/** Generic over the exports, because `QuitDialog` has one and the others do not. */
function render<E extends Record<string, unknown>>(component: Component<Record<string, never>, E>) {
  host = document.createElement("div");
  document.body.append(host);
  instance = mount(component, { target: host });
  return host;
}

/**
 * Mounts and lets the bindings settle.
 *
 * `mount` is synchronous but `bind:this` is assigned by an effect, so a method
 * called in the same tick finds its element undefined. `QuitDialog.ask`
 * answers `false` in that case, by design, which makes it quietly pass a test
 * that expects `false` for a different reason. In the app the question never
 * arises: `App.svelte` hands `ask` over from inside an `$effect`.
 */
async function renderSettled<E extends Record<string, unknown>>(
  component: Component<Record<string, never>, E>,
) {
  const el = render(component);
  await Promise.resolve();
  return el;
}

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host?.remove();
  instance = null;
  host = null;
  vi.useRealTimers();
});

describe("the toast", () => {
  it("shows nothing until there is something to say", async () => {
    const { clearToast } = await import("../../stores/toast.svelte");
    clearToast();
    expect(render(Toast).querySelector(".toast")).toBeNull();
  });

  it("shows a message, and clears itself", async () => {
    vi.useFakeTimers();
    const { toast, clearToast } = await import("../../stores/toast.svelte");
    clearToast();

    const el = render(Toast);
    toast("Rescanning…");
    await Promise.resolve();
    expect(el.querySelector(".toast")?.textContent).toContain("Rescanning");

    vi.advanceTimersByTime(4000);
    await Promise.resolve();
    expect(el.querySelector(".toast")).toBeNull();
  });

  it("keeps an error up longer, because it is usually a sentence", async () => {
    vi.useFakeTimers();
    const { toast, clearToast } = await import("../../stores/toast.svelte");
    clearToast();

    const el = render(Toast);
    toast("Failed to list recordings", "error");
    await Promise.resolve();

    vi.advanceTimersByTime(4000);
    await Promise.resolve();
    expect(el.querySelector(".toast")).not.toBeNull();

    vi.advanceTimersByTime(4000);
    await Promise.resolve();
    expect(el.querySelector(".toast")).toBeNull();
  });
});

describe("the daemon strip", () => {
  it("says nothing while nothing has answered", async () => {
    // "Not known yet" is not "not connected".
    expect(render(DaemonStrip).querySelector(".daemon-strip")).toBeNull();
  });
});

describe("the quit dialog", () => {
  it("answers false when it is dismissed", async () => {
    const el = await renderSettled(QuitDialog);
    const component = instance as unknown as { ask: () => Promise<boolean> };
    const dialog = el.querySelector("dialog");
    if (!dialog) throw new Error("no dialog");

    // Escape, or anything else that is not one of the two buttons.
    const answer = component.ask();
    dialog.close();
    await expect(answer).resolves.toBe(false);
  });

  it("answers true only for the quit button's value", async () => {
    const el = await renderSettled(QuitDialog);
    const component = instance as unknown as { ask: () => Promise<boolean> };
    const dialog = el.querySelector("dialog");
    if (!dialog) throw new Error("no dialog");
    // Through the button, because that is where the choice is recorded. The
    // form's `method="dialog"` closes it; jsdom does not run that, so the
    // close is explicit here.
    // Through the button, which is where the choice is recorded and which
    // closes the dialog itself.
    const answer = component.ask();
    el.querySelector<HTMLButtonElement>(".danger")?.click();
    await expect(answer).resolves.toBe(true);
  });

  it("offers keeping the recording first", async () => {
    // The destructive option is not the default, and is not the one nearest
    // the reading order's start.
    const buttons = (await renderSettled(QuitDialog)).querySelectorAll("button");
    expect(buttons[0].textContent).toContain("Keep recording");
    expect(buttons[1].textContent).toContain("Quit anyway");
    expect(buttons[1].className).toContain("danger");
  });
});
