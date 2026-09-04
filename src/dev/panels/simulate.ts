/**
 * Driving the state machine and the marker pipeline with no League
 * running.
 *
 * `state_machine::machine`'s pure transition function is well covered by
 * unit tests. `state_machine::supervisor` — the part that spawns watchers,
 * starts the recorder, and writes the finalize row — has almost none, and
 * has never run against a real client. Everything on this page pushes
 * synthetic input through that real code path.
 */
import type { Panel } from "../main";
import { tryCall } from "../ipc";
import {
  card,
  duration,
  escapeHtml,
  kv,
  output,
  panelHead,
  toast,
} from "../ui";
import {
  GAME_STATES,
  type DevSessionView,
  type DispatchReport,
  type FixturesState,
  type InjectReport,
  type ReplayStatus,
} from "../types";

/** The transitions that matter, in the order a real game hits them. */
const EVENTS: Array<{ label: string; event: Record<string, unknown>; note: string }> = [
  {
    label: "Client opened",
    event: { kind: "lockfile_present" },
    note: "Idle → ClientRunning. Spawns a gameflow watcher against a fabricated lockfile, which will fail to connect and retry harmlessly.",
  },
  {
    label: "Phase: ChampSelect",
    event: { kind: "gameflow_phase", phase: "ChampSelect" },
    note: "Not a recording trigger — included so a dodge can be simulated.",
  },
  {
    label: "Phase: InProgress",
    event: { kind: "gameflow_phase", phase: "InProgress" },
    note: "ClientRunning → WaitingForGame.",
  },
  {
    label: "Phase: Reconnect",
    event: { kind: "gameflow_phase", phase: "Reconnect" },
    note: "Treated identically to InProgress — the machine has no memory of how it got here.",
  },
  {
    label: "Live Client up",
    event: { kind: "live_client_up" },
    note: "WaitingForGame → Recording. Really calls Recorder::start.",
  },
  {
    label: "Live Client down",
    event: { kind: "live_client_down" },
    note: "Game crashed mid-match. Recording → Finalizing, preserving whatever was captured.",
  },
  {
    label: "Phase: EndOfGame",
    event: { kind: "gameflow_phase", phase: "EndOfGame" },
    note: "Recording → Finalizing. Really calls Recorder::stop and writes the DB row.",
  },
  {
    label: "Client closed",
    event: { kind: "lockfile_absent" },
    note: "Client crash or quit, from any state.",
  },
];

const DEFAULT_REPLAY_EVENTS = [
  { event_time: 92, event_name: "FirstBlood", Recipient: "Ahri" },
  { event_time: 92, event_name: "ChampionKill", KillerName: "Ahri", VictimName: "Sylas", Assisters: [] },
  { event_time: 260, event_name: "ChampionKill", KillerName: "Viego", VictimName: "Ahri", Assisters: [] },
  { event_time: 420, event_name: "DragonKill", KillerName: "Ahri", DragonType: "Infernal" },
  { event_time: 430, event_name: "ChampionKill", KillerName: "Ahri", VictimName: "Kai'Sa", Assisters: [] },
  { event_time: 436, event_name: "ChampionKill", KillerName: "Ahri", VictimName: "Nami", Assisters: [] },
  { event_time: 441, event_name: "Ace", Acer: "Ahri", AcingTeam: "ORDER" },
  { event_time: 700, event_name: "TurretKilled", KillerName: "Ahri", TurretKilled: "Turret_T2_C_05_A" },
  { event_time: 980, event_name: "BaronKill", KillerName: "Ahri" },
];

let root: HTMLElement | null = null;
let state = "Idle";
let session: DevSessionView | null = null;
let lastDispatch: { value: unknown; error: boolean } | null = null;
let lastInject: { value: unknown; error: boolean } | null = null;
let replay: ReplayStatus | null = null;
let fixtures: FixturesState | null = null;
let snapshotJson = "";
let replayTimer: number | null = null;

function stateStrip(): string {
  return `<div class="states">${GAME_STATES.map(
    (s, i) =>
      `${i > 0 ? `<span class="state-arrow">→</span>` : ""}
       <span class="state-node${s === state ? " current" : ""}">${s}</span>`,
  ).join("")}</div>`;
}

