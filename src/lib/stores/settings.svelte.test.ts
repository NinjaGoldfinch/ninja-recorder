import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The settings store's own rules.
 *
 * The wording lives in `lib/settings/` and is tested there. What is here is
 * the state handling that decides whether a control tells the truth: which
 * writes roll back, which answers are believed over the request that caused
 * them, and what a failed read leaves on screen.
 */

const call = vi.hoisted(() => vi.fn());
vi.mock("../../bridge", () => ({ call, hasDevCommands: vi.fn(), assetUrl: (p: string) => p }));
const toast = vi.hoisted(() => vi.fn());
vi.mock("./toast.svelte", () => ({ toast }));
const savePref = vi.hoisted(() => vi.fn());
const getPrefs = vi.hoisted(() => vi.fn());
vi.mock("../../prefs", async (original) => {
  const actual = await original<typeof import("../../prefs")>();
  return { ...actual, savePref, getPrefs };
});
const refreshLibrary = vi.hoisted(() => vi.fn());
const refreshDiskUsage = vi.hoisted(() => vi.fn());
vi.mock("./library.svelte", () => ({ refreshLibrary, refreshDiskUsage }));

let store: typeof import("./settings.svelte");

beforeEach(async () => {
  vi.resetModules();
  for (const fn of [call, toast, savePref, getPrefs, refreshLibrary, refreshDiskUsage]) {
    fn.mockReset();
  }
  store = await import("./settings.svelte");
});

describe("preferences", () => {
  it("mirrors what prefs.ts resolved, once it lands", () => {
    getPrefs.mockReturnValue({ theme: "dark", defaultSort: "oldest" });
    store.syncFromPrefs();
    expect(store.settings.prefs.theme).toBe("dark");
    expect(store.settings.prefs.defaultSort).toBe("oldest");
  });

  it("mirrors a write and persists it, in that order", () => {
    store.setPref("theme", "dark");
    expect(store.settings.prefs.theme).toBe("dark");
    expect(savePref).toHaveBeenCalledWith("theme", "dark");
  });

  it("blanks a one-time notice rather than deleting it", () => {
    // `set_ui_pref` only writes, and Rust reads an empty value as "not yet
    // shown". Deleting the key is not an option the backend offers.
    store.resetNotices();
    expect(savePref).toHaveBeenCalledWith("notice.closeToTray.seen", "");
    expect(toast).toHaveBeenCalled();
  });

  it("gates exactly the three per-event switches", () => {
    expect(store.NOTIFY_KEYS).toEqual([
      "notifyRecordingStarted",
      "notifyRecordingFinished",
      "notifyRecordingFailed",
    ]);
  });
});

describe("start on login", () => {
  it("believes the platform's answer over the request", async () => {
    // A Run-key write can be overruled by policy, so applying the response
    // rather than the request is what keeps the checkbox honest.
    call.mockResolvedValue({ enabled: false, managed: true });
    await store.setAutostart(true);
    expect(store.settings.autostart).toEqual({ enabled: false, managed: true });
  });

  it("says it could not read the setting rather than showing it as off", async () => {
    call.mockRejectedValue(new Error("registry locked"));
    await store.loadAutostart();
    expect(store.settings.autostart).toBeNull();
    expect(store.settings.autostartError).toContain("registry locked");
  });

  it("clears a previous error once a read succeeds", async () => {
    call.mockRejectedValueOnce(new Error("locked"));
    await store.loadAutostart();
    call.mockResolvedValue({ enabled: true });
    await store.loadAutostart();
    expect(store.settings.autostartError).toBeNull();
  });

  it("marks itself busy only while the write is in flight", async () => {
    let release: (v: unknown) => void = () => {};
    call.mockReturnValue(new Promise((r) => (release = r)));
    const pending = store.setAutostart(true);
    expect(store.settings.autostartBusy).toBe(true);
    release({ enabled: true });
    await pending;
    expect(store.settings.autostartBusy).toBe(false);
  });

  it("stays unbusy after a failed write, and says so out loud", async () => {
    call.mockRejectedValue(new Error("denied"));
    await store.setAutostart(true);
    expect(store.settings.autostartBusy).toBe(false);
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("start on login"), "error");
  });
});

describe("audio", () => {
  it("rolls the preset back when the write fails", async () => {
    // Not fire-and-forget like the preferences: this decides what gets
    // recorded, so a failed write must not leave the UI claiming otherwise.
    call.mockResolvedValue(null);
    await store.saveAudioPreset("game_mic", "mic-1");
    expect(store.settings.audioPreset).toBe("game_mic");

    call.mockRejectedValue(new Error("device gone"));
    await store.saveAudioPreset("game");
    expect(store.settings.audioPreset).toBe("game_mic");
    expect(store.settings.micDeviceId).toBe("mic-1");
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("audio setting"), "error");
  });

  it("enables the device picker only for a preset that records a mic", async () => {
    call.mockResolvedValue(null);
    await store.saveAudioPreset("game");
    expect(store.settings.micEnabled).toBe(false);
    await store.saveAudioPreset("game_mic", "mic-1");
    expect(store.settings.micEnabled).toBe(true);
  });

  it("survives a device list that is not a list", async () => {
    // It feeds a `.map`, so anything else takes out the whole audio panel
    // rather than costing it one row.
    call.mockImplementation(async (command: string) =>
      command === "list_audio_inputs" ? null : { kind: "game" },
    );
    await store.loadAudioSettings();
    expect(store.settings.micDevices).toEqual([]);
  });

  it("falls back to game audio when the preset cannot be read", async () => {
    call.mockImplementation(async (command: string) => {
      if (command === "list_audio_inputs") return [];
      throw new Error("no daemon");
    });
    await store.loadAudioSettings();
    expect(store.settings.audioPreset).toBe("game");
  });
});

