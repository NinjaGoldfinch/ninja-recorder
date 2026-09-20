/**
 * Which audio preset records what.
 *
 * Moved out of `settings.ts` by WS4.4. The preview mirrors
 * `AudioPreset::layout()` in Rust and is **for the preview only**: the backend
 * decides what actually gets recorded.
 */

import type { AudioInputDevice, AudioPreset, AudioPresetKey } from "../../types";

/**
 * Which presets record a microphone.
 *
 * Data rather than an `if` chain, so the device picker's enabled state and the
 * track preview cannot disagree about it.
 */
export const PRESETS_WITH_MIC: readonly AudioPresetKey[] = ["game_mic", "game_mic_discord"];

/**
 * Track 0 is always the combined mix. "Game" alone has nothing to isolate, so
 * it stays one track.
 */
export const TRACK_LABELS: Record<AudioPresetKey, readonly string[]> = {
  game: ["Game"],
  game_mic: ["Everything", "Game", "Mic"],
  game_mic_discord: ["Everything", "Game", "Mic", "Discord"],
  desktop: ["System audio", "Game"],
};

export function usesMic(key: AudioPresetKey): boolean {
  return PRESETS_WITH_MIC.includes(key);
}

/** "Track 0: Everything · Track 1: Game · Track 2: Mic". */
export function trackPreview(key: AudioPresetKey): string {
  return TRACK_LABELS[key].map((label, i) => `Track ${i}: ${label}`).join(" · ");
}

/**
 * The preset key to show for whatever the backend returned.
 *
 * `custom` has no button yet, and an unknown value can reach us from a newer
 * build's settings row. Neither should leave the toggle showing nothing at
 * all, so both fall back to `game`.
 */
export function knownPreset(preset: AudioPreset): AudioPresetKey {
  const key = preset.preset as string;
  return key in TRACK_LABELS ? (key as AudioPresetKey) : "game";
}

/** The `AudioPreset` to send for a key and a chosen microphone. */
export function presetFor(key: AudioPresetKey, micDeviceId: string): AudioPreset {
  return usesMic(key) && micDeviceId
    ? ({ preset: key, mic_device_id: micDeviceId } as AudioPreset)
    : ({ preset: key } as AudioPreset);
}

/**
 * The microphone options, Windows default first.
 *
 * A device name comes from the driver, so it is untrusted text in the same way
 * a filename is. `settings.ts` built these as HTML with `escapeAttr` on the id
 * and `escapeHtml` on the label; a Svelte `{#each}` over this needs neither.
 */
export function micOptions(devices: readonly AudioInputDevice[]): { id: string; label: string }[] {
  return [
    { id: "", label: "Windows default" },
    ...devices.map((device) => ({
      id: device.id,
      label: device.name + (device.is_default ? " (current default)" : ""),
    })),
  ];
}
