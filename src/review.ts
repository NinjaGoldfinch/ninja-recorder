import { invoke, convertFileSrc } from "@tauri-apps/api/core";

export interface RecordingRow {
  id: number;
  path: string;
  started_at: number;
  duration_s: number | null;
  game_id: number | null;
  queue: number | null;
  champion: string | null;
  role: string | null;
  win: boolean | null;
  kda_k: number | null;
  kda_d: number | null;
  kda_a: number | null;
  patch: string | null;
  pinned: boolean;
  size_bytes: number;
}

interface MarkerRow {
  id: number;
  recording_id: number;
  game_time_s: number;
  video_time_s: number;
  kind: string;
  payload_json: string;
}

interface SampleRow {
  id: number;
  recording_id: number;
  game_time_s: number;
  video_time_s: number;
  our_team: string | null;
  gold_diff_est: number | null;
  kill_diff: number | null;
  cs_diff: number | null;
  our_gold: number | null;
  our_level: number | null;
}

const MARKER_STYLE: Record<string, { icon: string; label: string; color: string }> = {
  kill: { icon: "⚔️", label: "Kill", color: "#43a047" },
  death: { icon: "💀", label: "Death", color: "#e53935" },
  assist: { icon: "🤝", label: "Assist", color: "#1e88e5" },
  dragon: { icon: "🐉", label: "Dragon", color: "#8e24aa" },
  baron: { icon: "👑", label: "Baron", color: "#6d4c41" },
  herald: { icon: "🦅", label: "Herald", color: "#00897b" },
  turret: { icon: "🏰", label: "Turret", color: "#fb8c00" },
  ace: { icon: "⭐", label: "Ace", color: "#fdd835" },
  first_blood: { icon: "🩸", label: "First Blood", color: "#d81b60" },
};

// When several markers collapse into one timeline glyph, the cluster shows
// a single icon — this is which one wins. Ordered by how much the event
// changes what you're looking for in a VOD: your own deaths and kills first,
// then objectives by value, with assists last because they're the most
// numerous and the least individually interesting.
const MARKER_PRIORITY = [
  "death",
  "kill",
  "baron",
  "dragon",
  "herald",
  "ace",
  "first_blood",
  "turret",
  "assist",
];

// Real per-recording frame rate isn't probed anywhere yet (that's an
// encoder-config concern for Phase 6) — 1/30s is a reasonable
// approximation for review purposes, not frame-exact.
const FRAME_SECONDS = 1 / 30;

let reviewView: HTMLElement | null;
let libraryView: HTMLElement | null;
let backBtn: HTMLButtonElement | null;
let reviewTitle: HTMLElement | null;
let video: HTMLVideoElement | null;
let videoError: HTMLElement | null;
let videoErrorText: HTMLElement | null;
let videoErrorDetail: HTMLElement | null;
let frameBackBtn: HTMLButtonElement | null;
let frameFwdBtn: HTMLButtonElement | null;
let rateSelect: HTMLSelectElement | null;
let markerListEl: HTMLElement | null;
let playerWrap: HTMLElement | null;
let playPauseBtn: HTMLButtonElement | null;
let timeDisplay: HTMLElement | null;
let muteBtn: HTMLButtonElement | null;
let volumeSlider: HTMLInputElement | null;
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
let isScrubbing = false;

