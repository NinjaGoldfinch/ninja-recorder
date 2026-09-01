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
let frameBackBtn: HTMLButtonElement | null;
let frameFwdBtn: HTMLButtonElement | null;
let rateSelect: HTMLSelectElement | null;
let timelineEl: HTMLElement | null;
let markerListEl: HTMLElement | null;

let currentMarkers: MarkerRow[] = [];

export function initReview() {
  reviewView = document.querySelector("#review-view");
  libraryView = document.querySelector("#library-view");
  backBtn = document.querySelector("#back-to-library-btn");
  reviewTitle = document.querySelector("#review-title");
  video = document.querySelector("#review-video");
  videoError = document.querySelector("#review-video-error");
  frameBackBtn = document.querySelector("#frame-back-btn");
  frameFwdBtn = document.querySelector("#frame-fwd-btn");
  rateSelect = document.querySelector("#playback-rate-select");
  timelineEl = document.querySelector("#marker-timeline");
  markerListEl = document.querySelector("#marker-list");

  backBtn?.addEventListener("click", closeReview);
  frameBackBtn?.addEventListener("click", () => stepFrame(-1));
  frameFwdBtn?.addEventListener("click", () => stepFrame(1));
  rateSelect?.addEventListener("change", () => {
    if (video && rateSelect) video.playbackRate = Number(rateSelect.value);
  });
  video?.addEventListener("loadedmetadata", renderTimeline);
  video?.addEventListener("error", () => {
    if (videoError) videoError.hidden = false;
  });

  timelineEl?.addEventListener("click", (e) => {
    const target = (e.target as HTMLElement).closest<HTMLElement>("[data-time]");
    if (target && video) video.currentTime = Number(target.dataset.time);
  });
  markerListEl?.addEventListener("click", (e) => {
    const target = (e.target as HTMLElement).closest<HTMLElement>("li[data-time]");
    if (target && video) video.currentTime = Number(target.dataset.time);
  });

  document.addEventListener("keydown", handleHotkey);
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

/** Hotkeys: `[`/`]` = prev/next marker (any kind), `d`/`D` = prev/next death. */
function handleHotkey(e: KeyboardEvent) {
  if (!isReviewOpen() || isTypingInField()) return;

  if (e.key === "]") {
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

// Markers seconds apart in a long game can land within this many percentage
// points of each other on the timeline — close enough that their glyphs
// would overlap. Alternating a "lane" (vertical offset) for anything that
// close to its predecessor fans them out instead of stacking illegibly.
const COLLISION_THRESHOLD_PCT = 2.5;

function renderTimeline() {
  if (!timelineEl || !video || !video.duration || !isFinite(video.duration)) return;
  const duration = video.duration;

  let lastPct: number | null = null;
  let lane = 0;

  timelineEl.innerHTML = currentMarkers
    .map((m) => {
      const style = MARKER_STYLE[m.kind] ?? { icon: "●", label: m.kind, color: "#999" };
      const pct = Math.min(100, Math.max(0, (m.video_time_s / duration) * 100));

      lane = lastPct !== null && pct - lastPct < COLLISION_THRESHOLD_PCT ? 1 - lane : 0;
      lastPct = pct;
      const topPct = lane === 0 ? 25 : 75;

      return `<button type="button" class="marker-glyph" style="left:${pct}%; top:${topPct}%; --marker-color:${style.color}" data-time="${m.video_time_s}" title="${style.label} at ${formatTime(m.video_time_s)}">${style.icon}</button>`;
    })
    .join("");
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

  if (reviewTitle) reviewTitle.textContent = row.champion ?? `Recording #${row.id}`;
  if (videoError) videoError.hidden = true;
  video.src = convertFileSrc(row.path);
  video.playbackRate = rateSelect ? Number(rateSelect.value) : 1;

  currentMarkers = [];
  renderTimeline();
  renderMarkerList();

  libraryView.hidden = true;
  reviewView.hidden = false;

  try {
    currentMarkers = await invoke<MarkerRow[]>("get_recording_markers", {
      recordingId: row.id,
    });
  } catch (err) {
    console.error("Failed to load markers", err);
  }
  renderMarkerList();
  // Duration may not have been known when we first tried (loadedmetadata
  // hadn't fired yet) — render again now that markers are in either way.
  renderTimeline();
}

function closeReview() {
  if (!reviewView || !libraryView || !video) return;
  video.pause();
  video.removeAttribute("src");
  video.load();
  reviewView.hidden = true;
  libraryView.hidden = false;
}
