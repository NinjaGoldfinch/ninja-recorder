<!--
  The review player - WS4 task 4.5.

  **This is the imperative island, and it is deliberate.** A `<video>`'s
  `currentTime` is not state anything should be diffing: it changes sixty
  times a second while playing, the element is its own source of truth, and
  every seek is a command rather than an assignment. So this component holds a
  real element reference and talks to it directly, exactly as `review.ts` did.
  What it does *not* do is own the recording; that is `stores/review.svelte.ts`.

  The position is published into `playhead` from the rAF loop so the timeline
  can draw it. That is one number crossing the boundary in one direction, which
  is the smallest seam that still lets the timeline be declarative.
-->

<script lang="ts">
import { assetUrl, call } from "../../../bridge";
import { vodHeading } from "../../../format";
import { showView } from "../../../router";
import { toast } from "../../../toast";
import type { AudioLayout } from "../../../types";
import { laneOpponent } from "../../library/scoreboard";
import { type HotkeyContext, hotkeyAction, SEEK_STEP_S } from "../../review/hotkeys";
import { parseAudioLayout, videoErrorReport } from "../../review/playback";
import { closeRecording, review, setDuration } from "../../stores/review.svelte";
import type { MetricKey } from "../../timeline/graph";
import { nextMarker } from "../../timeline/navigate";
import { stemCorrection } from "../../timeline/stem";
import { clamp } from "../../timeline/window";
import MarkerList from "./MarkerList.svelte";
import PlayerControls from "./PlayerControls.svelte";
import Timeline from "./Timeline.svelte";

let video = $state<HTMLVideoElement>();
let playerWrap = $state<HTMLElement>();

/** The player's own state. None of it belongs in the store. */
let playhead = $state(0);
let paused = $state(true);
let rate = $state(1);
let fullscreen = $state(false);
let menuOpen = $state(false);
let videoError = $state<{ message: string; detail: string } | null>(null);

// **Volume and mute are the user's intent**, held here rather than read back
// off the element. Playing an isolated stem means muting the video and
// letting a separate `<audio>` carry the sound, so controls that read
// `video.muted` would render a muted player over audible audio.
let userVolume = $state(1);
let userMuted = $state(false);

let layout = $state<AudioLayout | null>(null);
let selectedTrack = $state(0);
let stem: HTMLAudioElement | null = null;

let raf: number | null = null;
let scrubbing: HTMLElement | null = null;
let startApplied = false;
/** Set when *we* paused because the window went away. */
let pausedByHide = false;

const heading = $derived(
  review.recording
    ? vodHeading(review.recording, laneOpponent(review.recording)?.champion ?? null)
    : "",
);

// --- The window ---------------------------------------------------------

/** Every seek goes through here, so nothing can land in the skipped lead
 *  or in the black tail past the end of the game. */
function seekTo(videoTimeS: number) {
  // Guarded on the window rather than on `video.duration`, which is the same
  // fact read from the other side: the window is empty until the duration is
  // known, and clamping into an empty one would seek to 0 rather than do
  // nothing. Everything else here reads `review.window`, so this does too.
  if (!video || review.window.span <= 0) return;
  video.currentTime = clamp(videoTimeS, review.window.start, review.window.end);
}

function seekBy(seconds: number) {
  if (video) seekTo(video.currentTime + seconds);
}

/** Seeks to the position `clientX` falls at along `track`. Takes the element
 *  so the in-player bar and the rich timeline can share it. */
function seekFromPointer(track: HTMLElement, clientX: number) {
  if (!video || !Number.isFinite(video.duration) || !video.duration) return;
  const rect = track.getBoundingClientRect();
  if (rect.width === 0) return;
  const fraction = clamp((clientX - rect.left) / rect.width, 0, 1);
  seekTo(review.window.start + fraction * review.window.span);
}

function startScrub(track: HTMLElement, e: PointerEvent) {
  if (!video) return;
  // Seek first, capture second: the seek is the part that must happen, and
  // pointer capture can throw. Losing the click to that would be worse.
  seekFromPointer(track, e.clientX);
  scrubbing = track;
  track.classList.add("open");
  try {
    track.setPointerCapture(e.pointerId);
  } catch {
    // Dragging still works; it just stops tracking outside the element.
  }
}

