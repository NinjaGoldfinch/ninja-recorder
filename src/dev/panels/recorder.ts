/** Capture backend: identity, preflight, and a manual start/stop. */
import type { Panel, PanelContext } from "../main";
import { tryCall } from "../ipc";
import { bytes, card, kv, output, panelHead, toast } from "../ui";
import type { DevHealth } from "../types";

let root: HTMLElement | null = null;
let ctx: PanelContext | null = null;
let health: DevHealth | null = null;
let lastResult: { text: string; error: boolean } | null = null;

function draw() {
  if (!root || !ctx) return;
  const env = ctx.env;
  const recording = health?.is_recording ?? false;
  const lowSpace = (health?.free_bytes ?? Infinity) < 1024 ** 3;

  root.innerHTML =
    panelHead(
      "Recorder",
      "The capture backend, and a manual start/stop that bypasses the state machine.",
    ) +
    `<div class="warnbar">
      This starts the recorder <em>directly</em>. The supervisor doesn't know about it, so if the
      state machine also decides to record, the two disagree about whether capture is running —
      the known divergence documented on <code>Supervisor::start_recording</code>. Use the
      Simulate panel to exercise the real path.
    </div>` +
    card(
      "Backend",
      kv([
        ["Active", env?.recorder_backend ?? "?"],
        ["Platform", env ? `${env.os}/${env.arch}` : "?"],
        ["Capturing", recording ? "yes" : "no"],
        ["Output directory", env?.recordings_dir],
        ["Free space", bytes(health?.free_bytes)],
        [
          "Preflight",
          lowSpace
            ? "would REFUSE — under the 1 GiB minimum"
            : "would allow — at least 1 GiB free",
        ],
      ]) +
        (env?.recorder_backend === "stub"
          ? `<p class="hint-block">The stub simulates encoder latency and then produces a file:
             a copy of <code>fixtures/sample.mp4</code> when that exists, otherwise a 31-byte
             placeholder that will not decode. ${
               env.sample_mp4_present
                 ? "It is present, so stub recordings are playable."
                 : "It is not checked in, so stub recordings will not play."
             }</p>`
          : ""),
    ) +
    card(
      "Manual capture",
      `<div class="row">
        <button type="button" class="primary" data-start ${recording ? "disabled" : ""}>Start recording</button>
        <button type="button" class="danger" data-stop ${recording ? "" : "disabled"}>Stop recording</button>
        <button type="button" class="ghost" data-is-recording>Query is_recording</button>
      </div>` + (lastResult ? output(lastResult.text, lastResult.error) : ""),
    );
}

async function run(command: string, describe: (value: unknown) => string) {
  const result = await tryCall<unknown>(command);
  lastResult = result.ok
    ? { text: describe(result.value), error: false }
    : { text: result.error, error: true };
  if (!result.ok) toast(result.error, "err");
  draw();
}

export const recorderPanel: Panel = {
  id: "recorder",
  title: "Recorder",
  icon: "⏺",
  group: "Status",

  async mount(el, context) {
    root = el;
    ctx = context;
    lastResult = null;
    const h = await tryCall<DevHealth>("dev_health");
    health = h.ok ? h.value : null;
    draw();

    el.addEventListener("click", async (e) => {
      const target = e.target as HTMLElement;
      if (target.closest("[data-start]")) {
        await run("start_recording", () => "Recording started.");
      } else if (target.closest("[data-stop]")) {
        await run("stop_recording", (path) => `Saved: ${path}`);
      } else if (target.closest("[data-is-recording]")) {
        await run("is_recording", (v) => `is_recording → ${v}`);
      }
    });
  },

  onHealth(next) {
    // Redrawing the whole panel each tick would fight the button states,
    // so only react when capture actually flips.
    if (health?.is_recording !== next.is_recording || health?.free_bytes !== next.free_bytes) {
      health = next;
      draw();
    } else {
      health = next;
    }
  },

  unmount() {
    root = null;
    ctx = null;
  },
};
