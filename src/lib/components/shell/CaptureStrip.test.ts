import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Event } from "../../contract/types";

/**
 * The capture-failure strip (#10), rendered from the daemon's event.
 *
 * Driven through the real store and the real subscription, with only the
 * transport faked, so what is pinned is the whole path from a `captureProblems`
 * event to text on the screen, and that the reason (untrusted: it is what a
 * Windows call said) is shown as text and never interpreted as markup.
 */

let push: ((event: Event) => void) | undefined;

let host: HTMLElement;
let instance: Record<string, unknown> | null = null;
/** Imported after the module reset, so the component and this share one runtime. */
let svelte: typeof import("svelte");

async function load() {
  vi.resetModules();
  push = undefined;
  vi.doMock("../../transport/invoke", () => ({ IN_TAURI: true }));
  vi.doMock("../../transport/pipe", () => ({
    subscribe: (handlers: { onEvent?: (event: Event) => void }) => {
      push = handlers.onEvent;
      return () => {};
    },
  }));
  svelte = await import("svelte");
  const store = await import("../../stores/capture.svelte");
  store.initCaptureNotices();
  const CaptureStrip = (await import("./CaptureStrip.svelte")).default;
  instance = svelte.mount(CaptureStrip, { target: host });
  return store;
}

function send(event: Event) {
  if (!push) throw new Error("the store never subscribed");
  push(event);
  svelte.flushSync();
}

const refused: Event = {
  type: "captureProblems",
  recordingId: 12,
  windowsBuild: 19045,
  problems: [
    {
      kind: "sourceFailed",
      source: "game",
      reason: "process-loopback activation for PID 1 was refused: (0x80070005)",
    },
  ],
};

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
});

afterEach(async () => {
  if (instance) await svelte.unmount(instance, { outro: false });
  instance = null;
  host.remove();
});

describe("CaptureStrip", () => {
  it("renders nothing until a recording has lost something", async () => {
    await load();
    expect(host.querySelector(".capture-strip")).toBeNull();
    send({ type: "recordingStopped", recordingId: 1, outcome: { kind: "clean" } });
    expect(host.querySelector(".capture-strip")).toBeNull();
  });

  it("says what was lost, why, and on which build, from the event", async () => {
    await load();
    send(refused);
    const strip = host.querySelector(".capture-strip");
    expect(strip?.getAttribute("data-kind")).toBe("warn");
    expect(strip?.getAttribute("role")).toBe("status");
    const text = strip?.querySelector(".capture-strip-text")?.textContent ?? "";
    expect(text).toContain("saved without game audio");
    expect(text).toContain("(0x80070005)");
    expect(text).toContain("Windows build 19045");
  });

  it("is an error when the game was not recorded at all", async () => {
    await load();
    send({
      type: "captureProblems",
      recordingId: null,
      windowsBuild: 19041,
      problems: [{ kind: "notStarted", reason: "no frame from WGC" }],
    });
    expect(host.querySelector(".capture-strip")?.getAttribute("data-kind")).toBe("error");
  });

  it("stays until dismissed, and the next problem replaces it", async () => {
    await load();
    send(refused);
    const dismiss = host.querySelector<HTMLButtonElement>(".capture-strip-dismiss");
    expect(dismiss?.getAttribute("type")).toBe("button");
    dismiss?.click();
    svelte.flushSync();
    expect(host.querySelector(".capture-strip")).toBeNull();

    send({
      type: "captureProblems",
      recordingId: 13,
      windowsBuild: null,
      problems: [{ kind: "endedEarly", reason: "the GPU device was lost" }],
    });
    expect(host.querySelector(".capture-strip-text")?.textContent).toContain(
      "the end of the game (the GPU device was lost)",
    );
  });

  /** The reason is untrusted: markup in it is shown, not built. */
  it("shows markup in a reason as text", async () => {
    await load();
    const hostile = '<img src=x onerror="alert(1)"><b>bold</b>';
    send({
      type: "captureProblems",
      recordingId: 1,
      windowsBuild: 1,
      problems: [{ kind: "sourceFailed", source: "<i>game</i>", reason: hostile }],
    });
    const strip = host.querySelector(".capture-strip");
    expect(strip?.querySelector("img, b, i")).toBeNull();
    expect(strip?.textContent).toContain(hostile);
    expect(strip?.textContent).toContain("<i>game</i> audio");
  });
});