function moveScrub(track: HTMLElement, clientX: number) {
  if (scrubbing === track) seekFromPointer(track, clientX);
}

function endScrub(e: PointerEvent) {
  const track = scrubbing;
  if (!track) return;
  scrubbing = null;
  track.classList.remove("open");
  try {
    track.releasePointerCapture(e.pointerId);
  } catch {
    // Never captured; nothing to release.
  }
}

// --- The loop -----------------------------------------------------------

/**
 * Stops playback at the end of the window rather than letting it run into
 * the black tail (#119).
 *
 * Checked from the rAF loop *and* from `timeupdate`: the loop is smooth but
 * only runs while a frame is produced, and `timeupdate` keeps firing at
 * ~4 Hz regardless. Whichever gets there first wins.
 */
function stopAtWindowEnd() {
  if (!video || video.paused || !Number.isFinite(video.duration)) return;
  const end = review.window.end;
  if (end >= video.duration || video.currentTime < end) return;
  video.pause();
  // Clamped rather than left a few milliseconds past: "27:20 / 27:18" is
  // the kind of thing that looks like a bug.
  video.currentTime = end;
}

/** Keeps the stem aligned. Only runs while playing, so a paused player
 *  costs nothing. */
function correctStemDrift() {
  if (!stem || !video || video.paused || stem.seeking) return;
  const correction = stemCorrection(stem.currentTime - video.currentTime, video.playbackRate);
  if (correction.seek) stem.currentTime = video.currentTime;
  stem.playbackRate = correction.playbackRate;
}

function loop() {
  stopAtWindowEnd();
  if (video) playhead = video.currentTime;
  correctStemDrift();
  raf = requestAnimationFrame(loop);
}

function startLoop() {
  stopLoop();
  raf = requestAnimationFrame(loop);
}

function stopLoop() {
  if (raf !== null) cancelAnimationFrame(raf);
  raf = null;
}

// --- Audio --------------------------------------------------------------

/** Pushes the user's intent onto whichever element is producing sound. The
 *  video is muted whenever a stem plays, or the two would overlap. */
function applyAudioOutput() {
  if (video) {
    video.volume = userVolume;
    video.muted = userMuted || stem !== null;
  }
  if (stem) {
    stem.volume = userVolume;
    stem.muted = userMuted;
  }
}

function detachStem() {
  if (!stem) return;
  stem.pause();
  stem.removeAttribute("src");
  stem.load();
  stem = null;
  applyAudioOutput();
}

async function resumeStem() {
  if (!stem || !video) return;
  stem.currentTime = video.currentTime;
  stem.playbackRate = video.playbackRate;
  if (video.paused) return;
  try {
    await stem.play();
  } catch {
    // Autoplay rejection or a load race; the drift check retries next frame.
  }
}

/**
 * Switches which audio track is audible.
 *
 * Track 0 is the combined mix and plays straight off the video element.
 * Anything else has to be extracted first: WebView2 gives no way to select
 * among the audio tracks of one `<video>` (DEVELOPMENT.md §2.5).
 */
async function selectTrack(index: number) {
  const path = review.recording?.path;
  if (!video || !path) return;
  selectedTrack = index;

  if (index === 0) {
    detachStem();
    applyAudioOutput();
    return;
  }

  try {
    const stemPath = await call<string>("extract_audio_track", {
      recordingPath: path,
      trackIndex: index,
    });
    // The user can switch again while an extraction is in flight, and a
    // slow one must not stomp a newer choice.
    if (selectedTrack !== index) return;
    detachStem();
    const audio = new Audio(assetUrl(stemPath));
    audio.preload = "auto";
    audio.currentTime = video.currentTime;
    audio.playbackRate = video.playbackRate;
    stem = audio;
    applyAudioOutput();
    void resumeStem();
  } catch (err) {
    if (selectedTrack !== index) return;
    toast(`Couldn't load that audio track: ${err}`, "error");
    // Fall back to the mix rather than leaving the player silent with a
    // picker claiming otherwise.
    selectedTrack = 0;
    detachStem();
  }
}

// --- Controls -----------------------------------------------------------

function togglePlay() {
  if (!video || !video.src) return;
  if (!video.paused) {
    video.pause();
    return;
  }
  // Parked at the end of the window, play would hit `stopAtWindowEnd` next
  // frame and pause again: a button that visibly does nothing. Restart,
  // which is what reaching the end of any other video does.
  if (review.window.span > 0 && video.currentTime >= review.window.end - 0.05) {
    seekTo(review.window.start);
  }
  video.play().catch(() => {});
}