export function initReview() {
  reviewView = document.querySelector("#review-view");
  libraryView = document.querySelector("#library-view");
  backBtn = document.querySelector("#back-to-library-btn");
  reviewTitle = document.querySelector("#review-title");
  video = document.querySelector("#review-video");
  videoError = document.querySelector("#review-video-error");
  videoErrorText = document.querySelector("#review-video-error-text");
  videoErrorDetail = document.querySelector("#review-video-error-detail");
  frameBackBtn = document.querySelector("#frame-back-btn");
  frameFwdBtn = document.querySelector("#frame-fwd-btn");
  rateSelect = document.querySelector("#playback-rate-select");
  markerListEl = document.querySelector("#marker-list");
  playerWrap = document.querySelector(".player-wrap");
  playPauseBtn = document.querySelector("#play-pause-btn");
  timeDisplay = document.querySelector("#time-display");
  muteBtn = document.querySelector("#mute-btn");
  volumeSlider = document.querySelector("#volume-slider");
  fullscreenBtn = document.querySelector("#fullscreen-btn");
  timelineBody = document.querySelector("#timeline-body");
  timelineGraph = document.querySelector("#timeline-graph");
  timelineGlyphs = document.querySelector("#timeline-glyphs");
  timelineRuler = document.querySelector("#timeline-ruler");
  timelinePlayhead = document.querySelector("#timeline-playhead");
  timelineTooltip = document.querySelector("#timeline-tooltip");
  metricSelect = document.querySelector("#timeline-metric-select");
  metricSummary = document.querySelector("#timeline-metric-summary");

  backBtn?.addEventListener("click", closeReview);
  frameBackBtn?.addEventListener("click", () => stepFrame(-1));
  frameFwdBtn?.addEventListener("click", () => stepFrame(1));
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
  });
  video?.addEventListener("pause", () => {
    syncPlayButton();
    stopPlayheadLoop();
    updatePlayhead();
  });
  video?.addEventListener("ended", stopPlayheadLoop);
  video?.addEventListener("seeked", updatePlayhead);
  video?.addEventListener("loadedmetadata", updatePlayhead);
  video?.addEventListener("volumechange", syncVolumeControls);

  playPauseBtn?.addEventListener("click", togglePlay);
  muteBtn?.addEventListener("click", toggleMute);
  fullscreenBtn?.addEventListener("click", toggleFullscreen);
  volumeSlider?.addEventListener("input", () => {
    if (!video || !volumeSlider) return;
    video.volume = Number(volumeSlider.value);
    video.muted = video.volume === 0;
  });

  metricSelect?.addEventListener("change", () => {
    currentMetric = (metricSelect!.value as MetricKey) ?? "gold_diff";
    renderGraph();
  });

  // Seeking. A click on a glyph keeps its existing precise-jump behaviour;
  // anywhere else on the track seeks to that position, and holding scrubs.
  timelineBody?.addEventListener("pointerdown", (e) => {
    if (!video) return;
    const target = (e.target as HTMLElement).closest<HTMLElement>("[data-time]");
    if (target) {
      video.currentTime = Number(target.dataset.time);
      return;
    }
    // Seek first, capture second: the seek is the part that must happen, and
    // pointer capture can throw (a pointer that's already been released, a
    // synthetic event) — losing the click to that would be the worse bug.
    seekFromPointer(e.clientX);
    isScrubbing = true;
    try {
      timelineBody?.setPointerCapture(e.pointerId);
    } catch {
      // Dragging still works; it just stops tracking outside the element.
    }
  });
  timelineBody?.addEventListener("pointermove", (e) => {
    if (isScrubbing) seekFromPointer(e.clientX);
  });
  const endScrub = (e: PointerEvent) => {
    if (!isScrubbing) return;
    isScrubbing = false;
    try {
      timelineBody?.releasePointerCapture(e.pointerId);
    } catch {
      // Never captured (see above) — nothing to release.
    }
  };
  timelineBody?.addEventListener("pointerup", endScrub);
  timelineBody?.addEventListener("pointercancel", endScrub);

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
    if (target && video) video.currentTime = Number(target.dataset.time);
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

function stepFrame(direction: 1 | -1) {
  if (!video) return;
  video.pause();
  video.currentTime = Math.max(0, video.currentTime + direction * FRAME_SECONDS);
}

function isReviewOpen(): boolean {
  return !!reviewView && !reviewView.hidden;
}

function isTypingInField(): boolean {
  const el = document.activeElement;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA");
}

function isOnFormControl(): boolean {
  const el = document.activeElement;
  return !!el && (el.tagName === "BUTTON" || el.tagName === "SELECT");
}

function seekBy(seconds: number) {
  if (!video || !isFinite(video.duration)) return;
  video.currentTime = clamp(video.currentTime + seconds, 0, video.duration);
}

/**
 * Hotkeys: Space play/pause, arrows seek 5s, `[`/`]` prev/next marker,
 * `d`/`D` prev/next death, `f` fullscreen, `m` mute.
 */
function handleHotkey(e: KeyboardEvent) {
  if (!isReviewOpen() || isTypingInField()) return;

  // Space and arrows are the browser's own controls for a focused button or
  // select, so leave them alone there — stealing Space would break every
  // control in the row the moment one had focus.
  if ((e.key === " " || e.key.startsWith("Arrow")) && isOnFormControl()) return;

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
  video.currentTime = target.video_time_s;
}

// --- Timeline rendering ---------------------------------------------------

type MetricKey = "gold_diff" | "kill_diff" | "cs_diff" | "none";

const METRIC_META: Record<
  Exclude<MetricKey, "none">,
  { label: string; estimated: boolean; format: (v: number) => string }
> = {
  gold_diff: { label: "Gold diff", estimated: true, format: formatSignedGold },
  kill_diff: { label: "Kill diff", estimated: false, format: formatSigned },
  cs_diff: { label: "CS diff", estimated: false, format: formatSigned },
};

