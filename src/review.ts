import { assetUrl, call } from "./bridge";
import { escapeHtml } from "./dom";
import { formatTime, vodTitle } from "./format";
import { currentView, showView } from "./router";
import { toast } from "./toast";
import type { AudioLayout, MarkerRow, RecordingRow, SampleRow } from "./types";

export type { RecordingRow };

const MARKER_STYLE: Record<string, { icon: string; label: string; color: string }> = {
  kill: { icon: "⚔️", label: "Kill", color: "#43a047" },
  death: { icon: "💀", label: "Death", color: "#e53935" },
  assist: { icon: "🤝", label: "Assist", color: "#1e88e5" },
  dragon: { icon: "🐉", label: "Dragon", color: "#8e24aa" },
  baron: { icon: "👑", label: "Baron", color: "#6d4c41" },
  herald: { icon: "🦅", label: "Herald", color: "#00897b" },
  turret: { icon: "🏰", label: "Turret", color: "#fb8c00" },
  inhibitor: { icon: "💠", label: "Inhibitor", color: "#5e35b1" },
  ace: { icon: "⭐", label: "Ace", color: "#fdd835" },
  multikill: { icon: "🔥", label: "Multikill", color: "#f4511e" },
  first_blood: { icon: "🩸", label: "First Blood", color: "#d81b60" },
};

// When several markers collapse into one timeline glyph, the cluster shows
// a single icon — this is which one wins. Ordered by how much the event
// changes what you're looking for in a VOD: your own deaths and kills first,
// then objectives by value, with assists last because they're the most
// numerous and the least individually interesting.
const MARKER_PRIORITY = [
  "multikill",
  "death",
  "kill",
  "baron",
  "dragon",
  "herald",
  "inhibitor",
  "ace",
  "first_blood",
  "turret",
  "assist",
];

// Volume and mute are the *user's* intent, held here rather than read back
// off the video element. Playing an isolated stem means muting the video and
// letting a separate <audio> carry the sound, and if the controls read
// `video.muted` they would then render a muted player over audible audio.
let userVolume = 1;
let userMuted = false;

// The <audio> carrying the selected stem, or null while track 0 (the
// combined mix) plays from the video element itself.
let stemAudio: HTMLAudioElement | null = null;
let selectedTrack = 0;

// Drift thresholds for keeping the stem aligned to the video.
// 0.04s is ~2.4 frames at 60fps — below the point A/V desync is noticeable —
// and a rate nudge beyond ~2% is audible as a pitch shift, so anything
// worse than SYNC_HARD is re-seeked instead of nudged.
const SYNC_NUDGE = 0.04;
const SYNC_HARD = 0.25;

let backBtn: HTMLButtonElement | null;
let reviewTitle: HTMLElement | null;
let video: HTMLVideoElement | null;
let videoError: HTMLElement | null;
let videoErrorText: HTMLElement | null;
let videoErrorDetail: HTMLElement | null;
let rateSelect: HTMLSelectElement | null;
let markerListEl: HTMLElement | null;
let playerWrap: HTMLElement | null;
let playerScrub: HTMLElement | null;
let playerProgress: HTMLElement | null;
let playPauseBtn: HTMLButtonElement | null;
let timeDisplay: HTMLElement | null;
let muteBtn: HTMLButtonElement | null;
let volumeControl: HTMLElement | null;
let volumeSlider: HTMLInputElement | null;
let settingsBtn: HTMLButtonElement | null;
let settingsMenu: HTMLElement | null;
let trackField: HTMLElement | null;
let trackSelect: HTMLSelectElement | null;
let fullscreenBtn: HTMLButtonElement | null;
let timelineBody: HTMLElement | null;
let timelineGraph: SVGSVGElement | null;
let timelineGlyphs: HTMLElement | null;
let timelineRuler: HTMLElement | null;
let timelinePlayhead: HTMLElement | null;
let timelineTooltip: HTMLElement | null;
let metricSelect: HTMLSelectElement | null;
let metricSummary: HTMLElement | null;

let currentMarkers: MarkerRow[] = [];
let currentSamples: SampleRow[] = [];
let currentRecordingPath: string | null = null;
/// Survives across recordings on purpose — someone comparing games
/// shouldn't have to re-pick the metric every time they open a VOD.
let currentMetric: MetricKey = "gold_diff";
/// Glyph clusters from the last `renderGlyphs`, indexed by `data-cluster`
/// so hover can list a cluster's members without re-deriving them.
let currentClusters: MarkerRow[][] = [];
let rafHandle: number | null = null;
/// Which seek track is currently being dragged, or null. There are two of
/// them — the in-player scrub bar and the rich `#vod-timeline` — and a drag
/// on one must not be ended by a stray pointer event on the other.
let scrubbingTrack: HTMLElement | null = null;
/// Whether the settings menu was open when the pointer went down on the
/// video. A click that only dismissed the menu must not also toggle
/// playback, and by the time `click` fires the menu is already closed.
let menuWasOpenOnPointerDown = false;