function toggleMute() {
  userMuted = !userMuted;
  applyAudioOutput();
}

function setVolume(v: number) {
  userVolume = v;
  userMuted = v === 0;
  applyAudioOutput();
}

function setRate(r: number) {
  rate = r;
  if (video) video.playbackRate = r;
  if (stem) stem.playbackRate = r;
}

function toggleFullscreen() {
  if (!playerWrap) return;
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  else playerWrap.requestFullscreen().catch(() => {});
}

function jump(direction: 1 | -1, predicate?: (m: { kind: string }) => boolean) {
  if (!video) return;
  const target = nextMarker(review.markers, video.currentTime, direction, predicate);
  if (target) seekTo(target.video_time_s);
}

// --- Lifecycle ----------------------------------------------------------

/**
 * Hands the recording's real aspect ratio to CSS as `--player-ratio`.
 *
 * `.player-wrap` caps the player by height and has to express that cap as a
 * *width*; capping the height instead letterboxes inside a full-width
 * element (DEVELOPMENT.md §5.1). Turning a height budget into a width needs
 * the ratio, and only the file knows it.
 */
function publishRatio() {
  if (!video || !playerWrap) return;
  const { videoWidth, videoHeight } = video;
  // Zero on an audio-only or still-loading file; a zero here would collapse
  // the player to the `max()` floor.
  if (!videoWidth || !videoHeight) return;
  playerWrap.style.setProperty("--player-ratio", String(videoWidth / videoHeight));
}

function onLoadedMetadata() {
  if (!video) return;
  setDuration(video.duration);
  publishRatio();
  // The first video position the player shows, applied once.
  if (!startApplied && review.window.start > 0) {
    startApplied = true;
    video.currentTime = review.window.start;
  }
}

function onVideoError() {
  if (!video) return;
  const err = video.error;
  videoError = videoErrorReport(
    err?.code ?? null,
    err?.message ?? null,
    video.currentSrc || video.src,
    review.recording?.path ?? null,
  );
}

function close() {
  stopLoop();
  menuOpen = false;
  detachStem();
  selectedTrack = 0;
  layout = null;
  videoError = null;
  playhead = 0;
  startApplied = false;
  if (video) {
    video.pause();
    video.removeAttribute("src");
    video.load();
  }
  closeRecording();
  showView("library");
}

/**
 * Loads whichever recording the store says is open.
 *
 * Keyed on the id rather than the object: the store replaces the row when
 * the markers land, and reloading the file then would restart playback
 * mid-session.
 */
$effect(() => {
  const row = review.recording;
  if (!row || !video) return;
  // Read so the effect re-runs when the recording changes and not when its
  // markers do.
  const id = row.id;
  void id;

  startApplied = false;
  videoError = null;
  detachStem();
  selectedTrack = 0;
  layout = parseAudioLayout(row.audio_tracks_json);
  video.src = assetUrl(row.path);
  video.playbackRate = rate;
  applyAudioOutput();
});

/**
 * Hotkeys, at the document level.
 *
 * **These are the only marker navigation available in fullscreen**, because
 * the rich timeline is outside `.player-wrap` and is not rendered there.
 */
$effect(() => {
  function onKey(e: KeyboardEvent) {
    const active = document.activeElement;
    const ctx: HotkeyContext = {
      reviewOpen: review.isOpen,
      typing: !!active && (active.tagName === "INPUT" || active.tagName === "TEXTAREA"),
      onFormControl: !!active && (active.tagName === "BUTTON" || active.tagName === "SELECT"),
      onArrowControl: active?.tagName === "SELECT",
      menuOpen,
    };
    const action = hotkeyAction(e.key, ctx);
    if (action === null) return;
    e.preventDefault();

    switch (action) {
      case "togglePlay":
        togglePlay();
        break;
      case "seekForward":
        seekBy(SEEK_STEP_S);
        break;
      case "seekBack":
        seekBy(-SEEK_STEP_S);
        break;
      case "toggleFullscreen":
        toggleFullscreen();
        break;
      case "toggleMute":
        toggleMute();
        break;
      case "nextMarker":
        jump(1);
        break;
      case "prevMarker":
        jump(-1);
        break;
      case "nextDeath":
        jump(1, (m) => m.kind === "death");
        break;
      case "prevDeath":
        jump(-1, (m) => m.kind === "death");
        break;
      case "closeMenu":
        menuOpen = false;
        break;
    }
  }

  document.addEventListener("keydown", onKey);
  return () => document.removeEventListener("keydown", onKey);
});

