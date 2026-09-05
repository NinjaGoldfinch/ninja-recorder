import { call } from "./bridge";
import { el } from "./dom";
import { BYTES_PER_GB, formatBytes } from "./format";
import { refreshDiskUsage, refreshLibrary } from "./library";
import { getPrefs, savePref, type SortKey, type ThemePref } from "./prefs";
import { showView } from "./router";
import { setThemePref } from "./theme";
import { toast } from "./toast";
import type { EnforcementReport, RetentionPolicy } from "./types";

interface Els {
  open: HTMLButtonElement;
  back: HTMLButtonElement;
  themeToggle: HTMLElement;
  defaultSort: HTMLSelectElement;
  form: HTMLFormElement;
  sizeEnabled: HTMLInputElement;
  sizeGb: HTMLInputElement;
  ageEnabled: HTMLInputElement;
  ageDays: HTMLInputElement;
  status: HTMLElement;
  preview: HTMLElement;
  report: HTMLElement;
  path: HTMLElement;
  openFolder: HTMLButtonElement;
  version: HTMLElement;
}

let els: Els;

export function initSettings() {
  els = {
    open: el<HTMLButtonElement>("#open-settings-btn"),
    back: el<HTMLButtonElement>("#back-to-library-from-settings-btn"),
    themeToggle: el("#theme-toggle"),
    defaultSort: el<HTMLSelectElement>("#default-sort-select"),
    form: el<HTMLFormElement>("#retention-form"),
    sizeEnabled: el<HTMLInputElement>("#retention-size-enabled"),
    sizeGb: el<HTMLInputElement>("#retention-size-gb"),
    ageEnabled: el<HTMLInputElement>("#retention-age-enabled"),
    ageDays: el<HTMLInputElement>("#retention-age-days"),
    status: el("#retention-status"),
    preview: el("#retention-preview"),
    report: el("#retention-report"),
    path: el("#recordings-path"),
    openFolder: el<HTMLButtonElement>("#open-folder-btn"),
    version: el("#about-version"),
  };

  els.open.addEventListener("click", () => showView("settings"));
  els.back.addEventListener("click", () => showView("library"));

  els.themeToggle.addEventListener("click", (e) => {
    const button = (e.target as HTMLElement).closest<HTMLElement>(
      "[data-theme-choice]",
    );
    if (!button) return;
    const choice = button.dataset.themeChoice as ThemePref;
    setThemePref(choice);
    syncThemeToggle(choice);
  });

  els.defaultSort.addEventListener("change", () => {
    savePref("defaultSort", els.defaultSort.value as SortKey);
  });

  els.sizeEnabled.addEventListener("change", () => {
    els.sizeGb.disabled = !els.sizeEnabled.checked;
    schedulePreview();
  });
  els.ageEnabled.addEventListener("change", () => {
    els.ageDays.disabled = !els.ageEnabled.checked;
    schedulePreview();
  });
  els.sizeGb.addEventListener("input", schedulePreview);
  els.ageDays.addEventListener("input", schedulePreview);
  els.form.addEventListener("submit", saveRetentionPolicy);

  els.openFolder.addEventListener("click", openFolder);

  void loadRetentionPolicy();
  void loadRecordingsDir();
  els.version.textContent = __APP_VERSION__;
}

// Called once prefs have resolved from the DB.
export function syncSettingsFromPrefs() {
  const prefs = getPrefs();
  syncThemeToggle(prefs.theme);
  els.defaultSort.value = prefs.defaultSort;
}

function syncThemeToggle(active: ThemePref) {
  for (const button of els.themeToggle.querySelectorAll<HTMLElement>(
    "[data-theme-choice]",
  )) {
    button.setAttribute(
      "aria-checked",
      String(button.dataset.themeChoice === active),
    );
  }
}

async function loadRecordingsDir() {
  try {
    els.path.textContent = await call<string>("get_recordings_dir");
  } catch (err) {
    els.path.textContent = `Unavailable: ${err}`;
  }
}

async function openFolder() {
  try {
    await call("open_recordings_folder");
  } catch (err) {
    toast(`Couldn't open the folder: ${err}`, "error");
  }
}

function readPolicy(): RetentionPolicy {
  return {
    max_total_bytes: els.sizeEnabled.checked
      ? Math.round(Number(els.sizeGb.value) * BYTES_PER_GB)
      : null,
    max_age_days: els.ageEnabled.checked ? Number(els.ageDays.value) : null,
  };
}

function applyPolicyToForm(policy: RetentionPolicy) {
  els.sizeEnabled.checked = policy.max_total_bytes !== null;
  els.sizeGb.disabled = policy.max_total_bytes === null;
  els.sizeGb.value =
    policy.max_total_bytes !== null
      ? String(Math.round(policy.max_total_bytes / BYTES_PER_GB))
      : "";

  els.ageEnabled.checked = policy.max_age_days !== null;
  els.ageDays.disabled = policy.max_age_days === null;
  els.ageDays.value =
    policy.max_age_days !== null ? String(policy.max_age_days) : "";
}

async function loadRetentionPolicy() {
  try {
    applyPolicyToForm(await call<RetentionPolicy>("get_retention_policy"));
  } catch (err) {
    els.status.textContent = `Failed to load policy: ${err}`;
  }
}

// The form is the one place in the app where a careless edit destroys
// footage, so it says what saving would delete before you save it.
let previewTimer: number | undefined;

function schedulePreview() {
  window.clearTimeout(previewTimer);
  previewTimer = window.setTimeout(runPreview, 350);
}

async function runPreview() {
  const policy = readPolicy();
  if (policy.max_total_bytes === null && policy.max_age_days === null) {
    els.preview.hidden = true;
    return;
  }
  try {
    const report = await call<EnforcementReport>("preview_retention_policy", {
      policy,
    });
    if (report.deleted.length === 0) {
      els.preview.hidden = true;
      return;
    }
    els.preview.hidden = false;
    els.preview.textContent =
      `Saving this will delete ${report.deleted.length} recording(s) ` +
      `and free ${formatBytes(report.freed_bytes)}.`;
  } catch (err) {
    console.error("Retention preview failed", err);
    els.preview.hidden = true;
  }
}

async function saveRetentionPolicy(e: Event) {
  e.preventDefault();
  const policy = readPolicy();
  try {
    els.status.textContent = "Saving…";
    const report = await call<EnforcementReport>("set_retention_policy", {
      policy,
    });
    els.status.textContent = "Saved.";
    els.preview.hidden = true;
    els.report.hidden = report.deleted.length === 0;
    els.report.textContent = `Deleted ${report.deleted.length} recording(s), freed ${formatBytes(
      report.freed_bytes,
    )}.`;
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    els.status.textContent = `Failed to save: ${err}`;
  }
}
