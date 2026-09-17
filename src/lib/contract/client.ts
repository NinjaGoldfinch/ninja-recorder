// GENERATED FILE. Do not edit by hand.
//
// Regenerate with:  cargo run --features contract-gen --bin gen-contract   (from src-tauri/)
// CI runs the same binary with --check and fails if this file is stale.
//
// The declaration lives in Rust:
//   commands  src-tauri/src/core/dispatch.rs   (dispatch_table!)
//   events    src-tauri/src/contract/events.rs (contract_events!)
//   types     src-tauri/src/contract/types.rs  (the boundary list)

import type { AudioInputDevice, AudioPreset, AutostartStatus, BackfillReport, DiskUsage, EnforcementReport, IconRequest, IconSet, LcuStatus, MarkerRow, QuitOutcome, ReconcileReport, RecordingRow, RetentionPolicy, SampleRow, SupervisorStatus, UpdateStatus } from "./types";

/**
 * How a command reaches the backend. Supplied by the caller rather
 * than imported, so this file stays free of any transport: the Tauri
 * `invoke` today, a pipe frame under WS3, and a mock in tests.
 */
export type Invoke = (command: string, args: Record<string, unknown>) => Promise<unknown>;

/** Every command, as one object. The key is the wire name. */
export function createClient(invoke: Invoke) {
  return {
    start_recording: (): Promise<null> =>
      invoke("start_recording", {}) as Promise<null>,
    stop_recording: (): Promise<string> =>
      invoke("stop_recording", {}) as Promise<string>,
    is_recording: (): Promise<boolean> =>
      invoke("is_recording", {}) as Promise<boolean>,
    list_recordings: (): Promise<Array<RecordingRow>> =>
      invoke("list_recordings", {}) as Promise<Array<RecordingRow>>,
    rescan_recordings: (): Promise<ReconcileReport> =>
      invoke("rescan_recordings", {}) as Promise<ReconcileReport>,
    get_recording_markers: (recordingId: number): Promise<Array<MarkerRow>> =>
      invoke("get_recording_markers", { recordingId }) as Promise<Array<MarkerRow>>,
    get_recording_samples: (recordingId: number): Promise<Array<SampleRow>> =>
      invoke("get_recording_samples", { recordingId }) as Promise<Array<SampleRow>>,
    get_disk_usage: (): Promise<DiskUsage> =>
      invoke("get_disk_usage", {}) as Promise<DiskUsage>,
    get_retention_policy: (): Promise<RetentionPolicy> =>
      invoke("get_retention_policy", {}) as Promise<RetentionPolicy>,
    set_retention_policy: (policy: RetentionPolicy): Promise<EnforcementReport> =>
      invoke("set_retention_policy", { policy }) as Promise<EnforcementReport>,
    set_pinned: (recordingId: number, pinned: boolean): Promise<null> =>
      invoke("set_pinned", { recordingId, pinned }) as Promise<null>,
    preview_retention_policy: (policy: RetentionPolicy): Promise<EnforcementReport> =>
      invoke("preview_retention_policy", { policy }) as Promise<EnforcementReport>,
    delete_recording: (recordingId: number): Promise<null> =>
      invoke("delete_recording", { recordingId }) as Promise<null>,
    get_recordings_dir: (): Promise<string> =>
      invoke("get_recordings_dir", {}) as Promise<string>,
    get_ui_prefs: (): Promise<{ [key in string]: string }> =>
      invoke("get_ui_prefs", {}) as Promise<{ [key in string]: string }>,
    set_ui_pref: (key: string, value: string): Promise<null> =>
      invoke("set_ui_pref", { key, value }) as Promise<null>,
    get_autostart: (): Promise<AutostartStatus> =>
      invoke("get_autostart", {}) as Promise<AutostartStatus>,
    set_autostart: (enabled: boolean): Promise<AutostartStatus> =>
      invoke("set_autostart", { enabled }) as Promise<AutostartStatus>,
    get_audio_preset: (): Promise<AudioPreset> =>
      invoke("get_audio_preset", {}) as Promise<AudioPreset>,
    set_audio_preset: (preset: AudioPreset): Promise<null> =>
      invoke("set_audio_preset", { preset }) as Promise<null>,
    list_audio_inputs: (): Promise<Array<AudioInputDevice>> =>
      invoke("list_audio_inputs", {}) as Promise<Array<AudioInputDevice>>,
    extract_audio_track: (recordingPath: string, trackIndex: number): Promise<string> =>
      invoke("extract_audio_track", { recordingPath, trackIndex }) as Promise<string>,
    lcu_status: (): Promise<LcuStatus> =>
      invoke("lcu_status", {}) as Promise<LcuStatus>,
    backfill_match_metadata: (): Promise<BackfillReport> =>
      invoke("backfill_match_metadata", {}) as Promise<BackfillReport>,
    resolve_icons: (request: IconRequest): Promise<IconSet> =>
      invoke("resolve_icons", { request }) as Promise<IconSet>,
    game_state_status: (): Promise<SupervisorStatus> =>
      invoke("game_state_status", {}) as Promise<SupervisorStatus>,
    get_update_status: (): Promise<UpdateStatus> =>
      invoke("get_update_status", {}) as Promise<UpdateStatus>,
    check_for_update: (): Promise<null> =>
      invoke("check_for_update", {}) as Promise<null>,
    install_update: (): Promise<null> =>
      invoke("install_update", {}) as Promise<null>,
    quit_recorder: (force: boolean): Promise<QuitOutcome> =>
      invoke("quit_recorder", { force }) as Promise<QuitOutcome>,
  };
}
