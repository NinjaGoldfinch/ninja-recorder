/**
 * Everything the settings view reads that is not a preference - WS4 task 4.4.
 *
 * Preferences have their own module (`prefs.ts`) and their own cache, and this
 * does not duplicate them: `prefsState` below is a reactive mirror that the
 * controls bind to, and every write still goes through `savePref` so the
 * localStorage cache the boot script reads stays in step.
 *
 * What is held here is the half that comes from the backend and can change
 * without anyone touching a control: the registry's autostart entry, the audio
 * preset the recorder will actually use, the capture backend, the retention
 * policy, the recordings folder, and the version.
 */

import { call } from "../../bridge";
import { DEFAULT_PREFS, getPrefs, type NotifyPrefKey, type Prefs, savePref } from "../../prefs";
import type {
  AudioInputDevice,
  AudioPreset,
  AudioPresetKey,
  AutostartStatus,
  BackfillReport,
  EnforcementReport,
  RetentionPolicy,
} from "../../types";
import type { CaptureBackend, CaptureBackendStatus } from "../contract/types";
import { knownPreset, presetFor, usesMic } from "../settings/audio";
import { backfillSummary } from "../settings/backfill";
import {
  formToPolicy,
  limitsAnything,
  policyToForm,
  previewMessage,
  type RetentionForm,
  savedMessage,
} from "../settings/retention";
import { refreshDiskUsage, refreshLibrary } from "./library.svelte";
import { toast } from "./toast.svelte";

// --- Preferences, mirrored -------------------------------------------------

/**
 * A reactive copy of `getPrefs()`.
 *
 * `prefs.ts` resolves from SQLite a beat after first paint and holds the
 * answer in a plain object, which nothing can subscribe to. `syncFromPrefs`
 * copies it in once it lands, the same moment `settings.ts` used to re-apply
 * every control by hand.
 */
const prefsState = $state<Prefs>({ ...DEFAULT_PREFS });

export function syncFromPrefs() {
  Object.assign(prefsState, getPrefs());
}

/**
 * Writes a preference and mirrors it, in that order.
 *
 * Fire-and-forget by design for everything here: these decide what Rust does
 * next time it reads SQLite, and it re-reads on every use, so a slow write
 * cannot desync anything. The audio preset is the exception and does not go
 * through this.
 */
export function setPref<K extends keyof Prefs>(key: K, value: Prefs[K]) {
  prefsState[key] = value;
  savePref(key, value);
}

export function resetNotices() {
  // Blanked rather than deleted: `set_ui_pref` only writes, and Rust treats an
  // empty value as "not yet shown".
  setPref("notice.closeToTray.seen", "");
  toast("One-time notices will show again.");
}

/** The three per-event switches the master switch gates in Rust. */
export const NOTIFY_KEYS: readonly NotifyPrefKey[] = [
  "notifyRecordingStarted",
  "notifyRecordingFinished",
  "notifyRecordingFailed",
];

// --- Backend-owned state ---------------------------------------------------

let autostart = $state<AutostartStatus | null>(null);
let autostartError = $state<string | null>(null);
let autostartBusy = $state(false);

let audioPreset = $state<AudioPresetKey>("game");
let micDeviceId = $state("");
let micDevices = $state<AudioInputDevice[]>([]);

let captureBackend = $state<CaptureBackendStatus | null>(null);
let captureBackendError = $state<string | null>(null);
let captureBackendBusy = $state(false);

let retention = $state<RetentionForm>({
  sizeEnabled: false,
  sizeGb: "",
  ageEnabled: false,
  ageDays: "",
});
let retentionStatus = $state("");
let retentionPreview = $state<string | null>(null);
let retentionReport = $state<string | null>(null);

let recordingsDir = $state<string | null>(null);
let backfillReport = $state<string | null>(null);
let backfillBusy = $state(false);

