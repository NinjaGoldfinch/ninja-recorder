import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/**
 * A strip, not a toast: it is true until it stops being true.
 *
 * The rendering is three lines, and the one thing worth pinning is that
 * nothing is rendered when there is nothing to say. A strip that left an empty
 * bar above the views would take vertical space from every window that is
 * working correctly.
 */

const daemon = vi.hoisted(() => ({ strip: null as { kind: string; text: string } | null }));
vi.mock("../../stores/daemon.svelte", () => ({ daemon }));

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
let DaemonStrip: typeof import("./DaemonStrip.svelte").default;

beforeEach(async () => {
  daemon.strip = null;
  DaemonStrip = (await import("./DaemonStrip.svelte")).default;
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await unmount(instance, { outro: false });
  host.remove();
  instance = null;
});

describe("DaemonStrip", () => {
  it("renders nothing at all when the recorder is there", () => {
    instance = mount(DaemonStrip, { target: host });
    expect(host.querySelector(".daemon-strip")).toBeNull();
    expect(host.textContent?.trim()).toBe("");
  });

  it("says what is wrong, and carries the kind for styling", () => {
    daemon.strip = { kind: "error", text: "The recorder is not running." };
    instance = mount(DaemonStrip, { target: host });
    const strip = host.querySelector(".daemon-strip");
    expect(strip?.textContent?.trim()).toBe("The recorder is not running.");
    expect(strip?.getAttribute("data-kind")).toBe("error");
  });

  it("announces itself politely rather than interrupting", () => {
    // It appears while the user is doing something else, so it must not
    // preempt a screen reader mid-sentence.
    daemon.strip = { kind: "warn", text: "Reconnecting…" };
    instance = mount(DaemonStrip, { target: host });
    const strip = host.querySelector(".daemon-strip");
    expect(strip?.getAttribute("role")).toBe("status");
    expect(strip?.getAttribute("aria-live")).toBe("polite");
  });
});
