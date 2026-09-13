/** Health dashboard: state machine, recorder, LCU, disk, paths. */
import type { Panel, PanelContext } from "../main";
import { tryCall } from "../ipc";
import { bytes, card, duration, escapeHtml, kv, output, panelHead, timestamp, toast } from "../ui";
import { GAME_STATES, type DevHealth, type LcuStatus } from "../types";

let root: HTMLElement | null = null;
let lastCtx: PanelContext | null = null;
let lcu: LcuStatus | null = null;
let lcuError: string | null = null;

/** `DEVELOPMENT.md` §3.4's diagram, with the live state lit. */
function stateDiagram(current: string): string {
  return `<div class="states">${GAME_STATES.map(
    (s, i) =>
      `${i > 0 ? `<span class="state-arrow">→</span>` : ""}
       <span class="state-node${s === current ? " current" : ""}">${s}</span>`,
  ).join("")}</div>`;
}

function sessionCard(health: DevHealth): string {
  if (!health.session) {
    return card(
      "Live session",
      `<p class="hint">No recording session open. Markers and samples are only collected between
       <code>Recorder::start</code> and <code>Recorder::stop</code>.</p>`,
    );
  }
  const s = health.session;
  return card(
    "Live session",
    kv([
      ["Markers", String(s.marker_count)],
      ["Samples", String(s.sample_count)],
      ["Elapsed", duration(s.elapsed_s)],
      [
        "Time alignment",
        s.alignment_offset_s === null
          ? "not aligned yet — no snapshot has arrived"
          : `${s.alignment_offset_s.toFixed(2)}s offset`,
      ],
      ["Started", timestamp(s.started_at_millis)],
    ]) +
      (s.recent_markers.length
        ? `<p class="hint" style="margin-top:.6rem">Latest: ${escapeHtml(
            s.recent_markers
              .slice(0, 6)
              .map((m) => `${m.kind}@${m.game_time_s.toFixed(0)}s`)
              .join(", "),
          )}</p>`
        : ""),
  );
}

function lcuCard(): string {
  if (lcuError) return card("League Client", output(lcuError, true));
  if (!lcu) return card("League Client", `<p class="hint">Not checked yet.</p>`);
  if (lcu.error) return card("League Client", output(lcu.error, true));
  if (!lcu.connected) {
    return card(
      "League Client",
      `<p class="hint">Not running — no lockfile found. Set
       <code>NINJA_RECORDER_LOCKFILE_PATH</code>, or use the Simulate panel to drive the
       state machine without a client.</p>`,
    );
  }
  return card("League Client", kv([["Summoner", lcu.summoner], ["Phase", lcu.phase]]));
}

function paths(ctx: PanelContext): string {
  const env = ctx.env;
  if (!env) return "";
  const reveal = (which: string, label: string) =>
    `<button type="button" class="tiny ghost" data-reveal="${which}">${label}</button>`;

  return card(
    "Environment",
    kv([
      ["Version", `${env.app_version} (${env.build_profile})`],
      ["Platform", `${env.os}/${env.arch} · Tauri ${env.tauri_version}`],
      ["Recorder", env.recorder_backend],
      ["Identifier", env.identifier],
      ["Database", env.db_path],
      ["Recordings", env.recordings_dir],
      ["Fixtures", env.fixtures_dir],
      ["Repo fixtures", env.repo_fixtures_dir],
      ["fixtures/sample.mp4", env.sample_mp4_present ? "present" : "not checked in"],
      ["Lockfile override", env.lockfile_override],
    ]) +
      `<div class="row" style="margin-top:.7rem">
        ${reveal("recordings", "Reveal recordings")}
        ${reveal("app_data", "Reveal app data")}
        ${reveal("fixtures", "Reveal fixtures")}
      </div>` +
      (env.sample_mp4_present
        ? ""
        : `<p class="hint-block">Without <code>fixtures/sample.mp4</code> the stub recorder and the
           seeder both write placeholder files, which no demuxer will open — seeded recordings will
           appear in the library but will not play. Drop a short real clip there to test the review
           player. It is gitignored except for the <code>!fixtures/*.mp4</code> whitelist.</p>`),
  );
}

function draw(health: DevHealth | null, ctx: PanelContext) {
  if (!root) return;
  root.innerHTML =
    panelHead(
      "Overview",
      "Live state of every subsystem. Polled once a second while the top bar's Live toggle is on.",
    ) +
    (health
      ? card("Game state machine", stateDiagram(health.supervisor.state)) +
        `<div class="card-grid">
          ${card(
            "Recorder",
            kv([
              ["Backend", ctx.env?.recorder_backend ?? "?"],
              ["Capturing", health.is_recording ? "yes" : "no"],
              ["Free space", bytes(health.free_bytes)],
            ]),
          )}
          ${card(
            "Library",
            kv([
              ["Recordings", String(health.counts.recordings)],
              ["Markers", String(health.counts.markers)],
              ["Samples", String(health.counts.samples)],
              ["Total size", bytes(health.total_bytes)],
            ]),
          )}
          ${card(
            "Retention policy",
            kv([
              [
                "Max total",
                health.policy.max_total_bytes === null
                  ? "unbounded"
                  : bytes(health.policy.max_total_bytes),
              ],
              [
                "Max age",
                health.policy.max_age_days === null
                  ? "unbounded"
                  : `${health.policy.max_age_days} days`,
              ],
            ]),
          )}
          ${sessionCard(health)}
          ${lcuCard()}
        </div>` +
        (health.supervisor.last_finalized
          ? card(
              "Last finalized recording",
              kv([
                [
                  "DB row",
                  health.supervisor.last_finalized.recording_id === null
                    ? "WRITE FAILED — kept in memory only"
                    : `id ${health.supervisor.last_finalized.recording_id}`,
                ],
                ["Path", health.supervisor.last_finalized.path],
                ["Markers", String(health.supervisor.last_finalized.markers.length)],
              ]),
            )
          : "")
      : `<p class="hint">Waiting for the first health poll…</p>`) +
    paths(ctx);

  root.querySelector("[data-reveal]")?.closest(".row")?.addEventListener("click", async (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>("[data-reveal]");
    if (!btn) return;
    const result = await tryCall("dev_open_data_dir", { which: btn.dataset.reveal });
    if (!result.ok) toast(result.error, "err");
  });
}

export const overviewPanel: Panel = {
  id: "overview",
  title: "Overview",
  icon: "◉",
  group: "Status",

  async mount(el, ctx) {
    root = el;
    lastCtx = ctx;
    draw(null, ctx);
    // The LCU check is a separate, slower call — it does lockfile
    // discovery plus two HTTP round trips — so it stays out of the 1 Hz
    // health poll and refreshes only on mount.
    const result = await tryCall<LcuStatus>("lcu_status");
    lcu = result.ok ? result.value : null;
    lcuError = result.ok ? null : result.error;
    const health = await tryCall<DevHealth>("dev_health");
    draw(health.ok ? health.value : null, ctx);
  },

  onHealth(health) {
    if (lastCtx) draw(health, lastCtx);
  },

  unmount() {
    root = null;
    lastCtx = null;
  },
};