export function initReview() {
  document.addEventListener("visibilitychange", onVisibilityChange);
  backBtn = document.querySelector("#back-to-library-btn");
  reviewTitle = document.querySelector("#review-title");
  video = document.querySelector("#review-video");
  videoError = document.querySelector("#review-video-error");
  videoErrorText = document.querySelector("#review-video-error-text");
  videoErrorDetail = document.querySelector("#review-video-error-detail");
  rateSelect = document.querySelector("#playback-rate-select");
  markerListEl = document.querySelector("#marker-list");
  playerWrap = document.querySelector(".player-wrap");
  playerScrub = document.querySelector("#player-scrub");
  playerProgress = document.querySelector("#player-progress");
  playPauseBtn = document.querySelector("#play-pause-btn");
  timeDisplay = document.querySelector("#time-display");
  muteBtn = document.querySelector("#mute-btn");
  volumeControl = document.querySelector("#volume-control");
  volumeSlider = document.querySelector("#volume-slider");
  settingsBtn = document.querySelector("#player-settings-btn");
  settingsMenu = document.querySelector("#player-settings-menu");
  fullscreenBtn = document.querySelector("#fullscreen-btn");
  timelineBody = document.querySelector("#timeline-body");
  timelineGraph = document.querySelector("#timeline-graph");
  timelineGlyphs = document.querySelector("#timeline-glyphs");
  timelineRuler = document.querySelector("#timeline-ruler");
  timelinePlayhead = document.querySelector("#timeline-playhead");
  timelineTooltip = document.querySelector("#timeline-tooltip");
  trackField = document.querySelector("#audio-track-field");
  trackSelect = document.querySelector("#audio-track-select");
  metricSelect = document.querySelector("#timeline-metric-select");
  metricSummary = document.querySelector("#timeline-metric-summary");

  backBtn?.addEventListener("click", closeReview);
  rateSelect?.addEventListener("change", () => {
    if (video && rateSelect) video.playbackRate = Number(rateSelect.value);
  });
  video?.addEventListener("loadedmetadata", renderTimeline);
  video?.addEventListener("error", showVideoError);

  // Playback state -> chrome. The playhead runs off rAF rather than
  // `timeupdate` (which fires ~4Hz and looks visibly steppy), but paused
  // seeks don't produce animation frames, so `seeked` updates directly.
  video?.addEventListener("play", () => {
    syncPlayButton();
    startPlayheadLoop();
    void resumeStem();
  });
  video?.addEventListener("pause", () => {
    syncPlayButton();
    stopPlayheadLoop();
    updatePlayhead();
    stemAudio?.pause();
  });
  video?.addEventListener("ended", () => {
    stopPlayheadLoop();
    stemAudio?.pause();
  });
  video?.addEventListener("seeked", () => {
    updatePlayhead();
    void resumeStem();
  });
  // The stem is a slave clock: pause it while the video is between frames
  // rather than letting it run on and then snap back.
  video?.addEventListener("seeking", () => stemAudio?.pause());
  video?.addEventListener("waiting", () => stemAudio?.pause());
  video?.addEventListener("stalled", () => stemAudio?.pause());
  video?.addEventListener("playing", () => void resumeStem());
  // Hooked on the event rather than in the rate <select>'s handler, so any
  // other path that changes the rate is covered too.
  video?.addEventListener("ratechange", () => {
    if (stemAudio) stemAudio.playbackRate = video!.playbackRate;
  });
  video?.addEventListener("loadedmetadata", () => {
    applyStartPosition();
    updatePlayhead();
  });

  playPauseBtn?.addEventListener("click", togglePlay);
  muteBtn?.addEventListener("click", toggleMute);
  fullscreenBtn?.addEventListener("click", toggleFullscreen);
  volumeSlider?.addEventListener("input", () => {
    if (!volumeSlider) return;
    userVolume = Number(volumeSlider.value);
    userMuted = userVolume === 0;
    applyAudioOutput();
    syncVolumeControls();
  });

  // The volume slider is revealed by hovering its group, but a drag routinely
  // wanders outside it — which would collapse the slider mid-drag. Pin it
  // open until the pointer is released anywhere.
  volumeSlider?.addEventListener("pointerdown", () => {
    volumeControl?.classList.add("open");
  });
  const releaseVolume = () => volumeControl?.classList.remove("open");
  document.addEventListener("pointerup", releaseVolume);
  document.addEventListener("pointercancel", releaseVolume);

  // Click the picture to play/pause, as every other video player does.
  // Bound to the <video> itself, so clicks on the overlay bar (a sibling
  // painted above it) are control interactions and never reach here.
  video?.addEventListener("pointerdown", () => {
    menuWasOpenOnPointerDown = isSettingsMenuOpen();
  });
  video?.addEventListener("click", () => {
    if (menuWasOpenOnPointerDown) return;
    togglePlay();
  });

  settingsBtn?.addEventListener("click", () => setSettingsMenu(!isSettingsMenuOpen()));
  // Outside-click dismissal, using the same delegation idiom as
  // `library.ts`'s card actions.
  document.addEventListener("pointerdown", (e) => {
    if (!isSettingsMenuOpen()) return;
    if ((e.target as HTMLElement).closest("#player-settings-menu, #player-settings-btn")) {
      return;
    }
    setSettingsMenu(false);
  });

  // Entering or leaving fullscreen — including via Escape or the OS, which
  // never go through `toggleFullscreen` — has to be reflected on the button,
  // or it sits there advertising the wrong state.
  document.addEventListener("fullscreenchange", () => {
    syncFullscreenButton();
    // The panel is positioned against a control bar that has just moved and
    // resized; reopening it is cheaper than reasoning about that.
    setSettingsMenu(false);
  });

  trackSelect?.addEventListener("change", () => {
    void selectTrack(Number(trackSelect!.value));
  });

  metricSelect?.addEventListener("change", () => {
    currentMetric = (metricSelect!.value as MetricKey) ?? "gold_diff";
    renderGraph();
  });

  // Seeking, on both tracks. On the rich timeline a click on a glyph keeps
  // its existing precise-jump behaviour; anywhere else on either track seeks
  // to that position, and holding scrubs.
  if (playerScrub) bindScrubbing(playerScrub);
  if (timelineBody) {
    bindScrubbing(timelineBody, (e) => {
      const target = (e.target as HTMLElement).closest<HTMLElement>("[data-time]");
      if (!target || !video) return false;
      // Suppressing the compatibility mouse events also suppresses the focus
      // they would have moved onto the glyph. A glyph is a seek target, not
      // somewhere to leave the caret: focused, it eats the next Space as
      // "press me again" instead of play/pause. Keyboard activation is
      // untouched — this only fires for a pointer.
      e.preventDefault();
      seekTo(Number(target.dataset.time));
      return true;
    });
  }

  timelineGlyphs?.addEventListener("mouseover", showClusterTooltip);
  timelineGlyphs?.addEventListener("mouseout", hideClusterTooltip);

  // Clustering is measured in pixels, so it has to be redone whenever the
  // track's width changes. The graph and ruler are laid out in percentages
  // and viewBox units, so neither needs this.
  if (timelineBody && typeof ResizeObserver !== "undefined") {
    let pending = 0;
    new ResizeObserver(() => {
      if (pending) return;
      pending = requestAnimationFrame(() => {
        pending = 0;
        renderGlyphs();
      });
    }).observe(timelineBody);
  }

  markerListEl?.addEventListener("click", (e) => {
    const target = (e.target as HTMLElement).closest<HTMLElement>("li[data-time]");
    if (target && video) seekTo(Number(target.dataset.time));
  });

  document.addEventListener("keydown", handleHotkey);
}

