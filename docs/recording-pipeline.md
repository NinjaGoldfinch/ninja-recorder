# The recording pipeline

The core workflow: from "League isn't running" to "a tagged VOD is in the
library." This is the document to read before touching `state_machine/`,
`lcu/` or `live_client/`.

---

## 1. The happy path, end to end

```mermaid
sequenceDiagram
    autonumber
    participant U as Player
    participant C as League client
    participant G as Game process
    participant S as Supervisor
    participant R as Recorder
    participant D as SQLite
    participant UI as Library UI

    U->>C: Launch League
    Note over S: lockfile::watch polls every 2 s<br/>(backs off to 30 s while no client)
    C-->>S: lockfile appears (pid, port, password)
    S->>S: Idle → ClientRunning
    S->>C: gameflow::watch (WebSocket, polling fallback @ 1 s)

    U->>C: Start a game
    C-->>S: phase = InProgress
    S->>S: ClientRunning → WaitingForGame
    S->>G: live_client::poller::watch @ 1 Hz

    Note over G: loading screen — port 2999 not up yet
    G-->>S: first successful /allgamedata
    S->>S: WaitingForGame → Recording
    S->>R: start(RecordConfig)
    S->>S: record started_at + first gameTime → TimeAlignment

    loop every second until the game ends
        G-->>S: allgamedata snapshot
        S->>S: MarkerTracker → new markers (kill, death, dragon …)
        S->>S: team_diff → one advantage sample
    end

    C-->>S: phase = EndOfGame (or 2999 stops responding)
    S->>S: Recording → Finalizing
    S->>R: stop()
    R-->>S: finalized MP4 path
    S->>D: insert recording + markers + samples (one row set)
    S->>D: retention::enforce_now
    S-->>UI: emit "library-changed"
    UI->>D: list_recordings
    S->>S: Finalizing → ClientRunning
```

## 2. The state machine

`state_machine::machine::StateMachine::handle` is a pure function — no I/O, no
clock, no async — which is why the whole edge-case matrix below is covered by
unit tests that need neither League nor Windows.

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> ClientRunning: lockfile appears<br/>▸ StartGameflowWatch
    ClientRunning --> Idle: lockfile gone<br/>▸ StopGameflowWatch
    ClientRunning --> ClientRunning: lockfile changed (client restart)<br/>▸ Stop + StartGameflowWatch
    ClientRunning --> WaitingForGame: phase InProgress / Reconnect<br/>▸ StartLiveClientPoll
    WaitingForGame --> ClientRunning: phase left the game<br/>(dodge, FailedToLaunch)<br/>▸ StopLiveClientPoll
    WaitingForGame --> Idle: lockfile gone<br/>▸ Stop watch + poll
    WaitingForGame --> Recording: Live Client Data reachable<br/>▸ StartRecording
    Recording --> Finalizing: phase EndOfGame<br/>or Live Client Data gone<br/>▸ StopRecording
    Finalizing --> ClientRunning: FinalizeComplete
    Finalizing --> Idle: FinalizeComplete<br/>+ client vanished
```

**Actions, not side effects.** `handle` returns a `Vec<Action>`
(`StartGameflowWatch`, `StopLiveClientPoll`, `StartRecording`, …). The
supervisor is the only thing that executes them, so "what should happen" and
"how it happens" are testable apart from each other.

### Which signal drives which transition

| Signal | Source | Cadence |
|---|---|---|
| `LockfileChanged` | `lcu::lockfile::watch` | poll every 2 s, backing off to 30 s while absent |
| `GameflowPhase` | `lcu::gameflow::watch` | LCU WebSocket, falling back to 1 s polling |
| `LiveClientUp` / `LiveClientDown` | `live_client::poller::watch` | 1 Hz, exponential backoff to 10 s while down |
| `FinalizeComplete` | the supervisor itself, after `stop()` and teardown | once per game |

### The capture backend's warm window

Orthogonal to the transitions above, and driven off the resulting state rather
than off any `Action`: the supervisor calls `Recorder::prepare` on every state
except `Idle`, and `Recorder::release` on `Idle`. In practice that means the
Windows backend is warm for exactly as long as the League client is running,
because holding it from launch to exit is the largest single item on the
idle-RAM budget ([DEVELOPMENT.md §2.2](../DEVELOPMENT.md#22-the-recorder-trait)).

`prepare` is only a pre-warm — `start` brings the backend up itself if it has to
— so a client that goes straight into a game is safe, and `release` is a no-op
while a recording is in flight.

### Edge cases the pure tests cover

| Case | Behaviour |
|---|---|
| Game crashes mid-match | Live Client Data stops responding → `LiveClientDown` → finalize normally; footage up to the crash is kept |
| Client crashes mid-match | lockfile disappears → finalize, then `Idle` |
| Client crashes before the game loads | `WaitingForGame` → `Idle`, nothing recorded, nothing to finalize |
| Reconnect to a game in progress | Identical to a fresh start — the machine has no memory of *how* it reached `WaitingForGame`, so recording begins when 2999 answers (later than a from-the-start recording) |
| Practice Tool | Reports the same `InProgress`/`Reconnect` phases, so it is not special-cased |
| Dodge / cancelled champ select | `WaitingForGame` bounces back to `ClientRunning` without ever recording |
| Client restart during finalize | Handled regardless of ordering against `FinalizeComplete` |

Two cases are **not** verified, both because they need a live client on real
hardware: **spectator mode** (no phase beyond `InProgress`/`Reconnect` is
special-cased, so if spectating also reports `InProgress` it would be
recorded) and **machine sleep** (backoff and the lockfile watch should
recover after wake, untested). See
[windows-verification.md](windows-verification.md).

## 3. Events → markers

Each 1 Hz snapshot goes through `live_client::events`, which owns both halves
of the transform: discrete events become markers, and the same snapshot also
yields one row of the advantage time series.

```mermaid
flowchart TB
    SNAP["/liveclientdata/allgamedata"] --> ID["Match activePlayer against allPlayers"]
    SNAP --> EV["the events list"]
    ID --> CL
    EV --> DEDUP["Drop events already seen<br/><small>matched on EventID — the endpoint<br/>returns the whole list every poll</small>"]
    DEDUP --> CL["classify_event<br/><small>is this about us?</small>"]
    CL --> K["kill · death · assist"]
    CL --> O["dragon · baron · herald · turret"]
    CL --> M["ace · first_blood"]
    K --> AL
    O --> AL
    M --> AL
    AL["TimeAlignment::video_time_s<br/><small>game time → video time</small>"] --> MK["Marker rows"]
    ID --> TD["team_diff<br/><small>gold estimate, kills, CS</small>"]
    SNAP --> TD
    TD --> SM["Sample rows @ 1 Hz"]
