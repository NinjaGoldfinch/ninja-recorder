import { call } from "./bridge";
import { el, escapeAttr, escapeHtml } from "./dom";
import {
  championIcon,
  itemIcon,
  loadIcons,
  parseScoreboard,
  runeIcon,
  spellIcon,
  spellIconById,
} from "./icons";
import {
  formatBytes,
  formatClock,
  formatDateTime,
  formatKda,
  formatRelative,
  patchLabel,
  formatSpan,
  kdaRatio,
  queueOrModeLabel,
  vodTitle,
} from "./format";
import { getPrefs } from "./prefs";
import { currentView, onViewChange } from "./router";
import { openReview } from "./review";
import { toast } from "./toast";
import type {
  DiskUsage,
  ReconcileReport,
  RecordingRow,
  ScoreboardPlayer,
} from "./types";

interface Els {
  grid: HTMLElement;
  empty: HTMLElement;
  noMatches: HTMLElement;
  noMatchesHint: HTMLElement;
  clearFilters: HTMLButtonElement;
  champion: HTMLInputElement;
  outcome: HTMLSelectElement;
  queue: HTMLSelectElement;
  role: HTMLSelectElement;
  patch: HTMLSelectElement;
  pinned: HTMLInputElement;
  sort: HTMLSelectElement;
  refresh: HTMLButtonElement;
  rescan: HTMLButtonElement;
  statGames: HTMLElement;
  statGamesSub: HTMLElement;
  statWinrate: HTMLElement;
  statWinrateSub: HTMLElement;
  statPlaytime: HTMLElement;
  statPlaytimeSub: HTMLElement;
  statDisk: HTMLElement;
  statDiskSub: HTMLElement;
}

let els: Els;

// The full set fetched from the DB; filters/sort below operate on this
// in-memory rather than re-querying, since the dataset is small and local.
let allRecordings: RecordingRow[] = [];
let usage: DiskUsage | null = null;

// A refresh that lands while the user is in the review or settings view
// writes the data but defers the re-render. Rebuilding the grid under a
// hidden view is wasted work, and doing it as they navigate back would
// yank the card they came from out from under them.
let pendingRender = false;

// Delete is a two-step on the button itself rather than a modal: the
// recording id currently armed, if any.
let armedForDelete: number | null = null;
let armTimer: number | undefined;

export function initLibrary() {
  els = {
    grid: el("#library-grid"),
    empty: el("#library-empty"),
    noMatches: el("#library-no-matches"),
    noMatchesHint: el("#no-matches-hint"),
    clearFilters: el<HTMLButtonElement>("#clear-filters-btn"),
    champion: el<HTMLInputElement>("#filter-champion"),
    outcome: el<HTMLSelectElement>("#filter-outcome"),
    queue: el<HTMLSelectElement>("#filter-queue"),
    role: el<HTMLSelectElement>("#filter-role"),
    patch: el<HTMLSelectElement>("#filter-patch"),
    pinned: el<HTMLInputElement>("#filter-pinned"),
    sort: el<HTMLSelectElement>("#sort-select"),
    refresh: el<HTMLButtonElement>("#refresh-btn"),
    rescan: el<HTMLButtonElement>("#rescan-btn"),
    statGames: el("#stat-games"),
    statGamesSub: el("#stat-games-sub"),
    statWinrate: el("#stat-winrate"),
    statWinrateSub: el("#stat-winrate-sub"),
    statPlaytime: el("#stat-playtime"),
    statPlaytimeSub: el("#stat-playtime-sub"),
    statDisk: el("#stat-disk"),
    statDiskSub: el("#stat-disk-sub"),
  };

  facets = [
    { select: els.queue, key: (row) => queueOrModeLabel(row), compare: byName },
    { select: els.role, key: (row) => row.role, compare: byLane },
    { select: els.patch, key: (row) => patchLabel(row.patch), compare: byPatchDesc },
  ];

  els.champion.addEventListener("input", render);
  els.outcome.addEventListener("change", render);
  els.pinned.addEventListener("change", render);
  els.sort.addEventListener("change", render);
  for (const facet of facets) facet.select.addEventListener("change", render);
  els.clearFilters.addEventListener("click", clearFilters);
  els.refresh.addEventListener("click", () => {
    refreshLibrary();
    refreshDiskUsage();
  });
  els.rescan.addEventListener("click", rescanRecordings);

  els.grid.addEventListener("click", onGridClick);
  // Rows are focusable, so they need to be openable from the keyboard —
  // they carried `tabindex` before the first redesign but no key handler.
  els.grid.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const card = (e.target as HTMLElement).closest<HTMLElement>(".vod-row");
    if (!card) return;
    e.preventDefault();
    const row = findRow(Number(card.dataset.id));
    if (row) openReview(row);
  });

  onViewChange((view) => {
    if (view === "library" && pendingRender) render();
  });
}