function sessionSummary(): string {
  if (!session) {
    return `<p class="hint">No session open — nothing is collecting markers right now.</p>`;
  }
  return (
    kv([
      ["Markers", String(session.marker_count)],
      ["Samples", String(session.sample_count)],
      ["Elapsed", duration(session.elapsed_s)],
      [
        "Alignment",
        session.alignment_offset_s === null
          ? "not aligned — no snapshot yet"
          : `${session.alignment_offset_s.toFixed(2)}s`,
      ],
    ]) +
    (session.recent_markers.length
      ? `<p class="hint" style="margin-top:.5rem">${escapeHtml(
          session.recent_markers
            .slice(0, 8)
            .map((m) => `${m.kind}@${m.game_time_s.toFixed(0)}s`)
            .join("  ·  "),
        )}</p>`
      : "")
  );
}

function draw() {
  if (!root) return;
  const liveClientFixtures = (fixtures?.entries ?? []).filter((f) =>
    f.group.includes("live-client") || f.name.includes("allgamedata"),
  );

  root.innerHTML =
    panelHead(
      "Simulate",
      "Drive the state machine and the marker pipeline without League running.",
    ) +
    `<div class="warnbar warnbar-danger">
      These are not dry runs. Dispatching an event executes the supervisor's real actions —
      <code>Recorder::start</code> and <code>Recorder::stop</code> included — and a finalize writes
      a real row and a real file.
    </div>` +
    card("Current state", stateStrip() + `<div style="margin-top:.8rem">${sessionSummary()}</div>`) +
    card(
      "State events",
      `<div class="row">${EVENTS.map(
        (e, i) => `<button type="button" data-event="${i}">${escapeHtml(e.label)}</button>`,
      ).join("")}</div>
      <p class="hint-block">A full game is: Client opened → Phase: InProgress → Live Client up →
        (inject snapshots) → Phase: EndOfGame.</p>
      ${lastDispatch ? output(lastDispatch.value, lastDispatch.error) : ""}`,
    ) +
    card(
      "Inject a Live Client Data snapshot",
      `<div class="row" style="margin-bottom:.5rem">
        <select id="fixture-pick">
          <option value="">Load a fixture…</option>
          ${liveClientFixtures
            .map(
              (f) =>
                `<option value="${escapeHtml(f.path)}">${escapeHtml(f.source)} · ${escapeHtml(
                  f.name,
                )}</option>`,
            )
            .join("")}
        </select>
        <button type="button" class="ghost tiny" data-probe>Fetch from a running game</button>
      </div>
      <textarea id="snapshot" rows="10" spellcheck="false"
        placeholder="Paste an allgamedata payload, or load one above">${escapeHtml(snapshotJson)}</textarea>
      <div class="row" style="margin-top:.6rem">
        <button type="button" class="primary" data-inject>Inject</button>
        <span class="hint">Runs the real MarkerTracker and team_diff over this payload.</span>
      </div>
      ${lastInject ? output(lastInject.value, lastInject.error) : ""}`,
    ) +
    card(
      "Scripted replay",
      `<div class="field-grid">
        <label class="field"><span>Game length (s)</span>
          <input type="number" id="replay-duration" value="1200" /></label>
        <label class="field"><span>Speed multiplier</span>
          <input type="number" id="replay-speed" value="60" />
          <span class="hint">60× runs a 20-minute game in 20 seconds</span></label>
      </div>
      <label class="check" style="margin-top:.6rem">
        <input type="checkbox" id="replay-drive" checked />
        Drive the state machine too — start recording, then finalize into a real row and file
      </label>
      <div class="row" style="margin-top:.8rem">
        <button type="button" class="primary" data-replay-start ${
          replay?.running ? "disabled" : ""
        }>Start replay</button>
        <button type="button" class="danger" data-replay-stop ${
          replay?.running ? "" : "disabled"
        }>Stop</button>
        ${
          replay
            ? `<span class="hint num">${replay.game_time_s.toFixed(0)}s / ${replay.duration_s.toFixed(
                0,
              )}s · ${replay.ticks} ticks · ${replay.events_fired} events${
                replay.finished ? " · finished" : ""
              }</span>`
            : ""
        }
      </div>
      ${replay?.error ? output(replay.error, true) : ""}
      <p class="hint-block">Each tick rewrites game time and the event list on the base payload,
        then pushes it through the same <code>on_snapshot</code> the 1 Hz poller uses — so the
        marker tracker's cross-poll de-duplication is exercised too, not bypassed. The base payload
        is whatever is in the snapshot box above; load a fixture first.</p>`,
    ) +
    card(
      "League API probes",
      `<div class="row">
        <label class="field field-inline" style="flex:1 1 22rem">
          <span>LCU path</span>
          <input type="text" id="lcu-path" value="/lol-gameflow/v1/gameflow-phase" style="flex:1" />
        </label>
        <button type="button" data-lcu-get>GET</button>
      </div>
      <div class="row" style="margin-top:.5rem">
        <label class="field field-inline"><span>Game id</span>
          <input type="number" id="game-id" style="width:11rem" /></label>
        <button type="button" data-match-summary>fetch_match_summary</button>
        <span class="hint">Implemented and tested, but called from nowhere in the app — which is
          why every recording's champion, win, and KDA are NULL.</span>
      </div>
      <div id="probe-out"></div>`,
    );
}