```

**Marker kinds** (`MarkerKind::as_str`, matching `markers.kind` in SQLite):
`kill`, `death`, `assist`, `dragon`, `baron`, `herald`, `turret`, `ace`,
`first_blood`. `custom` exists in the schema for hand-added markers.

### Timestamp alignment

Recording starts on the loading screen — before game time 0 — so event times
and video times do not share an origin.

```
offset     = elapsed_since_record_start − gameTime   (sampled at one poll)
video_time = max(0, game_time + offset)
```

A positive offset is the normal loading-screen case (recording ran for a few
seconds before the clock started). A negative offset means recording started
*after* game time 0 — a reconnect. The clamp to 0 keeps a backdated event at
game start from producing a negative seek target.

```
video time  0s        8s                              40s
            ├─────────┼───────────────────────────────┤
            │ loading │ game in progress              │
recording   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓
            ▲         ▲                    ▲
   Recorder::start    gameTime 0           kill at gameTime 12
                      (offset = +8s)       → video time 20s
```

#### Which poll the offset is measured at

The offset above is only correct if `elapsed` and `gameTime` are sampled at
the same instant *and* `gameTime` is really a clock. Neither holds on the
first poll:

- Recording starts **on** the first successful poll (`WaitingForGame +
  LiveClientUp -> StartRecording`, dispatched synchronously), so `elapsed` at
  that poll is ~0.
- That poll lands on the loading screen, where `gameTime` is a frozen `0`
  rather than a running clock.

`0 − 0 = 0` claims the video and the game start together, which puts every
marker one whole loading screen early. So `AlignmentTracker` waits for
`gameTime` to **advance** — proving it is a clock and not the frozen 0 —
before measuring anything, and then re-measures on **every** advancing poll.
Re-measuring also absorbs drift a single offset cannot: a game pause freezes
the clock while the video keeps rolling, and dropped encoder frames skew a
fixed offset over a long game.

```mermaid
flowchart TB
    P["poll: (gameTime, elapsed)"] --> Q{"gameTime ><br/>previous gameTime?"}
    Q -->|"no — loading screen,<br/>or paused"| H["hold the last alignment<br/><small>None if there isn't one yet</small>"]
    Q -->|"yes — the clock is running"| N["alignment = elapsed − gameTime<br/><small>remembered as 'first' if it is</small>"]
    H --> S["stamp markers/samples from<br/>this poll with that alignment"]
    N --> S
    S --> F["finalize: video_time = game_time + alignment<br/><small>alignment ?? first proven ?? 0</small>"]
```

**Markers are stored with `game_time_s` and mapped at finalize**, not at
ingest. A marker seen before the clock ever moved has no alignment yet; it is
still collected (and still fed to `MarkerTracker`, so its event ID is deduped)
and resolved at finalize against the first alignment the recording ever
proved. If the clock never moved at all — the game ended during loading —
the fallback is a 1:1 mapping. Nothing is dropped.

A reconnect's first poll reports a clock already at, say, 600, which is
indistinguishable from a frozen one until it ticks. That costs one poll of
accuracy (sub-second) instead of the minutes a wrong offset would cost.

## 4. Finalize

`Supervisor::stop_recording` is the one place a recording becomes a library
entry. It is deliberately fail-soft: every step that can fail logs and
continues, because losing the footage is worse than losing its metadata.

```mermaid
flowchart TB
    A["Recorder::stop()"] --> B{"ok?"}
    B -->|"no"| Z["log; keep last_finalized empty"]
    B -->|"yes"| C["stat file for size_bytes<br/><small>+ serialize the reported audio layout</small>"]
    C --> D["db.insert_recording"]
    D -->|"err"| E["log; recording_id = None<br/><small>UI shows DB WRITE FAILED</small>"]
    D -->|"ok"| F["insert_markers"]
    F --> G["insert_samples"]
    E --> H
    G --> H["last_finalized = {path, markers}"]
    H --> I["retention::enforce_now"]
    I --> J["emit library-changed"]
    style Z fill:#ffebee,stroke:#c62828
    style E fill:#fff3e0,stroke:#ef6c00
```

**Known gap:** `lcu::fetch_match_summary` is implemented and unit-tested but
is not called from finalize, so `champion`, `queue`, `win`, `kda_*`, `patch`
and `game_id` are `NULL` on rows written by a real game. Resolving *which*
`gameId` just ended needs LCU behaviour that has not been checked against a
live client. The dev portal can invoke the fetch by hand
(`dev_fetch_match_summary`).

## 5. Where recording can refuse to start

`retention::has_room_to_record` runs as a preflight from both
`Supervisor::start_recording` and the manual `start_recording` command, and
refuses below **1 GiB free** on the recordings volume. It fails *open* on a
stat error — a check that could not run is not a reason to lose a game.
