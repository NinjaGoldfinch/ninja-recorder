<!--
  The overlay: scrub bar, play, volume, clock, speed and track menu,
  fullscreen.

  **Inside `.player-wrap` on purpose.** That is the element passed to
  `requestFullscreen`, and anything outside the fullscreened subtree is simply
  not rendered. Controls that lived beside it vanished the moment you pressed
  `f`.
-->

<script lang="ts">
import { formatTime } from "../../../format";
import type { AudioLayout } from "../../../types";
import { hasStems } from "../../review/playback";

interface Props {
  /** Position and length within the *window*, not the file. */
  atS: number;
  totalS: number;
  paused: boolean;
  muted: boolean;
  volume: number;
  rate: number;
  fullscreen: boolean;
  layout: AudioLayout | null;
  selectedTrack: number;
  menuOpen: boolean;
  ontoggleplay: () => void;
  ontogglemute: () => void;
  onvolume: (v: number) => void;
  onrate: (r: number) => void;
  ontrack: (index: number) => void;
  ontogglefullscreen: () => void;
  onmenu: (open: boolean) => void;
  onscrub: (track: HTMLElement, clientX: number) => void;
  onscrubstart: (track: HTMLElement, e: PointerEvent) => void;
}

const {
  atS,
  totalS,
  paused,
  muted,
  volume,
  rate,
  fullscreen,
  layout,
  selectedTrack,
  menuOpen,
  ontoggleplay,
  ontogglemute,
  onvolume,
  onrate,
  ontrack,
  ontogglefullscreen,
  onmenu,
  onscrub,
  onscrubstart,
}: Props = $props();

const percent = $derived(totalS > 0 ? (atS / totalS) * 100 : 0);
const tracks = $derived(layout?.tracks ?? []);
</script>

<div class="player-overlay">
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="player-scrub"
    role="slider"
    tabindex="0"
    aria-label="Seek"
    aria-valuemin="0"
    aria-valuemax={Math.round(totalS)}
    aria-valuenow={Math.round(atS)}
    aria-valuetext="{formatTime(atS)} of {formatTime(totalS)}"
    onpointerdown={(e) => onscrubstart(e.currentTarget as HTMLElement, e)}
    onpointermove={(e) => onscrub(e.currentTarget as HTMLElement, e.clientX)}
  >
    <div class="player-progress" style="width:{percent.toFixed(3)}%"></div>
  </div>

  <div class="player-controls">
    <button
      type="button"
      class="icon-btn"
      title={paused ? "Play (Space)" : "Pause (Space)"}
      onclick={ontoggleplay}>{paused ? "▶" : "⏸"}</button
    >

    <div class="volume-control">
      <button
        type="button"
        class="icon-btn"
        title={muted ? "Unmute (m)" : "Mute (m)"}
        onclick={ontogglemute}>{muted || volume === 0 ? "🔇" : "🔊"}</button
      >
      <input
        class="volume-slider"
        type="range"
        min="0"
        max="1"
        step="0.05"
        aria-label="Volume"
        value={muted ? 0 : volume}
        oninput={(e) => onvolume(Number((e.currentTarget as HTMLInputElement).value))}
      />
    </div>

    <span class="time-display">{formatTime(atS)} / {formatTime(totalS)}</span>

    <div class="player-menu">
      <button
        type="button"
        class="icon-btn"
        title="Settings"
        aria-haspopup="true"
        aria-expanded={menuOpen}
        onclick={() => onmenu(!menuOpen)}>⚙</button
      >
      <div class="player-menu-panel" hidden={!menuOpen}>
        <label>
          Speed
          <select
            value={String(rate)}
            onchange={(e) => onrate(Number((e.currentTarget as HTMLSelectElement).value))}
          >
            <option value="0.25">0.25×</option>
            <option value="0.5">0.5×</option>
            <option value="1">1×</option>
            <option value="1.5">1.5×</option>
            <option value="2">2×</option>
          </select>
        </label>
        <!--
          Hidden unless there are at least two tracks: a single-track
          recording has nothing to choose between, and neither does one a
          rescan imported, whose layout we genuinely do not know.
        -->
        <label hidden={!hasStems(layout)}>
          Audio
          <select
            aria-label="Audio track"
            value={String(selectedTrack)}
            onchange={(e) => ontrack(Number((e.currentTarget as HTMLSelectElement).value))}
          >
            {#each tracks as track, i (i)}
              <option value={String(i)}>{track.label}</option>
            {/each}
          </select>
        </label>
      </div>
    </div>

    <!--
      The glyph stays ⛶ in both states: the icon set here is emoji, and there
      is no exit-fullscreen emoji with dependable coverage, so a
      missing-glyph box would be worse than a static icon. The state is
      carried by `aria-pressed`, which screen readers announce, and the title.
    -->
    <button
      type="button"
      class="icon-btn"
      title={fullscreen ? "Exit fullscreen (f)" : "Fullscreen (f)"}
      aria-pressed={fullscreen}
      onclick={ontogglefullscreen}>⛶</button
    >
  </div>
</div>
