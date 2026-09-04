/**
 * Fixture capture and browsing.
 *
 * `fixtures.rs` has always written every LCU / Live Client Data response
 * to disk when capture is on, but nothing ever read them back —
 * DEVELOPMENT.md §3.3 asks for a replay mode and none existed. This makes
 * the captured files listable, viewable, editable, and hands them to the
 * Simulate panel.
 */
import type { Panel, PanelContext } from "../main";
import { call, tryCall } from "../ipc";
import { bytes, card, escapeHtml, output, panelHead, table, timestamp, toast } from "../ui";
import type { FixturesState } from "../types";

let root: HTMLElement | null = null;
let ctx: PanelContext | null = null;
let state: FixturesState | null = null;
let openPath: string | null = null;
let openContents = "";
let error: string | null = null;

function draw() {
  if (!root) return;

  root.innerHTML =
    panelHead(
      "Fixtures",
      "Captured API responses. Turn capture on, play a game, then replay what it recorded through the pipeline.",
    ) +
    card(
      "Capture",
      `<div class="row">
        <label class="check">
          <input type="checkbox" id="capture-toggle" ${state?.recording_enabled ? "checked" : ""} />
          Record every LCU and Live Client Data response
        </label>
        <span class="spacer"></span>
        <button type="button" class="ghost tiny" data-reveal>Reveal capture folder</button>
        <button type="button" class="ghost tiny" data-reload>Reload list</button>
      </div>
      ${kvPaths()}
      <p class="hint-block">Capture used to be settable only by launching with
        <code>NINJA_RECORDER_RECORD_FIXTURES</code> set, which meant deciding before the app
        started. Writes land under the capture folder as
        <code>&lt;group&gt;/&lt;endpoint&gt;.json</code>, one file per endpoint — a later response
        overwrites an earlier one.</p>`,
    ) +
    (error ? card("Error", output(error, true)) : "") +
    card(
      "Files",
      state && state.entries.length
        ? table(
            ["source", "group", "name", "size", "modified"],
            state.entries.map((f) => [
              f.source,
              f.group || "—",
              f.name,
              bytes(f.bytes),
              f.modified_millis ? timestamp(f.modified_millis) : null,
            ]),
            { clickable: true, emptyMessage: "No fixtures found." },
          )
        : `<p class="hint">No fixtures found. Turn capture on and run the app against a live
           client, or check in JSON under the repo's <code>fixtures/</code> directory.</p>`,
    ) +
    (openPath
      ? card(
          escapeHtml(openPath),
          `<textarea id="fixture-body" rows="16" spellcheck="false">${escapeHtml(
            openContents,
          )}</textarea>
          <div class="row" style="margin-top:.6rem">
            <button type="button" class="primary" data-send>Send to the snapshot injector</button>
            <button type="button" data-save>Save a copy…</button>
            <button type="button" class="ghost" data-close>Close</button>
          </div>
          <p class="hint-block">Saving writes into the capture folder, never over a repo fixture —
            checked-in fixtures are test inputs and should change through git, not through this
            page.</p>`,
          "",
          true,
        )
      : "");
}

function kvPaths(): string {
  if (!state) return "";
  return `<dl class="kv" style="margin-top:.7rem">
    <dt>Capture folder</dt><dd>${escapeHtml(state.capture_dir ?? "—")}</dd>
    <dt>Repo fixtures</dt><dd>${escapeHtml(state.repo_dir ?? "not resolvable in this build")}</dd>
  </dl>`;
}

async function reload() {
  const result = await tryCall<FixturesState>("dev_fixtures_state");
  if (result.ok) {
    state = result.value;
    error = null;
  } else {
    state = null;
    error = result.error;
  }
  draw();
}

export const fixturesPanel: Panel = {
  id: "fixtures",
  title: "Fixtures",
  icon: "❑",
  group: "Data",

  async mount(el, context) {
    root = el;
    ctx = context;
    openPath = null;
    openContents = "";
    await reload();

    el.addEventListener("change", async (e) => {
      const toggle = (e.target as HTMLElement).closest<HTMLInputElement>("#capture-toggle");
      if (!toggle) return;
      const result = await tryCall<boolean>("dev_set_fixture_recording", {
        enabled: toggle.checked,
      });
      if (result.ok) {
        toast(result.value ? "Fixture capture on" : "Fixture capture off", "ok");
        await reload();
      } else {
        toast(result.error, "err");
      }
    });

    el.addEventListener("click", async (e) => {
      const target = e.target as HTMLElement;

      if (target.closest("[data-reload]")) {
        await reload();
        return;
      }

      if (target.closest("[data-reveal]")) {
        const result = await tryCall("dev_open_data_dir", { which: "fixtures" });
        if (!result.ok) toast(result.error, "err");
        return;
      }

      const row = target.closest<HTMLElement>("tr[data-row-index]");
      if (row && state) {
        const entry = state.entries[Number(row.dataset.rowIndex)];
        const result = await tryCall<string>("dev_fixture_read", { path: entry.path });
        if (result.ok) {
          openPath = entry.path;
          openContents = result.value;
        } else {
          toast(result.error, "err");
        }
        draw();
        return;
      }

      if (target.closest("[data-close]")) {
        openPath = null;
        draw();
        return;
      }

      if (target.closest("[data-send]")) {
        const body = root!.querySelector<HTMLTextAreaElement>("#fixture-body")!.value;
        ctx?.navigate("simulate", body);
        return;
      }

      if (target.closest("[data-save]")) {
        const body = root!.querySelector<HTMLTextAreaElement>("#fixture-body")!.value;
        const name = prompt("Save as (name, no extension):");
        if (!name) return;
        const group = prompt("Group (folder):", "live-client");
        if (!group) return;
        try {
          const path = await call<string>("dev_fixture_write", { group, name, contents: body });
          toast(`Saved ${path}`, "ok");
          await reload();
        } catch (err) {
          toast(String(err), "err");
        }
      }
    });
  },

  unmount() {
    root = null;
    ctx = null;
  },
};