const GOLD_ESTIMATE_CAVEAT =
  "Estimated, not exact. The Live Client Data API exposes no per-player gold, " +
  "so this is the summed price of the items each team is holding, plus your own " +
  "unspent gold. Sold items, consumables and the enemy's unspent gold aren't " +
  "accounted for. Kill diff and CS diff are exact.";

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
      return sample.gold_diff_est;
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
    setMetricSummary("Not enough data to plot", true);
    return;
  }

  const reduced = downsample(points, 500);
  const bound = Math.max(...reduced.map((p) => Math.abs(p.v)));
  const duration = video.duration;

  const x = (t: number) => clamp((t / duration) * 1000, 0, 1000);
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
  const label = meta.estimated ? `${meta.label} (est.)` : meta.label;
  setMetricSummary(
    `${label} · ${meta.format(last)} at end · peak ${meta.format(peak)}`,
    false,
    meta.estimated ? GOLD_ESTIMATE_CAVEAT : ""
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
  const duration = video.duration;

  currentClusters = [];
  let clusterStartX = -Infinity;
  for (const marker of currentMarkers) {
    const px = (marker.video_time_s / duration) * width;
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
      const pct = clamp((mean / duration) * 100, 0, 100);
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
  const duration = video.duration;

  const major =
    RULER_STEPS.find((step) => duration / step <= MAX_RULER_LABELS) ??
    RULER_STEPS[RULER_STEPS.length - 1];

  // Minor ticks are a repeating gradient with a percentage period, so they
  // reflow with the container for free — no resize handling needed.
  timelineRuler.style.setProperty("--minor-gap", `${((major / 4) / duration) * 100}%`);

  const labels: string[] = [];
  for (let t = 0; t <= duration; t += major) {
    const pct = (t / duration) * 100;
    labels.push(
      `<span class="ruler-label" style="left:${pct.toFixed(3)}%">${formatTime(t)}</span>`
    );
  }
  timelineRuler.innerHTML = labels.join("");
}

function updatePlayhead() {
  if (!video || !isFinite(video.duration) || !video.duration) return;
  if (timelinePlayhead) {
    const pct = clamp((video.currentTime / video.duration) * 100, 0, 100);
    timelinePlayhead.style.left = `${pct.toFixed(3)}%`;
  }
  if (timeDisplay) {
    timeDisplay.textContent = `${formatTime(video.currentTime)} / ${formatTime(
      video.duration
    )}`;
  }
}

function startPlayheadLoop() {
  stopPlayheadLoop();
  const tick = () => {
    updatePlayhead();
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

function seekFromPointer(clientX: number) {
  if (!video || !timelineBody || !isFinite(video.duration) || !video.duration) return;
  const rect = timelineBody.getBoundingClientRect();
  if (rect.width === 0) return;
  video.currentTime = clamp((clientX - rect.left) / rect.width, 0, 1) * video.duration;
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
  if (!video) return;
  video.muted = !video.muted;
}

function syncVolumeControls() {
  if (!video) return;
  if (volumeSlider) volumeSlider.value = String(video.muted ? 0 : video.volume);
  if (muteBtn) {
    muteBtn.textContent = video.muted || video.volume === 0 ? "🔇" : "🔊";
    muteBtn.title = video.muted ? "Unmute (m)" : "Mute (m)";
  }
}

function toggleFullscreen() {
  if (!playerWrap) return;
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  else playerWrap.requestFullscreen().catch(() => {});
}

function formatTime(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
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
    case "kill":
      return `Killed ${str("victim")}`;
    case "death":
      return `Killed by ${str("killer")}`;
    case "assist":
      return `${str("killer")} killed ${str("victim")}`;
    case "dragon":
      return `${str("dragon_type")} Dragon — ${str("killer")}`;
    case "baron":
      return `Baron — ${str("killer")}`;
    case "herald":
      return `Herald — ${str("killer")}`;
    case "turret":
      return `Turret destroyed — ${str("killer")}`;
    case "ace":
      return `Ace (${str("acing_team")})`;
    case "first_blood":
      return `First Blood — ${str("recipient")}`;
    default:
      return m.kind;
  }
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
        <span>${escapeHtml(markerLabel(m))}</span>
        <span class="hint">${formatTime(m.video_time_s)}</span>
      </li>`;
    })
    .join("");
}

function escapeHtml(value: string): string {
  const div = document.createElement("div");
  div.textContent = value;
  return div.innerHTML;
}

export async function openReview(row: RecordingRow) {
  if (!reviewView || !libraryView || !video) return;

  currentRecordingPath = row.path;
  if (reviewTitle) reviewTitle.textContent = row.champion ?? `Recording #${row.id}`;
  if (videoError) videoError.hidden = true;
  if (videoErrorText) videoErrorText.textContent = "";
  if (videoErrorDetail) videoErrorDetail.textContent = "";
  video.src = convertFileSrc(row.path);
  video.playbackRate = rateSelect ? Number(rateSelect.value) : 1;

  currentMarkers = [];
  currentSamples = [];
  currentClusters = [];
  renderTimeline();
  renderMarkerList();
  syncPlayButton();
  syncVolumeControls();

  libraryView.hidden = true;
  reviewView.hidden = false;

  try {
    const [markers, samples] = await Promise.all([
      invoke<MarkerRow[]>("get_recording_markers", { recordingId: row.id }),
      invoke<SampleRow[]>("get_recording_samples", { recordingId: row.id }),
    ]);
    currentMarkers = markers;
    currentSamples = samples;
  } catch (err) {
    console.error("Failed to load timeline data", err);
  }
  renderMarkerList();
  // Duration may not have been known when we first tried (loadedmetadata
  // hadn't fired yet) — render again now that the data is in either way.
  renderTimeline();
}

function closeReview() {
  if (!reviewView || !libraryView || !video) return;
  stopPlayheadLoop();
  hideClusterTooltip();
  video.pause();
  video.removeAttribute("src");
  video.load();
  currentRecordingPath = null;
  currentSamples = [];
  currentClusters = [];
  reviewView.hidden = true;
  libraryView.hidden = false;
}