const MEDIA_ERROR_LABELS: Record<number, string> = {
  1: "Aborted",
  2: "Network error",
  3: "Decode error — the container loaded but the codec inside it isn't supported",
  4: "Source not supported — wrong format, or the file couldn't be reached at all",
};

/**
 * Surfaces the actual `MediaError` rather than a canned guess — a
 * "no playable video" message that's always about the stub placeholder
 * (DEVELOPMENT.md's dev-only fixture path) was actively misleading once
 * real files started getting reviewed: it told people to drop a clip in
 * as `fixtures/sample.mp4`, a path that only exists when running from
 * source, not in an installed build.
 */
function showVideoError() {
  if (!videoError || !video) return;
  videoError.hidden = false;

  const err = video.error;
  const isMkv = currentRecordingPath?.toLowerCase().endsWith(".mkv") ?? false;
  // Codes 3 (decode) and 4 (source not supported) on an mp4 that isn't
  // actually malformed are, in practice, almost always an unsupported
  // codec inside an otherwise-valid container.
  const likelyCodecIssue = !isMkv && (err?.code === 3 || err?.code === 4);

  if (videoErrorText) {
    if (isMkv) {
      // High-confidence special case: WebView2's <video> element has no
      // Matroska demuxer at all, so an .mkv fails here regardless of how
      // valid its contents are — most likely to bite anyone testing with
      // an OBS recording, since .mkv is OBS's crash-safe default output.
      videoErrorText.textContent =
        "This is an .mkv file — browsers (including WebView2) can't play Matroska containers natively, no matter what's encoded inside. Remux it to .mp4 (e.g. \"ffmpeg -i in.mkv -c copy out.mp4\", no re-encode needed) and try again.";
    } else if (likelyCodecIssue) {
      // H.265/HEVC-in-mp4 is the single most common real-world cause of
      // this: many capture tools (ShadowPlay, some phones) default to it,
      // and WebView2 can't decode it without an extra Windows codec pack
      // that's not installed by default.
      videoErrorText.textContent =
        `This recording's video couldn't be played (${MEDIA_ERROR_LABELS[err!.code]}). The most common cause for an otherwise-valid mp4 is H.265/HEVC video — WebView2 needs the "HEVC Video Extensions" from the Microsoft Store to decode it at all, and playback can still be unreliable even then. Re-encoding to H.264 is the more reliable fix: "ffmpeg -i in.mp4 -c:v libx264 -c:a aac out.mp4".`;
    } else {
      videoErrorText.textContent = err
        ? `This recording's video couldn't be played (${MEDIA_ERROR_LABELS[err.code] ?? `error code ${err.code}`}).`
        : "This recording's video couldn't be played.";
    }
  }
  if (videoErrorDetail) {
    const parts: string[] = [];
    if (err?.message) parts.push(err.message);
    parts.push(`src: ${video.currentSrc || video.src}`);
    videoErrorDetail.textContent = parts.join(" — ");
  }
}

function isReviewOpen(): boolean {
  return currentView() === "review";
}

function isTypingInField(): boolean {
  const el = document.activeElement;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA");
}

/** Controls that do something of their own with Space. */
function isOnFormControl(): boolean {
  const el = document.activeElement;
  return !!el && (el.tagName === "BUTTON" || el.tagName === "SELECT");
}

/** Controls that do something of their own with the arrow keys. */
function isOnArrowControl(): boolean {
  return document.activeElement?.tagName === "SELECT";
}

function seekBy(seconds: number) {
  if (!video) return;
  seekTo(video.currentTime + seconds);
}

/**
 * Hotkeys: Space play/pause, arrows seek 5s, `[`/`]` prev/next marker,
 * `d`/`D` prev/next death, `f` fullscreen, `m` mute, Escape closes the
 * settings menu.
 *
 * These are the only marker navigation available in fullscreen: the rich
 * `#vod-timeline` lives outside `.player-wrap` and so isn't rendered there.
 */
function handleHotkey(e: KeyboardEvent) {
  if (!isReviewOpen() || isTypingInField()) return;

  // Space is the browser's own way to press a focused button or open a
  // focused select, so stealing it would break every control in the row the
  // moment one had focus.
  if (e.key === " " && isOnFormControl()) return;

  // Arrows are not: a <button> ignores them entirely, so waving them off for
  // any focused button bought nothing and cost everything — clicking a
  // timeline glyph, which *is* a button, left it focused and killed seeking
  // until you happened to click somewhere else. A <select> does use them,
  // and a focused <input> is already out by `isTypingInField` (which is what
  // keeps the volume slider's own arrow handling).
  if (e.key.startsWith("Arrow") && isOnArrowControl()) return;

  if (e.key === " ") {
    e.preventDefault();
    togglePlay();
  } else if (e.key === "ArrowRight") {
    e.preventDefault();
    seekBy(5);
  } else if (e.key === "ArrowLeft") {
    e.preventDefault();
    seekBy(-5);
  } else if (e.key === "f") {
    e.preventDefault();
    toggleFullscreen();
  } else if (e.key === "m") {
    e.preventDefault();
    toggleMute();
  } else if (e.key === "]") {
    e.preventDefault();
    jumpToMarker(1, () => true);
  } else if (e.key === "[") {
    e.preventDefault();
    jumpToMarker(-1, () => true);
  } else if (e.key === "d") {
    e.preventDefault();
    jumpToMarker(-1, (m) => m.kind === "death");
  } else if (e.key === "D") {
    e.preventDefault();
    jumpToMarker(1, (m) => m.kind === "death");
  } else if (e.key === "Escape" && isSettingsMenuOpen()) {
    // Guarded on the menu being open so this never shadows the user agent's
    // own Escape-exits-fullscreen.
    e.preventDefault();
    setSettingsMenu(false);
    settingsBtn?.focus();
  }
}