// Called once prefs have loaded, which is after the first render.
export function applyDefaultSort() {
  els.sort.value = getPrefs().defaultSort;
  render();
}

function findRow(id: number): RecordingRow | undefined {
  return allRecordings.find((r) => r.id === id);
}

// --- The derived filters -------------------------------------------
//
// Queue, role and patch are built from the rows the library actually
// holds rather than from a fixed vocabulary. Patch is open-ended and
// could not be listed ahead of time at all; queue ids are a table this
// app only partly names, so `Queue 1234` is a real label a hard-coded
// list would have no entry for; and a fixed list offers "Ranked Flex" to
// somebody who has never queued it, which is a control that can only ever
// empty the list.
//
// They are derived from the *whole* library, not from what the other
// filters leave. A facet that narrows as you use its neighbours is how a
// person ends up with a selection they can no longer see the way out of.

/** Every named value carries this prefix, so no label can collide with the
 *  two reserved option values below. */
const VALUE_PREFIX = "v:";
/** No filtering on this facet. Shared with `#filter-outcome`'s own markup. */
const ANY = "all";
/** Rows whose column is empty — offered only when there are some. */
const NONE = "none";

interface Facet {
  select: HTMLSelectElement;
  /** Which bucket a row falls in, or null when its column is empty. */
  key: (row: RecordingRow) => string | null;
  /** Orders the named values. "Unknown" is appended after them regardless. */
  compare: (a: string, b: string) => number;
}

// Built in `initLibrary`, once the elements exist.
let facets: Facet[] = [];

function byName(a: string, b: string): number {
  return a.localeCompare(b);
}

// The order the game lists them in, not alphabetical: nobody scans a lane
// picker for "Bottom, Jungle, Middle, Support, Top".
const LANE_ORDER = ["Top", "Jungle", "Middle", "Bottom", "Support"];

function byLane(a: string, b: string): number {
  const ia = LANE_ORDER.indexOf(a);
  const ib = LANE_ORDER.indexOf(b);
  // `position()` only ever writes those five, but `role` is a TEXT column
  // and the LCU is not guaranteed to stay its only writer. Anything else
  // sorts after them rather than being dropped from the list.
  if (ia === -1 && ib === -1) return byName(a, b);
  if (ia === -1) return 1;
  if (ib === -1) return -1;
  return ia - ib;
}

/** `"15.10"` → `[15, 10]`, or null if it is not a run of numbers. */
function patchParts(label: string): number[] | null {
  const parts = label.split(".").map(Number);
  return parts.every((n) => Number.isFinite(n)) ? parts : null;
}