export const settings = {
  get prefs() {
    return prefsState;
  },
  get autostart() {
    return autostart;
  },
  get autostartError() {
    return autostartError;
  },
  get autostartBusy() {
    return autostartBusy;
  },
  get audioPreset() {
    return audioPreset;
  },
  get micDeviceId() {
    return micDeviceId;
  },
  get micDevices() {
    return micDevices;
  },
  /** Whether the device picker should be usable for the current preset. */
  get micEnabled() {
    return usesMic(audioPreset);
  },
  get captureBackend() {
    return captureBackend;
  },
  get captureBackendError() {
    return captureBackendError;
  },
  get captureBackendBusy() {
    return captureBackendBusy;
  },
  get retention() {
    return retention;
  },
  get retentionStatus() {
    return retentionStatus;
  },
  get retentionPreview() {
    return retentionPreview;
  },
  get retentionReport() {
    return retentionReport;
  },
  get recordingsDir() {
    return recordingsDir;
  },
  get backfillReport() {
    return backfillReport;
  },
  get backfillBusy() {
    return backfillBusy;
  },
  get version() {
    return __APP_VERSION__;
  },
};

// --- Start on login --------------------------------------------------------

/**
 * The one setting that is not a preference: the registry entry is the source
 * of truth (Rust's `get_autostart` says why), so the checkbox is filled in
 * from the backend rather than from the cached `Prefs`.
 */
export async function loadAutostart(): Promise<void> {
  try {
    autostart = await call<AutostartStatus>("get_autostart");
    autostartError = null;
  } catch (err) {
    // A readable failure, not a silent one: the user is looking at a checkbox
    // whose state we could not determine, and leaving it unticked would claim
    // the app does not start on login.
    console.error("Failed to read the start-on-login setting", err);
    autostart = null;
    autostartError = `Couldn't read this setting: ${err}`;
  }
}

export async function setAutostart(enabled: boolean): Promise<void> {
  autostartBusy = true;
  try {
    // **The returned status is what the platform says afterwards**, which is
    // not necessarily what was asked for: a Run-key write can be overruled by
    // policy. Applying the response rather than the request keeps the checkbox
    // honest.
    autostart = await call<AutostartStatus>("set_autostart", { enabled });
    autostartError = null;
  } catch (err) {
    toast(`Couldn't change start on login: ${err}`, "error");
  } finally {
    autostartBusy = false;
  }
}

// --- Audio -----------------------------------------------------------------

export async function loadAudioSettings(): Promise<void> {
  try {
    // `?? []` is not defensive clutter: this feeds `micOptions`, which maps
    // over it, so anything other than an array here takes out the whole audio
    // panel rather than costing it one row.
    micDevices = (await call<AudioInputDevice[]>("list_audio_inputs")) ?? [];
  } catch (err) {
    console.error("Failed to list audio inputs", err);
  }

  try {
    const preset = await call<AudioPreset>("get_audio_preset");
    audioPreset = knownPreset(preset);
    if ("mic_device_id" in preset && preset.mic_device_id) {
      micDeviceId = preset.mic_device_id;
    }
  } catch (err) {
    console.error("Failed to load the audio preset", err);
    audioPreset = "game";
  }
}

/**
 * Saves the preset, and rolls back if the write fails.
 *
 * **Not fire-and-forget like the preferences above**: this decides what gets
 * recorded, so a failed write must not leave the UI claiming otherwise.
 */
export async function saveAudioPreset(key: AudioPresetKey, mic = micDeviceId): Promise<void> {
  const previousPreset = audioPreset;
  const previousMic = micDeviceId;
  audioPreset = key;
  micDeviceId = mic;

  try {
    await call("set_audio_preset", { preset: presetFor(key, mic) });
  } catch (err) {
    audioPreset = previousPreset;
    micDeviceId = previousMic;
    toast(`Couldn't save the audio setting: ${err}`, "error");
  }
}

// --- Capture backend -------------------------------------------------------

/**
 * Which backend the daemon records with, which ones this build can construct,
 * and what is live. Read from the daemon rather than from the prefs cache for
 * the reason autostart is: availability is the daemon's answer, not a stored
 * value, and a worker restored by a repair install changes it.
 */