/** Null once the panel is unmounted — every caller runs after an await,
 *  so the user may have navigated away in the meantime. */
function textarea(): HTMLTextAreaElement | null {
  return root?.querySelector<HTMLTextAreaElement>("#snapshot") ?? null;
}

async function refreshSession() {
  const [status, s] = await Promise.all([
    tryCall<{ state: string }>("game_state_status"),
    tryCall<DevSessionView | null>("dev_session_snapshot"),
  ]);
  if (status.ok) state = status.value.state;
  session = s.ok ? s.value : null;
}

function pollReplay(on: boolean) {
  if (replayTimer !== null) {
    clearInterval(replayTimer);
    replayTimer = null;
  }
  if (!on) return;
  replayTimer = window.setInterval(async () => {
    const result = await tryCall<ReplayStatus>("dev_replay_status");
    if (!result.ok || !root) return;
    replay = result.value;
    await refreshSession();
    if (!root) return;
    draw();
    if (!replay.running) {
      pollReplay(false);
      if (replay.finished) toast("Replay finished", "ok");
    }
  }, 500);
}

export const simulatePanel: Panel = {
  id: "simulate",
  title: "Simulate",
  icon: "▶",
  group: "Status",

  async mount(el, context) {
    root = el;
    lastDispatch = null;
    lastInject = null;

    // A fixture handed over from the Fixtures panel.
    const handoff = context.payload;
    if (typeof handoff === "string") snapshotJson = handoff;

    const [f, r] = await Promise.all([
      tryCall<FixturesState>("dev_fixtures_state"),
      tryCall<ReplayStatus>("dev_replay_status"),
    ]);
    fixtures = f.ok ? f.value : null;
    replay = r.ok ? r.value : null;
    await refreshSession();
    draw();
    if (replay?.running) pollReplay(true);

    el.addEventListener("change", async (e) => {
      const picker = (e.target as HTMLElement).closest<HTMLSelectElement>("#fixture-pick");
      if (!picker || !picker.value) return;
      const result = await tryCall<string>("dev_fixture_read", { path: picker.value });
      const box = textarea();
      if (!box) return;
      if (result.ok) {
        snapshotJson = result.value;
        box.value = result.value;
        toast("Fixture loaded", "ok");
      } else {
        toast(result.error, "err");
      }
    });

    el.addEventListener("click", async (e) => {
      const target = e.target as HTMLElement;

      const eventBtn = target.closest<HTMLElement>("[data-event]");
      if (eventBtn) {
        const spec = EVENTS[Number(eventBtn.dataset.event)];
        const result = await tryCall<DispatchReport>("dev_dispatch_state_event", {
          event: spec.event,
        });
        if (!root) return;
        if (result.ok) {
          const r = result.value;
          state = r.after.state;
          session = r.session;
          lastDispatch = {
            value:
              r.before.state === r.after.state
                ? `${r.before.state} → (no change). ${spec.note}`
                : `${r.before.state} → ${r.after.state}. ${spec.note}`,
            error: false,
          };
        } else {
          lastDispatch = { value: result.error, error: true };
        }
        draw();
        return;
      }

      if (target.closest("[data-probe]")) {
        const result = await tryCall<unknown>("dev_live_client_probe");
        const box = textarea();
        if (!box) return;
        if (result.ok) {
          snapshotJson = JSON.stringify(result.value, null, 2);
          box.value = snapshotJson;
          toast("Fetched live snapshot", "ok");
        } else {
          toast(result.error, "err");
        }
        return;
      }

      if (target.closest("[data-inject]")) {
        const box = textarea();
        if (!box) return;
        snapshotJson = box.value;
        let parsed: unknown;
        try {
          parsed = JSON.parse(snapshotJson);
        } catch (err) {
          lastInject = { value: `Not valid JSON: ${err}`, error: true };
          draw();
          return;
        }
        const result = await tryCall<InjectReport>("dev_inject_snapshot", { snapshot: parsed });
        if (!root) return;
        if (result.ok) {
          const r = result.value;
          session = r.session;
          lastInject = {
            value: r.accepted
              ? `Accepted in state ${r.state}: +${r.markers_added} marker(s), +${r.samples_added} sample(s).`
              : r.note,
            error: !r.accepted,
          };
        } else {
          lastInject = { value: result.error, error: true };
        }
        await refreshSession();
        draw();
        return;
      }

      if (target.closest("[data-replay-start]")) {
        const box = textarea();
        if (!box) return;
        snapshotJson = box.value;
        let base: unknown;
        try {
          base = JSON.parse(snapshotJson);
        } catch {
          toast("Load a base snapshot into the box above first", "err");
          return;
        }
        const num = (id: string, fallback: number) =>
          Number(root?.querySelector<HTMLInputElement>(id)?.value ?? fallback);
        const spec = {
          base_snapshot: base,
          duration_s: num("#replay-duration", 1200),
          speed: num("#replay-speed", 60),
          events: DEFAULT_REPLAY_EVENTS,
          drive_state_machine:
            root?.querySelector<HTMLInputElement>("#replay-drive")?.checked ?? true,
        };
        const result = await tryCall("dev_replay_start", { spec });
        if (result.ok) {
          toast("Replay started");
          pollReplay(true);
        } else {
          toast(result.error, "err");
        }
        return;
      }

      if (target.closest("[data-replay-stop]")) {
        await tryCall("dev_replay_stop");
        pollReplay(false);
        replay = null;
        await refreshSession();
        draw();
        return;
      }

      if (target.closest("[data-lcu-get]")) {
        const path = root?.querySelector<HTMLInputElement>("#lcu-path")?.value.trim() ?? "";
        const result = await tryCall<unknown>("dev_lcu_get", { path });
        const out = root?.querySelector<HTMLElement>("#probe-out");
        if (out) out.innerHTML = output(result.ok ? result.value : result.error, !result.ok);
        return;
      }

      if (target.closest("[data-match-summary]")) {
        const gameId = Number(root?.querySelector<HTMLInputElement>("#game-id")?.value);
        if (Number.isNaN(gameId)) {
          toast("Enter a game id", "err");
          return;
        }
        const result = await tryCall<unknown>("dev_fetch_match_summary", { gameId });
        const out = root?.querySelector<HTMLElement>("#probe-out");
        if (out) out.innerHTML = output(result.ok ? result.value : result.error, !result.ok);
      }
    });
  },

  onHealth(health) {
    // Keep the strip live without redrawing the page under the user's
    // cursor — a full redraw would blow away the snapshot textarea.
    if (health.supervisor.state !== state) {
      state = health.supervisor.state;
      session = health.session;
      const strip = root?.querySelector<HTMLElement>(".states");
      if (strip) strip.outerHTML = stateStrip();
    }
  },

  unmount() {
    pollReplay(false);
    root = null;
  },
};