// Newest first, compared component-wise as numbers — "15.9" sorts *older*
// than "15.10", which is exactly what comparing them as strings gets
// wrong. `patchLabel` passes a malformed patch through untouched, so a
// label that is not a version still has to order somehow.
function byPatchDesc(a: string, b: string): number {
  const pa = patchParts(a);
  const pb = patchParts(b);
  if (pa === null || pb === null) return byName(a, b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const diff = (pb[i] ?? 0) - (pa[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/**
 * Rebuilds the three derived filters from the rows now in the library,
 * keeping each selection when the value it names is still present.
 */
function refreshFacets() {
  for (const facet of facets) {
    const values = new Set<string>();
    let unknowns = false;
    for (const row of allRecordings) {
      const key = facet.key(row);
      if (key === null) unknowns = true;
      else values.add(key);
    }
    const named = [...values].sort(facet.compare);

    const previous = facet.select.value;
    // Option 0 is the "All …" one and lives in the markup, not the data.
    while (facet.select.options.length > 1) facet.select.remove(1);
    // `new Option` sets the label as a text node. `game_mode` reaches these
    // labels straight from Live Client Data, so it is not built into HTML.
    for (const value of named) {
      facet.select.add(new Option(value, VALUE_PREFIX + value));
    }
    // "Unknown" is worth offering, and only when something is missing:
    // "which of my games never got a role" is the question the `Unknown`
    // on the row itself prompts, and the backfill leaves plenty of them.
    if (unknowns) facet.select.add(new Option("Unknown", NONE));

    // A selection whose value has left the library falls back to All
    // rather than quietly filtering everything out. Assigning a value the
    // select does not carry leaves `value` as the empty string.
    facet.select.value = previous;
    if (!facet.select.value) facet.select.value = ANY;

    // One choice is no choice — a library of nothing but ARAM has no queue
    // to pick between. But a facet that is *currently* filtering is never
    // disabled: retention or a delete can take the library down to the one
    // value already selected, and greying the control there would leave the
    // selection with no way to undo it from the control that made it.
    facet.select.disabled =
      facet.select.options.length < 3 && facet.select.value === ANY;
  }
}

function filtersActive(): boolean {
  return (
    els.champion.value.trim() !== "" ||
    els.outcome.value !== ANY ||
    els.pinned.checked ||
    facets.some((facet) => facet.select.value !== ANY)
  );
}

// Sort is deliberately left alone: it is not a filter, it hides nothing,
// and resetting it would throw away an order the user chose.
function clearFilters() {
  els.champion.value = "";
  els.outcome.value = ANY;
  els.pinned.checked = false;
  for (const facet of facets) facet.select.value = ANY;
  render();
}


export async function refreshLibrary() {
  try {
    allRecordings = await call<RecordingRow[]>("list_recordings");
    refreshFacets();
    render();
  } catch (err) {
    toast(`Failed to list recordings: ${err}`, "error");
  }
}

export async function refreshDiskUsage() {
  try {
    usage = await call<DiskUsage>("get_disk_usage");
    renderStats(visibleRows());
  } catch (err) {
    toast(`Failed to load disk usage: ${err}`, "error");
  }
}

async function rescanRecordings() {
  try {
    els.rescan.disabled = true;
    const report = await call<ReconcileReport>("rescan_recordings");
    toast(
      `Rescan complete — removed ${report.orphans_removed} orphan row(s), imported ${report.imported} untracked file(s).`,
    );
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    toast(`Failed to rescan: ${err}`, "error");
  } finally {
    els.rescan.disabled = false;
  }
}

function visibleRows(): RecordingRow[] {
  const championFilter = els.champion.value.trim().toLowerCase();
  const outcome = els.outcome.value;
  const pinnedOnly = els.pinned.checked;

  const rows = allRecordings.filter((row) => {
    if (championFilter && !vodTitle(row).toLowerCase().includes(championFilter)) {
      return false;
    }
    if (outcome === "wins" && row.win !== true) return false;
    if (outcome === "losses" && row.win !== false) return false;
    if (pinnedOnly && !row.pinned) return false;
    for (const facet of facets) {
      const selected = facet.select.value;
      if (selected === ANY) continue;
      const key = facet.key(row);
      if (selected === NONE) {
        if (key !== null) return false;
      } else if (key === null || VALUE_PREFIX + key !== selected) {
        return false;
      }
    }
    return true;
  });

  return rows.sort((a, b) => {
    switch (els.sort.value) {
      case "oldest":
        return a.started_at - b.started_at;
      case "longest":
        return (b.duration_s ?? 0) - (a.duration_s ?? 0);
      case "champion":
        return (a.champion ?? "").localeCompare(b.champion ?? "");
      default:
        return b.started_at - a.started_at;
    }
  });
}

function render() {
  if (currentView() !== "library") {
    pendingRender = true;
    return;
  }
  pendingRender = false;

  const rows = visibleRows();
  renderStats(rows);

  disarmDelete();
  if (rows.length === 0) {
    // "Nothing recorded yet" and "everything is filtered out" are different
    // problems with different next steps, and the first message was the
    // only one there used to be — which read as data loss the moment a
    // filter matched nothing.
    const filtered = filtersActive() && allRecordings.length > 0;
    if (filtered) {
      els.noMatchesHint.textContent =
        allRecordings.length === 1
          ? "The one recording in the library does not match."
          : `${allRecordings.length} recordings in the library \u2014 none of them match.`;
    }
    els.empty.hidden = filtered;
    els.noMatches.hidden = !filtered;
    els.grid.hidden = true;
    els.grid.innerHTML = "";
    return;
  }

  els.empty.hidden = true;
  els.noMatches.hidden = true;
  els.grid.hidden = false;
  els.grid.innerHTML = rows.map(card).join("");
  void fillInArt(rows);
}

/** How many rows to resolve art for before painting what has arrived. */
const ART_CHUNK = 8;

/**
 * Art, filled in after the list is already on screen.
 *
 * Deliberately a second pass. The first sighting of any icon is a CDN round
 * trip, so blocking the list on it would trade a library that renders
 * instantly for one that renders once the network says so — and the whole
 * thing has to work with no network at all, where the answer is "no art"
 * and every row is already correct without it.
 */
async function fillInArt(rows: RecordingRow[]) {
  // Paint what is already cached before awaiting anything. `render` rebuilds
  // the list with `innerHTML`, which throws away every image painted into
  // it, and that happens on every filter change, sort, refresh and
  // `library-changed`. Skipping this when the cache was warm — which is
  // what returning early on "nothing new to fetch" did — left the rows
  // blank from the second render onwards.
  paintAll(rows);

  // A chunk at a time, top down, so the rows a person is actually looking
  // at fill in first. The total wait is the same; what changes is that it
  // stops being one wait for everything. On a warm cache every chunk
  // resolves without a request and this is indistinguishable from the
  // single pass it replaced.
  for (let i = 0; i < rows.length; i += ART_CHUNK) {
    const chunk = rows.slice(i, i + ART_CHUNK);
    if (await loadIcons(chunk)) paintAll(chunk);
  }
}

function paintAll(rows: RecordingRow[]) {
  for (const row of rows) {
    // The list may have been re-rendered while the requests were in flight.
    const el = els.grid.querySelector<HTMLElement>(`.vod-row[data-id="${row.id}"]`);
    if (el) paintArt(el, row);
  }
}

/** Puts the resolved art into one row's already-rendered slots. */
function paintArt(el: HTMLElement, row: RecordingRow) {
  const portrait = championIcon(row.champion);
  const portraitSlot = el.querySelector<HTMLElement>(".vod-portrait");
  if (portraitSlot && portrait) {
    portraitSlot.innerHTML = `<img src="${escapeAttr(portrait)}" alt="" loading="lazy" />`;
  }

  for (const slot of el.querySelectorAll<HTMLElement>("[data-icon]")) {
    const { icon, key } = slot.dataset;
    const src =
      icon === "champion"
        ? championIcon(String(key))
        : icon === "item"
          ? itemIcon(Number(key))
          : icon === "spell"
            ? spellIcon(String(key))
            : icon === "spell-id"
              ? spellIconById(Number(key))
              : icon === "rune"
                ? runeIcon(Number(key))
                : null;
    if (src) slot.innerHTML = `<img src="${escapeAttr(src)}" alt="" loading="lazy" />`;
  }
}

// Stats are computed over the *filtered* rows so they track the filters,
// with a sub-label naming the total whenever a filter is narrowing things
// — otherwise "100%" under Wins-only reads as a perfect record.
function renderStats(rows: RecordingRow[]) {
  els.statGames.textContent = String(rows.length);
  els.statGamesSub.textContent =
    rows.length === allRecordings.length ? "" : `of ${allRecordings.length}`;

  // `win` is null for anything reconcile imported — it only knows the path
  // and size. Treating that as a loss would quietly understate the rate.
  const decided = rows.filter((r) => r.win !== null);
  const wins = decided.filter((r) => r.win === true).length;
  els.statWinrate.textContent =
    decided.length === 0 ? "—" : `${Math.round((wins / decided.length) * 100)}%`;
  els.statWinrateSub.textContent =
    decided.length === 0 ? "no results yet" : `${wins}W ${decided.length - wins}L`;

  // Same story for `duration_s`.
  const timed = rows.filter((r) => r.duration_s !== null);
  els.statPlaytime.textContent = formatSpan(
    timed.reduce((total, r) => total + (r.duration_s ?? 0), 0),
  );
  els.statPlaytimeSub.textContent =
    timed.length === rows.length ? "" : `${rows.length - timed.length} unknown`;

  // `size_bytes` is NOT NULL DEFAULT 0, so no null handling here.
  els.statDisk.textContent = formatBytes(
    rows.reduce((total, r) => total + r.size_bytes, 0),
  );
  els.statDiskSub.textContent = usage
    ? `${formatBytes(usage.free_bytes)} free`
    : "";
}

/** "8.3 /min", or nothing when either half is missing. */
function csPerMinute(row: RecordingRow): string | null {
  if (row.cs === null || row.duration_s === null || row.duration_s <= 0) return null;
  return `${(row.cs / (row.duration_s / 60)).toFixed(1)} /min`;
}

/**
 * The spells, runes and items the game ended on.
 *
 * Empty slots are rendered, not skipped. A build with four items is a
 * different thing from a game with no scoreboard, and a row that shrank
 * to fit would say neither — the boxes are the shape of the information.
 *
 * Every slot starts blank and is filled by `paintArt` once the CDN answers,
 * so this renders identically offline, just without pictures. The `title`
 * carries what each one is, which is the whole of what a row with no art
 * can tell you.
 */
function loadout(row: RecordingRow): string {
  const board = parseScoreboard(row.scoreboard_json);
  const us = board?.players.find((p) => p.is_us) ?? null;
  const runes = board?.our_runes ?? null;

  const slot = (kind: string, key: string | number, label: string) =>
    `<span class="vod-slot" data-icon="${kind}" data-key="${escapeAttr(String(key))}"
           title="${escapeAttr(label)}"></span>`;
  const empty = `<span class="vod-slot vod-slot-empty"></span>`;

  // Names when the scoreboard was captured live, ids when it was rebuilt
  // from match history. Both find the art; neither is converted into the
  // other, because that would need the CDN in a path that only talks to
  // the League client.
  const spells = us
    ? us.spells.length > 0
      ? us.spells.slice(0, 2).map((s) => slot("spell", s, s))
      : (us.spell_ids ?? []).slice(0, 2).map((id) => slot("spell-id", id, `Spell ${id}`))
    : [];
  while (spells.length < 2) spells.push(empty);

  const perks = runes
    ? [
        slot("rune", runes.keystone_id, runes.keystone || "Keystone"),
        slot("rune", runes.secondary_tree_id, "Secondary tree"),
      ]
    : [empty, empty];

  // Six plus the trinket, which is what an inventory holds.
  const items = (us?.items ?? []).slice(0, 7).map((id) => slot("item", id, `Item ${id}`));
  while (items.length < 7) items.push(empty);

  return `
      <!-- Order is load-bearing: the grid fills by column, so these four
           land as spell 1, spell 2 | keystone, secondary tree. -->
      <span class="vod-perks" aria-hidden="true">${spells.join("")}${perks.join("")}</span>
      <span class="vod-items" aria-hidden="true">${items.join("")}</span>`;
}

/** Five a side, which is what the block draws whatever the board holds. */
const TEAM_SIZE = 5;

/**
 * Both team compositions, ours on the top line.
 *
 * The ten champions are the fastest way to recognise a game that the
 * champion column cannot give you — "the one against the Yasuo" is how
 * people actually remember a match — and they are already in
 * `scoreboard_json`, so this costs no column, no query and no migration.
 *
 * **Which line is ours is a claim, and it is only made when the capture
 * can back it.** `our_team` is absent whenever the poller never matched us
 * in `allPlayers`, and the rule there is already that the row says nothing
 * about which half is which. So the halves still draw — grouped, in the
 * order the game listed them — but nothing labels either one, because a
 * top line silently meaning "yours" would be a guess in a slot read as
 * fact.
 *
 * Empty slots are drawn rather than skipped, exactly as in `loadout`: a
 * recording with no scoreboard keeps the same shape as one that has it.
 */
function teams(row: RecordingRow): string {
  const board = parseScoreboard(row.scoreboard_json);
  const players = board?.players ?? [];

  const known = board?.our_team ?? null;
  const lead = known ?? players[0]?.team ?? null;
  let ours = players.filter((p) => p.team === lead);
  let enemy = players.filter((p) => p.team !== lead);

  // A board that lands everybody on one side is not one this app has
  // written — both halves of `team` come from the same Rust struct — but
  // splitting it where the game would still draws two fives rather than a
  // ten and a gap.
  if (players.length > 0 && (ours.length === 0 || enemy.length === 0)) {
    ours = players.slice(0, TEAM_SIZE);
    enemy = players.slice(TEAM_SIZE);
  }

  const line = (side: ScoreboardPlayer[], label: string | null) => {
    const slots = side
      .slice(0, TEAM_SIZE)
      .map(
        (p) =>
          `<span class="vod-slot" data-icon="champion" data-key="${escapeAttr(p.champion)}"
                 title="${escapeAttr(label === null ? p.champion : `${label}: ${p.champion}`)}"></span>`,
      );
    while (slots.length < TEAM_SIZE) {
      slots.push(`<span class="vod-slot vod-slot-empty"></span>`);
    }
    return slots.join("");
  };

  // `aria-hidden` for the same reason the items and perks are: ten more
  // names in the row's label would bury the champion, the result and the
  // date that make the list navigable in the first place. The `title` on
  // each square is what a pointer gets, as everywhere else on the row.
  const oursLabel = known === null ? null : "Your team";
  const enemyLabel = known === null ? null : "Enemy team";
  return `
      <span class="vod-teams" aria-hidden="true">${line(ours, oursLabel)}${line(enemy, enemyLabel)}</span>`;
}

function outcomeAttr(win: boolean | null): string {
  if (win === null) return "unknown";
  return win ? "win" : "loss";
}

function card(row: RecordingRow): string {
  // `vodTitle` can fall back to the filename, which is user-controlled:
  // reconcile imports whatever video files it finds. Hence escapeAttr on
  // every attribute and escapeHtml on every text node below.
  const title = vodTitle(row);
  const kda = formatKda(row.kda_k, row.kda_d, row.kda_a);
  const queue = queueOrModeLabel(row);
  const length = row.duration_s === null ? null : formatClock(row.duration_s);

  const ratio = kdaRatio(row.kda_k, row.kda_d, row.kda_a);
  const patch = patchLabel(row.patch);

  // Said once, in the left block, and shown once, as the leading accent.
  // The word is what makes the row readable without colour — green and red
  // are exactly the pair a red-green deficiency cannot separate — and it
  // costs no column because that block is already a run of lines.
  //
  // Absent when the result is unknown, which is unambiguous rather than a
  // gap: a word is on every decided row, so no word means undecided.
  const outcomeWord = row.win === null ? null : row.win ? "Win" : "Loss";
  const outcome = outcomeWord ?? "Result unknown";

  // Every one of these can be absent, and a row that hides the slot when
  // it is reads as a different shape per recording — which is exactly what
  // makes a list scannable or not. `—` keeps the lines where they are and
  // says so out loud.
  const cell = (value: string | null, title?: string | null) =>
    value === null
      ? `<span class="vod-missing">—</span>`
      : `<span${title ? ` title="${escapeAttr(title)}"` : ""}>${escapeHtml(value)}</span>`;

  // Deaths in their own colour, which is the one number on a row people
  // look for first. Built from the three integers rather than by splitting
  // `formatKda`'s string, so nothing here has to parse its own output —
  // but `formatKda` still owns the all-three-or-nothing rule.
  const kdaMarkup =
    kda === null
      ? `<span class="vod-missing">—</span>`
      : `${row.kda_k} <span class="vod-slash">/</span>` +
        ` <span class="vod-deaths">${row.kda_d}</span>` +
        ` <span class="vod-slash">/</span> ${row.kda_a}`;

  return `
    <article class="vod-row" role="listitem" tabindex="0"
             data-id="${row.id}" data-outcome="${outcomeAttr(row.win)}"
             aria-label="${escapeAttr(`${title} — ${outcome}`)}">
      <span class="vod-meta">
        <span class="vod-queue">${cell(queue)}</span>
        <time datetime="${new Date(row.started_at).toISOString()}"
              class="vod-sub" title="${escapeAttr(formatDateTime(row.started_at))}"
        >${escapeHtml(formatRelative(row.started_at))}</time>
        <span class="vod-sub">${patch === null ? "&nbsp;" : `Patch ${escapeHtml(patch)}`}</span>
        <span class="vod-sub">${cell(length)}${
          outcomeWord ? ` · <span class="vod-outcome">${outcomeWord}</span>` : ""
        }</span>
      </span>

      <span class="vod-portrait" aria-hidden="true"></span>

      <span class="vod-cell">
        <span class="vod-champ" title="${escapeAttr(title)}">${escapeHtml(title)}</span>
        <span class="vod-sub">${
          row.role === null ? `<span class="vod-missing">Unknown</span>` : escapeHtml(row.role)
        }</span>
      </span>

      <span class="vod-cell">
        <span class="vod-value vod-kda">${kdaMarkup}</span>
        <span class="vod-sub">${ratio ? escapeHtml(ratio) : "&nbsp;"}</span>
      </span>

      <span class="vod-cell">
        <span class="vod-value">${cell(row.cs === null ? null : `${row.cs} cs`)}</span>
        <span class="vod-sub">${csPerMinute(row) ?? "&nbsp;"}</span>
      </span>

      ${loadout(row)}

      ${teams(row)}

      <span class="vod-slack" aria-hidden="true"></span>

      <span class="vod-sub vod-size">${formatBytes(row.size_bytes)}</span>

      <span class="vod-actions">
        <button class="icon-btn pin-btn${row.pinned ? " pinned" : ""}"
                type="button" data-pin="${row.id}"
                aria-pressed="${row.pinned}"
                title="${row.pinned ? "Unpin" : "Pin (exempt from disk retention)"}"
        >📌</button>
        <button class="icon-btn danger" type="button" data-delete="${row.id}"
                aria-label="Delete recording" title="Delete recording"
        >🗑</button>
      </span>
    </article>`;
}

function onGridClick(e: MouseEvent) {
  const target = e.target as HTMLElement;

  const pinBtn = target.closest<HTMLButtonElement>("button[data-pin]");
  if (pinBtn) {
    const row = findRow(Number(pinBtn.dataset.pin));
    if (row) togglePin(row);
    return;
  }

  const deleteBtn = target.closest<HTMLButtonElement>("button[data-delete]");
  if (deleteBtn) {
    onDeleteClick(deleteBtn, Number(deleteBtn.dataset.delete));
    return;
  }

  // Anything else inside the actions group must not fall through to
  // opening the VOD.
  if (target.closest(".vod-actions")) return;

  const card = target.closest<HTMLElement>(".vod-row");
  if (!card) return;
  const row = findRow(Number(card.dataset.id));
  if (row) openReview(row);
}

async function togglePin(row: RecordingRow) {
  try {
    await call("set_pinned", { recordingId: row.id, pinned: !row.pinned });
    await refreshLibrary();
  } catch (err) {
    toast(`Failed to update pin: ${err}`, "error");
  }
}

// Two-step confirm in place of a dialog: the first click arms the button,
// the second deletes. Cheaper than a modal and it keeps the destructive
// action next to the thing it destroys.
function onDeleteClick(button: HTMLButtonElement, id: number) {
  if (armedForDelete === id) {
    disarmDelete();
    deleteRecording(id);
    return;
  }
  disarmDelete();
  armedForDelete = id;
  button.classList.add("armed");
  button.textContent = "Delete?";
  window.clearTimeout(armTimer);
  armTimer = window.setTimeout(disarmDelete, 4000);
}

function disarmDelete() {
  window.clearTimeout(armTimer);
  if (armedForDelete === null) return;
  const button = els.grid.querySelector<HTMLButtonElement>(
    `button[data-delete="${armedForDelete}"]`,
  );
  if (button) {
    button.classList.remove("armed");
    button.textContent = "🗑";
  }
  armedForDelete = null;
}

async function deleteRecording(id: number) {
  const row = findRow(id);
  try {
    await call("delete_recording", { recordingId: id });
    toast(`Deleted ${row ? vodTitle(row) : "recording"}.`);
    await Promise.all([refreshLibrary(), refreshDiskUsage()]);
  } catch (err) {
    toast(`Failed to delete: ${err}`, "error");
  }
}