export async function loadCaptureBackend(): Promise<void> {
  try {
    captureBackend = await call<CaptureBackendStatus>("get_capture_backend");
    captureBackendError = null;
  } catch (err) {
    // Readable rather than silent, and the control is disabled with it:
    // guessing "libobs" here would claim a choice nobody read back.
    console.error("Failed to read the capture backend", err);
    captureBackend = null;
    captureBackendError = `Couldn't read this setting: ${err}`;
  }
}

/**
 * Asks the daemon to switch, and shows what it reports afterwards.
 *
 * **Applies the response, not the request**, like `setAutostart`: the daemon
 * refuses mid-game and refuses a backend it cannot build, and a control that
 * moved before the answer would claim a switch that did not happen. Nothing
 * is written optimistically, so a refusal has nothing to roll back.
 */
export async function saveCaptureBackend(backend: CaptureBackend): Promise<void> {
  if (captureBackend?.configured === backend) return;
  captureBackendBusy = true;
  try {
    captureBackend = await call<CaptureBackendStatus>("set_capture_backend", { backend });
    captureBackendError = null;
  } catch (err) {
    toast(`Couldn't change the capture backend: ${err}`, "error");
  } finally {
    captureBackendBusy = false;
  }
}

// --- Storage ---------------------------------------------------------------

export async function loadRecordingsDir(): Promise<void> {
  try {
    recordingsDir = await call<string>("get_recordings_dir");
  } catch (err) {
    recordingsDir = `Unavailable: ${err}`;
  }
}

export async function openRecordingsFolder(): Promise<void> {
  try {
    await call("open_recordings_folder");
  } catch (err) {
    toast(`Couldn't open the folder: ${err}`, "error");
  }
}

/**
 * Reads the client's match history and labels whatever it can.
 *
 * Disabled while it runs: it is one request for the history plus one champion
 * lookup, but it walks every unlabelled row and a second click would ask the
 * same questions again to no effect.
 */
export async function runBackfill(): Promise<void> {
  backfillBusy = true;
  backfillReport = null;
  try {
    const report = await call<BackfillReport>("backfill_match_metadata");
    backfillReport = backfillSummary(report);
    // A recovered curve changes the review view rather than the row, but the
    // refresh is cheap and the alternative is a library that disagrees with
    // what the report just said.
    if (report.patched > 0 || report.gold_filled > 0) {
      await Promise.all([refreshLibrary(), refreshDiskUsage()]);
    }
  } catch (err) {
    toast(`Couldn't fill in match data: ${err}`, "error");
  } finally {
    backfillBusy = false;
  }
}

// --- Retention -------------------------------------------------------------

export async function loadRetentionPolicy(): Promise<void> {
  try {
    retention = policyToForm(await call<RetentionPolicy>("get_retention_policy"));
  } catch (err) {
    retentionStatus = `Failed to load policy: ${err}`;
  }
}

/**
 * What saving would delete, asked for as the user types.
 *
 * The form is the one place in the app where a careless edit destroys footage,
 * so it says what saving would do before you save it.
 */
export async function previewRetention(): Promise<void> {
  if (!limitsAnything(retention)) {
    retentionPreview = null;
    return;
  }
  try {
    const report = await call<EnforcementReport>("preview_retention_policy", {
      policy: formToPolicy(retention),
    });
    retentionPreview = previewMessage(report);
  } catch (err) {
    console.error("Retention preview failed", err);
    retentionPreview = null;
  }
}

export async function saveRetentionPolicy(): Promise<void> {
  try {
    retentionStatus = "Saving…";
    const report = await call<EnforcementReport>("set_retention_policy", {
      policy: formToPolicy(retention),
    });
    retentionStatus = "Saved.";
    retentionPreview = null;
    retentionReport = savedMessage(report);
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    retentionStatus = `Failed to save: ${err}`;
  }
}
