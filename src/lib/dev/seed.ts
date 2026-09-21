/**
 * Synthetic library generation: the spec the backend takes, and the presets.
 *
 * **The presets are the point.** Each one corresponds to something that is
 * otherwise untestable without playing a real game on Windows, and `what` says
 * which behaviour it exists to exercise. They are data, so they live here
 * rather than inside the panel that renders them.
 */

export interface SeedSpec {
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

export const BASE: SeedSpec = {
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

export interface Preset {
  id: string;
  label: string;
  what: string;
  spec: SeedSpec;
}

export const PRESETS: Preset[] = [
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
    what: "Mixed champions, queues and game modes, unicode and over-long names, rows with a mode but no queue id (the live-client-only case), plus rows with NULL metadata. Exercises filters, sort, the Queue fallback, and text escaping.",
    spec: { ...BASE, count: 24, messy: true, spread_days: 45, seed: 99 },
  },
];

/**
 * The form's version of a spec: a cleared number field is `null`, not `0`.
 *
 * `<input type="number">` two-way bindings resolve an empty box to `null`, and
 * the distinction is worth keeping rather than coercing at the keystroke: a
 * field that snapped to `0` the moment you cleared it could not be retyped.
 */
export type SeedForm = {
  [K in keyof SeedSpec]: SeedSpec[K] extends boolean ? boolean : number | null;
};

/** The keys whose fields are numbers, and the keys whose fields are
 *  checkboxes. A `bind:` target cannot carry a cast, so the key type has to
 *  narrow the lookup on its own. */
export type NumericKey = {
  [K in keyof SeedSpec]: SeedSpec[K] extends number ? K : never;
}[keyof SeedSpec];

export type BooleanKey = {
  [K in keyof SeedSpec]: SeedSpec[K] extends boolean ? K : never;
}[keyof SeedSpec];

export function toForm(spec: SeedSpec): SeedForm {
  return { ...spec };
}

/** The spec to send. A field left empty falls back to the preset it came
 *  from, which is the only other value the user can be said to have chosen. */
export function toSpec(form: SeedForm, fallback: SeedSpec): SeedSpec {
  const out = { ...fallback };
  for (const key of Object.keys(out) as (keyof SeedSpec)[]) {
    const value = form[key];
    if (typeof value === "boolean") {
      (out as Record<string, unknown>)[key] = value;
    } else if (typeof value === "number" && Number.isFinite(value)) {
      (out as Record<string, unknown>)[key] = value;
    }
  }
  return out;
}
