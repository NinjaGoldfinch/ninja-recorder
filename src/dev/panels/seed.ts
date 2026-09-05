/**
 * Synthetic library generation.
 *
 * The presets are the point: each one corresponds to something that is
 * otherwise untestable without playing a real game on Windows, and the
 * descriptions say which behaviour it exists to exercise.
 */
import type { Panel, PanelContext } from "../main";
import { call, tryCall } from "../ipc";
import { bytes, card, confirmDialog, escapeHtml, output, panelHead, toast } from "../ui";
import type { DevEnvInfo, SeedReport } from "../types";

interface SeedSpec {
  count: number;
  duration_min_s: number;
  duration_max_s: number;
  markers_min: number;
  markers_max: number;
  samples: boolean;
  file_bytes: number;
  use_sample_mp4: boolean;
  spread_days: number;
  pinned_every: number;
  messy: boolean;
  seed: number;
}

const BASE: SeedSpec = {
  count: 10,
  duration_min_s: 900,
  duration_max_s: 2400,
  markers_min: 8,
  markers_max: 30,
  samples: true,
  file_bytes: 64 * 1024,
  use_sample_mp4: false,
  spread_days: 14,
  pinned_every: 0,
  messy: false,
  seed: 1,
};

interface Preset {
  id: string;
  label: string;
  what: string;
  spec: SeedSpec;
}

const PRESETS: Preset[] = [
  {
    id: "small",
    label: "Small library",
    what: "Ten ordinary recordings with markers and an advantage curve. The default starting point for library, filter, and sort work.",
    spec: { ...BASE },
  },
  {
    id: "review",
    label: "Review-ready",
    what: "One recording that copies fixtures/sample.mp4, with dense clustered markers and a full curve — the only preset that produces a VOD the review player can actually play.",
    spec: {
      ...BASE,
      count: 1,
      spread_days: 0,
      markers_min: 45,
      markers_max: 60,
      duration_min_s: 1500,
      duration_max_s: 1500,
      use_sample_mp4: true,
      seed: 7,
    },
  },
  {
    id: "retention-size",
    label: "Retention: size",
    what: "Twenty 3 GiB recordings, every fourth one pinned. Proves pinned rows are exempt from deletion while still counting toward the total. Files are sparse, so this costs almost no real disk.",
    spec: {
      ...BASE,
      count: 20,
      file_bytes: 3 * 1024 ** 3,
      pinned_every: 4,
      spread_days: 7,
      samples: false,
      markers_min: 2,
      markers_max: 6,
      seed: 12,
    },
  },
  {
    id: "retention-age",
    label: "Retention: age",
    what: "Fifteen recordings spread across 90 days, so a max-age policy has something to bite on without waiting.",
    spec: { ...BASE, count: 15, spread_days: 90, samples: false, seed: 21 },
  },
  {
    id: "messy",
    label: "Filter torture",
    what: "Mixed champions, queues, unicode and over-long names, plus rows with NULL metadata — the state every row is actually in today, since fetch_match_summary is never called. Exercises filters, sort, and HTML escaping.",
    spec: { ...BASE, count: 24, messy: true, spread_days: 45, seed: 99 },
  },
];

let root: HTMLElement | null = null;
let ctx: PanelContext | null = null;
let spec: SeedSpec = { ...BASE };
let activePreset = "small";
let report: { value: unknown; error: boolean } | null = null;
let busy = false;

function numberField(key: keyof SeedSpec, label: string, help = ""): string {
  return `<label class="field"><span>${escapeHtml(label)}</span>
    <input type="number" data-spec="${key}" value="${spec[key]}" />
    ${help ? `<span class="hint">${escapeHtml(help)}</span>` : ""}
  </label>`;
}
// (help is already inside the label here — see commands.ts's argField for
// why that placement matters inside a .field-grid)

function checkField(key: keyof SeedSpec, label: string): string {
  return `<label class="check">
    <input type="checkbox" data-spec="${key}" ${spec[key] ? "checked" : ""} /> ${escapeHtml(label)}
  </label>`;
}