function jumpToMarker(direction: 1 | -1, predicate: (m: MarkerRow) => boolean) {
  if (!video) return;
  const candidates = currentMarkers.filter(predicate);
  if (candidates.length === 0) return;

  // Small deadband around currentTime so "next" from exactly on a marker
  // advances instead of re-selecting the same one.
  const current = video.currentTime;
  let target: MarkerRow | undefined;
  if (direction === 1) {
    target = candidates.find((m) => m.video_time_s > current + 0.25) ?? candidates[0];
  } else {
    target =
      [...candidates].reverse().find((m) => m.video_time_s < current - 0.25) ??
      candidates[candidates.length - 1];
  }
  seekTo(target.video_time_s);
}

// --- The playable window --------------------------------------------------

/**
 * How much of the loading screen to keep in front of the game.
 *
 * Not zero: cutting to the exact frame the clock starts on means a VOD
 * opens mid-fade with no sense of where it began, and the alignment is
 * measured from a 1 Hz poll so it is only accurate to about a second
 * anyway. Two is enough to see the game appear without waiting for it.
 */
const LEAD_IN_S = 2;

/** Below this there is no loading screen worth skipping. */
const MIN_SKIP_S = 3;

/**
 * Where the game clock starts, in video time.
 *
 * A recording begins when the client says the game is in progress, which is
 * the loading screen — twenty seconds of a static splash before anything
 * happens. Markers already know this: every one is stamped with the
 * game-time → video-time alignment measured during the game, so the
 * difference between a sample's two clocks *is* the length of the loading
 * screen. It is read back out of the samples the timeline already fetches
 * rather than stored, which means it works on every recording ever made,
 * with no migration and nothing to keep in sync.
 *
 * Zero when there are no samples — a game whose live poller never came up
 * has no alignment, and guessing one would open the VOD somewhere arbitrary.
 */
let gameStartsAt = 0;

function measureGameStart(samples: readonly SampleRow[]): number {
  const earliest = samples.reduce<SampleRow | null>(
    (best, s) => (best === null || s.game_time_s < best.game_time_s ? s : best),
    null,
  );
  if (!earliest) return 0;
  const offset = earliest.video_time_s - earliest.game_time_s;
  // Negative means capture started *after* the game did — a reconnect —
  // and there is no loading screen in front of it to skip.
  return offset >= MIN_SKIP_S ? offset : 0;
}

/** The first video position the player will show. */
function windowStart(): number {
  return Math.max(0, gameStartsAt - LEAD_IN_S);
}

/** How much video the player treats as the recording. */
function windowSpan(): number {
  if (!video || !isFinite(video.duration)) return 0;
  return Math.max(0, video.duration - windowStart());
}

/** Video time → the position shown to the user, where 0 is the window's start. */
function displayTime(videoTime: number): number {
  return Math.max(0, videoTime - windowStart());
}

/** Fraction across the window, for anything drawn along the timeline. */
function windowFraction(videoTime: number): number {
  const span = windowSpan();
  if (span <= 0) return 0;
  return clamp((videoTime - windowStart()) / span, 0, 1);
}

/** Every seek goes through here, so nothing can land in the skipped lead. */
function seekTo(videoTime: number) {
  if (!video || !isFinite(video.duration)) return;
  video.currentTime = clamp(videoTime, windowStart(), video.duration);
}

/** Set once per recording, when both the samples and the duration are in. */
let startApplied = false;

function applyStartPosition() {
  if (!video || !isFinite(video.duration) || !video.duration) return;
  if (startApplied) return;
  startApplied = true;
  if (windowStart() > 0) video.currentTime = windowStart();
}

// --- Timeline rendering ---------------------------------------------------

type MetricKey = "gold_diff" | "kill_diff" | "cs_diff" | "none";

const METRIC_META: Record<
  Exclude<MetricKey, "none">,
  { label: string; empty: string; format: (v: number) => string }
> = {
  gold_diff: {
    label: "Gold diff",
    // Its own message, because its absence has its own cause. Gold comes
    // from the post-game match timeline, so a custom or a practice game
    // never has one, and a real game does not have one until the deferred
    // patch lands. Falling back to the generic "not enough data" would
    // read as a bug in all three cases.
    empty: "No gold data for this recording",
    format: formatSignedGold,
  },
  kill_diff: { label: "Kill diff", empty: "Not enough data to plot", format: formatSigned },
  cs_diff: { label: "CS diff", empty: "Not enough data to plot", format: formatSigned },
};

/** Re-renders every part of the timeline. Cheap enough to call wholesale. */
function renderTimeline() {
  renderGraph();
  renderGlyphs();
  renderRuler();
  updatePlayhead();
}

function metricValue(sample: SampleRow, metric: MetricKey): number | null {
  switch (metric) {
    case "gold_diff":
      return sample.gold_diff;
    case "kill_diff":
      return sample.kill_diff;
    case "cs_diff":
      return sample.cs_diff;
    default:
      return null;
  }
}

function setMetricSummary(text: string, muted: boolean, title = "") {
  if (!metricSummary) return;
  metricSummary.textContent = text;
  metricSummary.classList.toggle("hint", muted);
  if (title) metricSummary.title = title;
  else metricSummary.removeAttribute("title");
}

function hideGraph() {
  if (timelineGraph) {
    timelineGraph.innerHTML = "";
    timelineGraph.style.display = "none";
  }
}

/**
 * Draws the signed advantage curve.
 *
 * Two things here differ deliberately from a normal sparkline. The vertical
 * scale is symmetric about zero (`+bound` and `-bound` map to the top and
 * bottom edges) rather than min-to-max, because a min-to-max scale floats
 * the zero crossing — a game spent entirely behind would render as a line
 * through the middle and read as "even". And the area is filled between the
 * curve and the zero line rather than down to the bottom edge, split into
 * ahead/behind halves, since that split is what actually communicates the
 * swing at a glance.
 */
