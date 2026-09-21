/**
 * The scripted input the Simulate panel pushes through the real pipeline.
 *
 * `state_machine::machine`'s pure transition function is well covered by unit
 * tests. `state_machine::supervisor` - the part that spawns watchers, starts
 * the recorder, and writes the finalize row - has almost none, and has never
 * run against a real client. Everything here is synthetic input for that real
 * code path, which makes the lists themselves worth keeping honest.
 */

export interface StateEvent {
  label: string;
  event: Record<string, unknown>;
  note: string;
}

/** The transitions that matter, in the order a real game hits them. */
export const EVENTS: StateEvent[] = [
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

/**
 * The default timeline a replay fires, in game seconds.
 *
 * Two events share a timestamp on purpose: `FirstBlood` and the
 * `ChampionKill` that caused it both land at 92s, which is what the real Live
 * Client Data does and what the marker tracker's cross-poll de-duplication has
 * to survive.
 */
export const DEFAULT_REPLAY_EVENTS = [
  { event_time: 92, event_name: "FirstBlood", Recipient: "Ahri" },
  {
    event_time: 92,
    event_name: "ChampionKill",
    KillerName: "Ahri",
    VictimName: "Sylas",
    Assisters: [],
  },
  {
    event_time: 260,
    event_name: "ChampionKill",
    KillerName: "Viego",
    VictimName: "Ahri",
    Assisters: [],
  },
  { event_time: 420, event_name: "DragonKill", KillerName: "Ahri", DragonType: "Infernal" },
  {
    event_time: 430,
    event_name: "ChampionKill",
    KillerName: "Ahri",
    VictimName: "Kai'Sa",
    Assisters: [],
  },
  {
    event_time: 436,
    event_name: "ChampionKill",
    KillerName: "Ahri",
    VictimName: "Nami",
    Assisters: [],
  },
  { event_time: 441, event_name: "Ace", Acer: "Ahri", AcingTeam: "ORDER" },
  {
    event_time: 700,
    event_name: "TurretKilled",
    KillerName: "Ahri",
    TurretKilled: "Turret_T2_C_05_A",
  },
  { event_time: 980, event_name: "BaronKill", KillerName: "Ahri" },
];