function draw() {
  if (!root) return;
  const env: DevEnvInfo | null = ctx?.env ?? null;

  root.innerHTML =
    panelHead(
      "Seed",
      "Generate a realistic library from nothing — real files, real rows, real markers and samples.",
    ) +
    (env && !env.sample_mp4_present
      ? `<div class="warnbar"><code>fixtures/sample.mp4</code> is not present, so seeded files are
         sparse placeholders that no demuxer will open. Everything except video playback works;
         drop a short real clip there for the Review-ready preset to be worth running.</div>`
      : "") +
    card(
      "Presets",
      `<div class="row">${PRESETS.map(
        (p) =>
          `<button type="button" class="${p.id === activePreset ? "primary" : ""}" data-preset="${p.id}">
            ${escapeHtml(p.label)}
          </button>`,
      ).join("")}</div>
      <p class="hint-block">${escapeHtml(
        PRESETS.find((p) => p.id === activePreset)?.what ?? "",
      )}</p>`,
    ) +
    card(
      "Specification",
      `<div class="field-grid">
        ${numberField("count", "Recordings")}
        ${numberField("seed", "Random seed", "Same seed, same library")}
        ${numberField("spread_days", "Spread over days", "Back-dates started_at")}
        ${numberField("pinned_every", "Pin every Nth", "0 pins nothing")}
        ${numberField("duration_min_s", "Min duration (s)")}
        ${numberField("duration_max_s", "Max duration (s)")}
        ${numberField("markers_min", "Min markers")}
        ${numberField("markers_max", "Max markers")}
        ${numberField("file_bytes", "File size (bytes)", "Sparse — costs no real disk")}
      </div>
      <div class="row" style="margin-top:.8rem">
        ${checkField("samples", "Generate the 1 Hz advantage curve")}
        ${checkField("use_sample_mp4", "Copy fixtures/sample.mp4 when present")}
        ${checkField("messy", "Mix in NULL metadata, unicode, and long names")}
      </div>
      <div class="row" style="margin-top:1rem">
        <button type="button" class="primary" data-seed ${busy ? "disabled" : ""}>
          ${busy ? "Seeding…" : `Seed ${spec.count} recording(s)`}
        </button>
        <span class="hint">writes ${bytes(spec.count * spec.file_bytes)} (sparse)</span>
      </div>
      ${report ? output(report.value, report.error) : ""}`,
    ) +
    card(
      "Cleanup",
      `<p class="hint-block" style="margin-top:0">Seeded recordings are named
        <code>seed-*.mp4</code>. Clearing removes only those rows and files — anything captured
        for real is left alone.</p>
      <div class="row" style="margin-top:.7rem">
        <button type="button" class="danger" data-clear>Clear seeded recordings</button>
      </div>`,
      "card-danger",
    );
}

/** Reads the form back into `spec`. The cast is the price of driving a
 *  typed struct from `data-*` attributes; the keys come from the same
 *  `keyof SeedSpec` the fields were rendered from. */
function collect() {
  const target = spec as unknown as Record<string, number | boolean>;
  root!.querySelectorAll<HTMLInputElement>("[data-spec]").forEach((input) => {
    const key = input.dataset.spec as keyof SeedSpec;
    if (input.type === "checkbox") {
      target[key] = input.checked;
    } else {
      const n = Number(input.value);
      if (!Number.isNaN(n)) target[key] = n;
    }
  });
}

export const seedPanel: Panel = {
  id: "seed",
  title: "Seed",
  icon: "✦",
  group: "Data",

  mount(el, context) {
    root = el;
    ctx = context;
    report = null;
    draw();

    el.addEventListener("input", () => collect());

    el.addEventListener("click", async (e) => {
      const target = e.target as HTMLElement;

      const preset = target.closest<HTMLElement>("[data-preset]");
      if (preset) {
        const found = PRESETS.find((p) => p.id === preset.dataset.preset);
        if (found) {
          activePreset = found.id;
          spec = { ...found.spec };
          report = null;
          draw();
        }
        return;
      }

      if (target.closest("[data-seed]")) {
        collect();
        busy = true;
        draw();
        const result = await tryCall<SeedReport>("dev_seed_library", { spec });
        busy = false;
        if (result.ok) {
          const r = result.value;
          report = {
            value: {
              recordings: r.recording_ids.length,
              ids: r.recording_ids,
              markers: r.markers_inserted,
              samples: r.samples_inserted,
              bytes_on_disk: bytes(r.bytes_written),
              playable: r.used_sample_mp4,
              first_path: r.paths[0] ?? null,
            },
            error: false,
          };
          toast(`Seeded ${r.recording_ids.length} recording(s)`, "ok");
        } else {
          report = { value: result.error, error: true };
          toast(result.error, "err");
        }
        draw();
        return;
      }

      if (target.closest("[data-clear]")) {
        const ok = await confirmDialog({
          title: "Clear seeded recordings?",
          body: "Every <code>seed-*.mp4</code> file and its database row will be deleted. Captured recordings are not touched.",
          confirmLabel: "Clear seeded",
        });
        if (!ok) return;
        try {
          const r = await call<{ rows_deleted: number; files_deleted: number }>("dev_clear_seeded");
          toast(`Removed ${r.rows_deleted} row(s), ${r.files_deleted} file(s)`, "ok");
          report = null;
          draw();
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