function renderGraph() {
  if (!timelineGraph || !video || !isFinite(video.duration) || !video.duration) return;

  if (currentSamples.length === 0) {
    hideGraph();
    if (metricSelect) metricSelect.hidden = true;
    setMetricSummary("No metric data for this recording", true);
    return;
  }

  // A recording where we never matched ourselves in `allPlayers` has samples
  // but no side, so every diff's sign is unknowable. Showing the curve anyway
  // would risk telling someone they were ahead in a game they lost.
  const side = currentSamples.find((s) => s.our_team)?.our_team ?? null;
  if (!side) {
    hideGraph();
    if (metricSelect) {
      metricSelect.hidden = false;
      metricSelect.disabled = true;
    }
    setMetricSummary("Team side unknown — diff unavailable", true);
    return;
  }

  if (metricSelect) {
    metricSelect.hidden = false;
    metricSelect.disabled = false;
    metricSelect.value = currentMetric;
  }

  if (currentMetric === "none") {
    hideGraph();
    setMetricSummary("", true);
    return;
  }

  const meta = METRIC_META[currentMetric];
  const points = currentSamples
    .map((s) => ({ t: s.video_time_s, v: metricValue(s, currentMetric) }))
    .filter((p): p is { t: number; v: number } => p.v !== null);

  if (points.length < 2) {
    hideGraph();
    setMetricSummary(meta.empty, true);
    return;
  }

  const reduced = downsample(points, 500);
  const bound = Math.max(...reduced.map((p) => Math.abs(p.v)));
  const x = (t: number) => windowFraction(t) * 1000;
  // Symmetric about the y=50 baseline, 5 units of headroom each side.
  const y = (v: number) => (bound === 0 ? 50 : 50 - (v / bound) * 45);

  const line = reduced
    .map((p, i) => `${i === 0 ? "M" : "L"}${x(p.t).toFixed(2)} ${y(p.v).toFixed(2)}`)
    .join(" ");
  const area = `M${x(reduced[0].t).toFixed(2)} 50 ${line.slice(1)} L${x(
    reduced[reduced.length - 1].t
  ).toFixed(2)} 50 Z`;

  timelineGraph.style.display = "";
  timelineGraph.innerHTML = `
    <defs>
      <clipPath id="tl-clip-ahead"><rect x="0" y="0" width="1000" height="50" /></clipPath>
      <clipPath id="tl-clip-behind"><rect x="0" y="50" width="1000" height="50" /></clipPath>
    </defs>
    <path class="tl-area tl-area-ahead" d="${area}" clip-path="url(#tl-clip-ahead)" />
    <path class="tl-area tl-area-behind" d="${area}" clip-path="url(#tl-clip-behind)" />
    <line class="tl-baseline" x1="0" y1="50" x2="1000" y2="50" vector-effect="non-scaling-stroke" />
    <path class="tl-line" d="${line}" vector-effect="non-scaling-stroke" />`;

  const last = reduced[reduced.length - 1].v;
  const peak = reduced.reduce((a, p) => (Math.abs(p.v) > Math.abs(a) ? p.v : a), 0);
  setMetricSummary(
    `${meta.label} · ${meta.format(last)} at end · peak ${meta.format(peak)}`,
    false
  );
}

/**
 * Buckets `points` down to at most `target` entries, keeping the largest
 * magnitude in each bucket. Max-*abs* rather than max: on a signed series the
 * interesting value in a bucket is the biggest swing either way, and plain
 * max would quietly drop every trough.
 */
function downsample<T extends { v: number }>(points: T[], target: number): T[] {
  if (points.length <= target) return points;
  const size = points.length / target;
  const out: T[] = [];
  for (let i = 0; i < target; i++) {
    const slice = points.slice(Math.floor(i * size), Math.floor((i + 1) * size));
    if (slice.length === 0) continue;
    out.push(slice.reduce((a, b) => (Math.abs(b.v) > Math.abs(a.v) ? b : a)));
  }
  return out;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function formatSigned(value: number): string {
  return `${value > 0 ? "+" : ""}${Math.round(value)}`;
}

function formatSignedGold(value: number): string {
  const sign = value > 0 ? "+" : value < 0 ? "-" : "";
  const abs = Math.abs(value);
  return abs >= 1000 ? `${sign}${(abs / 1000).toFixed(1)}k` : `${sign}${Math.round(abs)}`;
}

// Roughly a glyph's width plus a gap: markers landing closer together than
// this on screen get collapsed into one badge-counted cluster. Measured in
// pixels rather than the percentage the old two-lane strip used, because a
// percentage threshold means something completely different on a 600px-wide
// window than on a 1600px one.
const CLUSTER_PX = 28;

function renderGlyphs() {
  if (!timelineGlyphs || !timelineBody || !video) return;
  if (!isFinite(video.duration) || !video.duration) return;

  const width = timelineBody.getBoundingClientRect().width;
  // Zero while the review view is still hidden — the ResizeObserver fires
  // again with a real width once it's shown.
  if (width === 0) return;
  currentClusters = [];
  let clusterStartX = -Infinity;
  for (const marker of currentMarkers) {
    const px = windowFraction(marker.video_time_s) * width;
    if (currentClusters.length > 0 && px - clusterStartX <= CLUSTER_PX) {
      currentClusters[currentClusters.length - 1].push(marker);
    } else {
      currentClusters.push([marker]);
      clusterStartX = px;
    }
  }

  timelineGlyphs.innerHTML = currentClusters
    .map((cluster, index) => {
      const lead = leadMarker(cluster);
      const style = markerStyle(lead);
      const mean =
        cluster.reduce((sum, m) => sum + m.video_time_s, 0) / cluster.length;
      const pct = windowFraction(mean) * 100;
      const badge =
        cluster.length > 1
          ? `<span class="glyph-badge">${cluster.length}</span>`
          : "";
      const label = cluster
        .map((m) => `${markerLabel(m)} at ${formatTime(m.video_time_s)}`)
        .join("; ");

      return `<button type="button" class="marker-glyph"
        style="left:${pct.toFixed(3)}%; --marker-color:${style.color}"
        data-time="${cluster[0].video_time_s}"
        data-cluster="${index}"
        aria-label="${escapeHtml(label)}">${style.icon}${badge}</button>`;
    })
    .join("");
}

function markerStyle(marker: MarkerRow) {
  return MARKER_STYLE[marker.kind] ?? { icon: "●", label: marker.kind, color: "#999" };
}

/** The marker whose icon represents a whole cluster. */
function leadMarker(cluster: MarkerRow[]): MarkerRow {
  return [...cluster].sort((a, b) => rank(a.kind) - rank(b.kind))[0];
}

function rank(kind: string): number {
  const i = MARKER_PRIORITY.indexOf(kind);
  return i === -1 ? MARKER_PRIORITY.length : i;
}

// Candidate spacings for labelled ticks, coarsest-wins. Every entry divides
// cleanly by 4 so the minor ticks between them land on whole seconds.
const RULER_STEPS = [15, 30, 60, 120, 300, 600, 900];
const MAX_RULER_LABELS = 16;

function renderRuler() {
  if (!timelineRuler || !video || !isFinite(video.duration) || !video.duration) return;
  // The window, not the file: the ruler reads 0:00 where the player starts,
  // so the skipped loading screen is not a stretch of timeline with nothing
  // in it.
  const span = windowSpan();
  if (span <= 0) return;

  const major =
    RULER_STEPS.find((step) => span / step <= MAX_RULER_LABELS) ??
    RULER_STEPS[RULER_STEPS.length - 1];

  // Minor ticks are a repeating gradient with a percentage period, so they
  // reflow with the container for free — no resize handling needed.
  timelineRuler.style.setProperty("--minor-gap", `${((major / 4) / span) * 100}%`);

  const labels: string[] = [];
  for (let t = 0; t <= span; t += major) {
    const pct = (t / span) * 100;
    labels.push(
      `<span class="ruler-label" style="left:${pct.toFixed(3)}%">${formatTime(t)}</span>`
    );
  }
  timelineRuler.innerHTML = labels.join("");
}

function updatePlayhead() {
  if (!video || !isFinite(video.duration) || !video.duration) return;
  const pct = windowFraction(video.currentTime) * 100;
  const at = displayTime(video.currentTime);
  const total = windowSpan();
  if (timelinePlayhead) {
    timelinePlayhead.style.left = `${pct.toFixed(3)}%`;
  }
  // Same rAF loop, `seeked` and `loadedmetadata` paths as the playhead above
  // — the in-player bar is just another view of the position.
  if (playerProgress) {
    playerProgress.style.width = `${pct.toFixed(3)}%`;
  }
  if (playerScrub) {
    playerScrub.setAttribute("aria-valuemax", total.toFixed(0));
    playerScrub.setAttribute("aria-valuenow", at.toFixed(0));
    playerScrub.setAttribute(
      "aria-valuetext",
      `${formatTime(at)} of ${formatTime(total)}`
    );
  }
  if (timeDisplay) {
    timeDisplay.textContent = `${formatTime(at)} / ${formatTime(total)}`;
  }
}

// Set when *we* paused playback because the window went away, so that
// becoming visible again only resumes a video the user had actually left
// playing. A hidden window still decodes video and still plays the detached
// stem <audio>, which is the single largest thing this app can burn while
// minimised to the tray.
let pausedByHide = false;

/// Pauses an open, playing VOD while the window is hidden and resumes it
/// after. Pausing cascades through the existing `pause`/`play` handlers, so
/// the rAF playhead loop stops and the stem pauses with it — and `resumeStem`
/// hard-resyncs on the way back, so the stem can't come back drifted.
function onVisibilityChange() {
  if (!video || !currentRecordingPath) return;
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
      // Nothing to recover: the user can press play. Resuming is a courtesy.
    });
  }
}