describe("storage", () => {
  it("shows why the folder is unknown in the field itself", async () => {
    call.mockRejectedValue(new Error("not connected"));
    await store.loadRecordingsDir();
    expect(store.settings.recordingsDir).toContain("not connected");
  });

  it("reports a folder that will not open", async () => {
    call.mockRejectedValue(new Error("no shell"));
    await store.openRecordingsFolder();
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("open the folder"), "error");
  });
});

describe("backfill", () => {
  it("re-reads the library only when something was actually written", async () => {
    call.mockResolvedValue({ scanned: 4, patched: 0, gold_filled: 0, skipped: 4 });
    await store.runBackfill();
    expect(refreshLibrary).not.toHaveBeenCalled();

    call.mockResolvedValue({ scanned: 4, patched: 2, gold_filled: 0, skipped: 2 });
    await store.runBackfill();
    expect(refreshLibrary).toHaveBeenCalled();
    expect(refreshDiskUsage).toHaveBeenCalled();
  });

  it("re-reads for a recovered curve too, which changes no row", async () => {
    call.mockResolvedValue({ scanned: 1, patched: 0, gold_filled: 1, skipped: 0 });
    await store.runBackfill();
    expect(refreshLibrary).toHaveBeenCalled();
  });

  it("clears the previous report before running, so two runs never blur", async () => {
    call.mockResolvedValue({ scanned: 1, patched: 1, gold_filled: 0, skipped: 0 });
    await store.runBackfill();
    expect(store.settings.backfillReport).not.toBeNull();

    call.mockRejectedValue(new Error("client closed"));
    await store.runBackfill();
    expect(store.settings.backfillReport).toBeNull();
    expect(store.settings.backfillBusy).toBe(false);
    expect(toast).toHaveBeenCalledWith(expect.stringContaining("fill in match data"), "error");
  });
});

describe("retention", () => {
  it("does not ask for a preview of a policy that limits nothing", async () => {
    await store.previewRetention();
    expect(call).not.toHaveBeenCalled();
    expect(store.settings.retentionPreview).toBeNull();
  });

  it("says what saving would delete, as the form is edited", async () => {
    store.settings.retention.sizeEnabled = true;
    store.settings.retention.sizeGb = "50";
    call.mockResolvedValue({ deleted: [1, 2], freed_bytes: 2048, scanned: 9 });
    await store.previewRetention();
    expect(call).toHaveBeenCalledWith("preview_retention_policy", expect.anything());
    expect(store.settings.retentionPreview).toBeTruthy();
  });

  it("shows nothing rather than a stale preview when the preview fails", async () => {
    store.settings.retention.ageEnabled = true;
    store.settings.retention.ageDays = "30";
    call.mockResolvedValue({ deleted: [1], freed_bytes: 1, scanned: 1 });
    await store.previewRetention();
    expect(store.settings.retentionPreview).toBeTruthy();

    call.mockRejectedValue(new Error("busy"));
    await store.previewRetention();
    expect(store.settings.retentionPreview).toBeNull();
  });

  it("drops the preview on save, since it is now a description of the past", async () => {
    store.settings.retention.sizeEnabled = true;
    store.settings.retention.sizeGb = "10";
    call.mockResolvedValue({ deleted: [1], freed_bytes: 1024, scanned: 3 });
    await store.previewRetention();
    await store.saveRetentionPolicy();

    expect(store.settings.retentionPreview).toBeNull();
    expect(store.settings.retentionStatus).toBe("Saved.");
    expect(store.settings.retentionReport).toBeTruthy();
    expect(refreshLibrary).toHaveBeenCalled();
  });

  it("leaves the failure in the status line, where the form is", async () => {
    call.mockRejectedValue(new Error("read-only db"));
    await store.saveRetentionPolicy();
    expect(store.settings.retentionStatus).toContain("read-only db");
  });

  it("puts a failed load in the same place", async () => {
    call.mockRejectedValue(new Error("no daemon"));
    await store.loadRetentionPolicy();
    expect(store.settings.retentionStatus).toContain("no daemon");
  });
});

describe("the capture backend", () => {
  const status = (software_encoding: boolean) => ({
    configured: "own",
    active: "own (idle)",
    software_encoding,
    options: [
      { backend: "libobs", unavailable: null },
      { backend: "own", unavailable: null },
    ],
  });

  // Called on every game-state edge, which starts before the view has read the
  // status at all; the view's own load decides when the first read happens.
  it("does not refresh before the view has read it", async () => {
    await store.refreshCaptureBackend();
    expect(call).not.toHaveBeenCalled();
  });

  it("refreshes once read, so the software notice can appear", async () => {
    call.mockResolvedValueOnce(status(false)).mockResolvedValueOnce(status(true));
    await store.loadCaptureBackend();
    expect(store.settings.captureBackend?.software_encoding).toBe(false);
    await store.refreshCaptureBackend();
    expect(call).toHaveBeenLastCalledWith("get_capture_backend");
    expect(store.settings.captureBackend?.software_encoding).toBe(true);
  });
});