/**
 * Pauses an open, playing VOD while the window is hidden, and resumes it
 * after.
 *
 * A hidden window still decodes video and still plays the detached stem,
 * which is the single largest thing this app can burn while minimised to
 * the tray. Only a video the user had actually left playing is resumed.
 */
$effect(() => {
  function onVisibility() {
    if (!video || !review.isOpen) return;
    if (document.hidden) {
      if (!video.paused) {
        pausedByHide = true;
        video.pause();
      }
      return;
    }
    if (pausedByHide) {
      pausedByHide = false;
      void video.play().catch(() => {
        // Resuming is a courtesy; the user can press play.
      });
    }
  }

  function onFullscreenChange() {
    fullscreen = document.fullscreenElement === playerWrap;
    // Closed on the way in and out: the panel is positioned against the
    // control row, which moves.
    menuOpen = false;
  }

  function onPointerDown(e: PointerEvent) {
    if (!menuOpen) return;
    if (!(e.target as HTMLElement).closest(".player-menu")) menuOpen = false;
  }

  document.addEventListener("visibilitychange", onVisibility);
  document.addEventListener("fullscreenchange", onFullscreenChange);
  document.addEventListener("pointerdown", onPointerDown);
  return () => {
    document.removeEventListener("visibilitychange", onVisibility);
    document.removeEventListener("fullscreenchange", onFullscreenChange);
    document.removeEventListener("pointerdown", onPointerDown);
    stopLoop();
    detachStem();
  };
});
</script>

<svelte:window onpointerup={endScrub} onpointercancel={endScrub} />

<div class="review-header">
  <button type="button" class="back-btn" onclick={close}>&larr; Back</button>
  <h2>{heading}</h2>
</div>

<div class="player-wrap" bind:this={playerWrap}>
  <!-- svelte-ignore a11y_media_has_caption -->
  <video
    bind:this={video}
    onloadedmetadata={onLoadedMetadata}
    onerror={onVideoError}
    onplay={() => {
      paused = false;
      startLoop();
      void resumeStem();
    }}
    onpause={() => {
      paused = true;
      stopLoop();
      stem?.pause();
      // One last update, so the bar lands where the video actually stopped.
      if (video) playhead = video.currentTime;
    }}
    onseeked={() => {
      if (video) playhead = video.currentTime;
      void resumeStem();
    }}
    ontimeupdate={stopAtWindowEnd}
    onratechange={() => {
      if (video) rate = video.playbackRate;
    }}
  ></video>

  {#if videoError}
    <div class="video-error">
      <p>{videoError.message}</p>
      <code>{videoError.detail}</code>
    </div>
  {/if}

  <PlayerControls
    atS={Math.max(0, playhead - review.window.start)}
    totalS={review.window.span}
    {paused}
    muted={userMuted}
    volume={userVolume}
    {rate}
    {fullscreen}
    {layout}
    {selectedTrack}
    {menuOpen}
    ontoggleplay={togglePlay}
    ontogglemute={toggleMute}
    onvolume={setVolume}
    onrate={setRate}
    ontrack={(i) => void selectTrack(i)}
    ontogglefullscreen={toggleFullscreen}
    onmenu={(open) => (menuOpen = open)}
    onscrub={moveScrub}
    onscrubstart={startScrub}
  />
</div>

<Timeline
  markers={review.markers}
  samples={review.samples}
  metric={review.metric}
  window={review.window}
  currentTimeS={playhead}
  onmetric={(m: MetricKey) => (review.metric = m)}
  onseek={seekTo}
  onscrub={moveScrub}
  onscrubstart={startScrub}
/>

<MarkerList markers={review.markers} onseek={seekTo} />

<p class="hint">
  Space play/pause &middot; &larr; &rarr; seek 5s &middot; [ ] markers &middot; d / D deaths
  &middot; f fullscreen &middot; m mute
</p>