function startPlayheadLoop() {
  stopPlayheadLoop();
  const tick = () => {
    updatePlayhead();
    correctStemDrift();
    rafHandle = requestAnimationFrame(tick);
  };
  rafHandle = requestAnimationFrame(tick);
}

function stopPlayheadLoop() {
  if (rafHandle !== null) {
    cancelAnimationFrame(rafHandle);
    rafHandle = null;
  }
}

/// Seeks to the position `clientX` falls at along `track`. Takes the element
/// to measure against so the in-player scrub bar and the rich timeline can
/// share it — they differ only in geometry.
function seekFromPointer(track: HTMLElement, clientX: number) {
  if (!video || !isFinite(video.duration) || !video.duration) return;
  const rect = track.getBoundingClientRect();
  if (rect.width === 0) return;
  const fraction = clamp((clientX - rect.left) / rect.width, 0, 1);
  seekTo(windowStart() + fraction * windowSpan());
}

/// Makes `track` a click-to-seek, hold-to-scrub surface.
///
/// `intercept` runs first and returns true if it fully handled the press —
/// the timeline uses it so a click on a marker glyph jumps precisely instead
/// of seeking to the glyph's pixel position.
function bindScrubbing(track: HTMLElement, intercept?: (e: PointerEvent) => boolean) {
  track.addEventListener("pointerdown", (e) => {
    if (!video) return;
    if (intercept?.(e)) return;
    // Seek first, capture second: the seek is the part that must happen, and
    // pointer capture can throw (a pointer that's already been released, a
    // synthetic event) — losing the click to that would be the worse bug.
    seekFromPointer(track, e.clientX);
    scrubbingTrack = track;
    // "Being dragged", set on either track. Only the in-player bar styles it
    // today (to pin its handle visible while the pointer is off the element).
    track.classList.add("open");
    try {
      track.setPointerCapture(e.pointerId);
    } catch {
      // Dragging still works; it just stops tracking outside the element.
    }
  });
  track.addEventListener("pointermove", (e) => {
    if (scrubbingTrack === track) seekFromPointer(track, e.clientX);
  });
  const endScrub = (e: PointerEvent) => {
    if (scrubbingTrack !== track) return;
    scrubbingTrack = null;
    track.classList.remove("open");
    try {
      track.releasePointerCapture(e.pointerId);
    } catch {
      // Never captured (see above) — nothing to release.
    }
  };
  track.addEventListener("pointerup", endScrub);
  track.addEventListener("pointercancel", endScrub);
}

function showClusterTooltip(e: MouseEvent) {
  const glyph = (e.target as HTMLElement).closest<HTMLElement>("[data-cluster]");
  if (!glyph || !timelineTooltip) return;
  const cluster = currentClusters[Number(glyph.dataset.cluster)];
  if (!cluster) return;

  // Payload strings carry other players' names, so they must be escaped —
  // the old timeline interpolated a fixed label table into `title` and got
  // away with it; this doesn't.
  timelineTooltip.innerHTML = cluster
    .map(
      (m) =>
        `<span class="tooltip-row"><span class="marker-icon">${
          markerStyle(m).icon
        }</span>${escapeHtml(markerLabel(m))}<span class="hint">${formatTime(
          m.video_time_s
        )}</span></span>`
    )
    .join("");
  timelineTooltip.style.left = glyph.style.left;
  timelineTooltip.hidden = false;
}

function hideClusterTooltip() {
  if (timelineTooltip) timelineTooltip.hidden = true;
}

// --- Player controls ------------------------------------------------------

function togglePlay() {
  if (!video || !video.src) return;
  if (video.paused) video.play().catch(() => {});
  else video.pause();
}

function syncPlayButton() {
  if (!playPauseBtn || !video) return;
  playPauseBtn.textContent = video.paused ? "▶" : "⏸";
  playPauseBtn.title = video.paused ? "Play (Space)" : "Pause (Space)";
}

function toggleMute() {
  userMuted = !userMuted;
  applyAudioOutput();
  syncVolumeControls();
}

/// Pushes the user's volume/mute intent onto whichever element is actually
/// producing sound. The video is muted whenever a stem is playing — not
/// because the user asked, but because otherwise the combined mix and the
/// isolated stem would play on top of each other.
function applyAudioOutput() {
  if (video) {
    video.volume = userVolume;
    video.muted = userMuted || stemAudio !== null;
  }
  if (stemAudio) {
    stemAudio.volume = userVolume;
    stemAudio.muted = userMuted;
  }
}

function syncVolumeControls() {
  if (volumeSlider) volumeSlider.value = String(userMuted ? 0 : userVolume);
  if (muteBtn) {
    muteBtn.textContent = userMuted || userVolume === 0 ? "🔇" : "🔊";
    muteBtn.title = userMuted ? "Unmute (m)" : "Mute (m)";
  }
}

// --- Audio stems ----------------------------------------------------------

/// The per-recording track layout, as stored by the recorder.
///
/// `null` for anything we didn't record — a rescan-imported file, or a VOD
/// made before multi-track audio existed. Parse failures are treated the
/// same way: an unreadable layout is an unknown one, and the picker hides
/// rather than guessing at a file's contents.
function parseAudioLayout(json: string | null): AudioLayout | null {
  if (!json) return null;
  try {
    const parsed = JSON.parse(json) as AudioLayout;
    return Array.isArray(parsed?.tracks) ? parsed : null;
  } catch {
    return null;
  }
}

/// Renders the stem picker for a recording, or hides it.
///
/// Hidden unless there are at least two tracks: a single-track recording has
/// nothing to choose between, and neither does one imported by a rescan,
/// whose layout we genuinely don't know.
function renderTrackPicker(layout: AudioLayout | null) {
  selectedTrack = 0;

  const tracks = layout?.tracks ?? [];
  if (trackField) trackField.hidden = tracks.length < 2;
  if (!trackSelect) return;

  trackSelect.innerHTML = tracks
    .map((track, i) => `<option value="${i}">${escapeHtml(track.label)}</option>`)
    .join("");
  trackSelect.value = "0";
}

/// Switches which audio track is audible.
///
/// Track 0 is the combined mix and plays straight off the video element.
/// Anything else has to be extracted to its own file first — WebView2 gives
/// no way to select among the audio tracks of one `<video>`. See
/// DEVELOPMENT.md §2.5.
async function selectTrack(index: number) {
  if (!video || !currentRecordingPath) return;
  selectedTrack = index;

  if (index === 0) {
    detachStem();
    applyAudioOutput();
    return;
  }

  try {
    const path = await call<string>("extract_audio_track", {
      recordingPath: currentRecordingPath,
      trackIndex: index,
    });
    // The user can switch again while an extraction is in flight, and a
    // slow one must not stomp a newer choice.
    if (selectedTrack !== index) return;
    attachStem(assetUrl(path));
  } catch (err) {
    if (selectedTrack !== index) return;
    toast(`Couldn't load that audio track: ${err}`, "error");
    // Fall back to the combined mix rather than leaving the player silent
    // with a picker claiming otherwise.
    selectedTrack = 0;
    if (trackSelect) trackSelect.value = "0";
    detachStem();
    applyAudioOutput();
  }
}

function attachStem(src: string) {
  if (!video) return;
  detachStem();

  const audio = new Audio(src);
  audio.preload = "auto";
  audio.currentTime = video.currentTime;
  audio.playbackRate = video.playbackRate;
  stemAudio = audio;
  applyAudioOutput();
  void resumeStem();
}

function detachStem() {
  if (!stemAudio) return;
  stemAudio.pause();
  stemAudio.removeAttribute("src");
  stemAudio.load();
  stemAudio = null;
  applyAudioOutput();
}

async function resumeStem() {
  if (!stemAudio || !video) return;
  stemAudio.currentTime = video.currentTime;
  stemAudio.playbackRate = video.playbackRate;
  if (video.paused) return;
  try {
    await stemAudio.play();
  } catch {
    // Autoplay rejection or a load race — the drift check re-tries on the
    // next frame, so this doesn't need to be loud.
  }
}

/// Keeps the stem aligned with the video. Called from the rAF playhead loop,
/// which only runs while playing, so a paused player costs nothing.
function correctStemDrift() {
  if (!stemAudio || !video || video.paused || stemAudio.seeking) return;

  const drift = stemAudio.currentTime - video.currentTime;
  if (Math.abs(drift) > SYNC_HARD) {
    stemAudio.currentTime = video.currentTime;
    stemAudio.playbackRate = video.playbackRate;
  } else if (Math.abs(drift) > SYNC_NUDGE) {
    // Ease back into alignment instead of seeking, which would be audible
    // as a click at this magnitude.
    stemAudio.playbackRate = video.playbackRate * (drift > 0 ? 0.98 : 1.02);
  } else {
    stemAudio.playbackRate = video.playbackRate;
  }
}

function toggleFullscreen() {
  if (!playerWrap) return;
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  else playerWrap.requestFullscreen().catch(() => {});
}

/// Reflects the real fullscreen state on the button. Driven by
/// `fullscreenchange`, not by `toggleFullscreen`, because Escape and the OS
/// can both change it without going through us.
///
/// The glyph stays ⛶ in both states: the icon set already in use here is
/// emoji, and there is no exit-fullscreen emoji with dependable coverage —
/// a missing-glyph box would be worse than a static icon. The state is
/// carried by `aria-pressed` (which screen readers announce) and the title.
function syncFullscreenButton() {
  if (!fullscreenBtn) return;
  const on = document.fullscreenElement === playerWrap;
  fullscreenBtn.setAttribute("aria-pressed", String(on));
  fullscreenBtn.title = on ? "Exit fullscreen (f)" : "Fullscreen (f)";
}

function isSettingsMenuOpen(): boolean {
  return !!settingsMenu && !settingsMenu.hidden;
}

function setSettingsMenu(open: boolean) {
  if (!settingsMenu) return;
  settingsMenu.hidden = !open;
  settingsBtn?.setAttribute("aria-expanded", String(open));
}

// Only ever appended when the flag is explicitly true: it is absent on
// every marker recorded before steals were captured, and `undefined` is
// not "it wasn't stolen", it's "nobody asked".
function stolen(payload: Record<string, unknown>): string {
  return payload.stolen === true ? " (stolen)" : "";
}

// 2 through 5 have names everyone uses; anything beyond that is either a
// pentakill already or a game mode where counting up is the wrong answer.
const MULTIKILL_NAMES: Record<number, string> = {
  2: "Double Kill",
  3: "Triple Kill",
  4: "Quadra Kill",
  5: "Penta Kill",
};

function multikillLabel(streak: unknown): string {
  if (typeof streak !== "number") return "Multikill";
  return MULTIKILL_NAMES[streak] ?? `${streak}× Multikill`;
}

function markerLabel(m: MarkerRow): string {
  let payload: Record<string, unknown> = {};
  try {
    payload = JSON.parse(m.payload_json);
  } catch {
    // Malformed payload — fall back to just the kind below.
  }
  const str = (key: string) => (typeof payload[key] === "string" ? (payload[key] as string) : "?");

  switch (m.kind) {
    // The three that name somebody: who you killed, who killed you, and
    // whose kill you helped with. That name is the whole content of the
    // marker and it is never yours, so it stays.
    case "kill":
      return `Killed ${str("victim")}`;
    case "death":
      return `Killed by ${str("killer")}`;
    case "assist":
      return `${str("killer")} killed ${str("victim")}`;

    // The objectives name nobody. `classify_event` only writes one of
    // these when you took part — `took_part()` gates every branch — so the
    // killer is you or an ally you assisted, and printing it told you
    // either your own champion's name or a detail you were not scrubbing
    // for. Same for the elemental type: it says which drake, not which
    // moment, and "Fire Dragon — Shyvana" is four words to say "Dragon".
    case "dragon":
      return `${isElder(payload) ? "Elder Dragon" : "Dragon"}${stolen(payload)}`;
    case "baron":
      return `Baron${stolen(payload)}`;
    case "herald":
      return `Herald${stolen(payload)}`;
    case "turret":
      return "Turret";
    case "inhibitor":
      return "Inhibitor";

    // `Ace` is only recorded when you landed the closing kill, so the
    // acing team was always yours; `FirstBlood` only when you got it.
    // Both suffixes were constants dressed as data.
    case "ace":
      return "Ace";
    case "first_blood":
      return "First Blood";

    case "multikill":
      return multikillLabel(payload.kill_streak);
    default:
      return m.kind;
  }
}

/**
 * Elder is the one dragon type worth keeping.
 *
 * The elemental drakes are interchangeable to somebody scrubbing a VOD —
 * the moment is "we took a dragon", and which one it was does not change
 * what is on screen. Elder is a different objective: it usually decides
 * the game, and it is a thing you would go looking for by name.
 */
function isElder(payload: Record<string, unknown>): boolean {
  const type = payload.dragon_type;
  return typeof type === "string" && type.trim().toLowerCase() === "elder";
}

function renderMarkerList() {
  if (!markerListEl) return;
  if (currentMarkers.length === 0) {
    markerListEl.innerHTML = `<li class="hint">No markers recorded for this game.</li>`;
    return;
  }
  markerListEl.innerHTML = currentMarkers
    .map((m) => {
      const style = MARKER_STYLE[m.kind] ?? { icon: "●", label: m.kind, color: "#999" };
      return `<li data-time="${m.video_time_s}" style="--marker-color:${style.color}">
        <span class="marker-icon">${style.icon}</span>
        <span class="marker-label">${escapeHtml(markerLabel(m))}</span>
        <span class="hint">${formatTime(m.video_time_s)}</span>
      </li>`;
    })
    .join("");
}

export async function openReview(row: RecordingRow) {
  if (!video) return;

  currentRecordingPath = row.path;
  if (reviewTitle) reviewTitle.textContent = vodTitle(row);
  if (videoError) videoError.hidden = true;
  if (videoErrorText) videoErrorText.textContent = "";
  if (videoErrorDetail) videoErrorDetail.textContent = "";
  video.src = assetUrl(row.path);
  video.playbackRate = rateSelect ? Number(rateSelect.value) : 1;
  detachStem();
  renderTrackPicker(parseAudioLayout(row.audio_tracks_json));

  currentMarkers = [];
  currentSamples = [];
  currentClusters = [];
  gameStartsAt = 0;
  startApplied = false;
  renderTimeline();
  renderMarkerList();
  syncPlayButton();
  applyAudioOutput();
  syncVolumeControls();

  showView("review");

  try {
    const [markers, samples] = await Promise.all([
      call<MarkerRow[]>("get_recording_markers", { recordingId: row.id }),
      call<SampleRow[]>("get_recording_samples", { recordingId: row.id }),
    ]);
    currentMarkers = markers;
    currentSamples = samples;
    gameStartsAt = measureGameStart(samples);
    applyStartPosition();
  } catch (err) {
    console.error("Failed to load timeline data", err);
  }
  renderMarkerList();
  // Duration may not have been known when we first tried (loadedmetadata
  // hadn't fired yet) — render again now that the data is in either way.
  renderTimeline();
}

function closeReview() {
  if (!video) return;
  pausedByHide = false;
  stopPlayheadLoop();
  hideClusterTooltip();
  setSettingsMenu(false);
  detachStem();
  renderTrackPicker(null);
  video.pause();
  video.removeAttribute("src");
  video.load();
  // `updatePlayhead` bails out without a duration, so the bar would otherwise
  // keep the last recording's position until the next one loads.
  if (playerProgress) playerProgress.style.width = "0%";
  currentRecordingPath = null;
  currentSamples = [];
  currentClusters = [];
  showView("library");
}
