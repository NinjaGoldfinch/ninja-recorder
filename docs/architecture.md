# Architecture

How the pieces fit together, what owns what, and where a given behaviour
lives in the tree. Start here; [recording-pipeline.md](recording-pipeline.md)
then walks the runtime path end to end.

For the *reasoning* behind these choices — why libobs, why no injection, why
Tauri — read [DEVELOPMENT.md](../DEVELOPMENT.md). This file describes the
shape; that one defends it.

---

## The whole system at a glance

One process, two windows, three external interfaces (two local HTTP APIs and
the filesystem).

```mermaid
flowchart TB
    subgraph League["League of Legends (external)"]
        LCU["LCU API<br/>127.0.0.1, port from lockfile<br/>HTTP Basic auth"]
        LIVE["Live Client Data API<br/>127.0.0.1:2999<br/>no auth, in-game only"]
    end

    subgraph App["ninja-recorder (single Tauri v2 process)"]
        direction TB
        subgraph Rust["Rust core"]
            SUP["state_machine::Supervisor<br/><small>async orchestration</small>"]
            SM["state_machine::StateMachine<br/><small>pure transitions</small>"]
            LCUC["lcu::<br/>lockfile · gameflow · match_data"]
            LC["live_client::<br/>client · poller · events"]
            REC["recorder::Recorder<br/><small>trait</small>"]
            DB["db::Db<br/><small>SQLite + migrations</small>"]
            RET["retention::<br/><small>size / age policy</small>"]
        end
        subgraph Web["WebView2 / WKWebView frontend"]
            MAIN["index.html<br/>library · review · settings"]
            DEV["dev.html<br/><small>dev portal, feature-gated</small>"]
        end
    end

    subgraph Disk["Disk"]
        MP4["recordings/*.mp4"]
        SQLITE["library.sqlite"]
    end

    LCU -->|"phase, match summary"| LCUC
    LIVE -->|"allgamedata @ 1 Hz"| LC
    LCUC --> SUP
    LC --> SUP
    SUP <--> SM
    SUP --> REC
    SUP --> DB
    SUP --> RET
    REC --> MP4
    DB --> SQLITE
    RET --> MP4
    RET --> SQLITE
    MAIN <-->|"Tauri invoke"| Rust
    DEV <-->|"dev_* invoke"| Rust
    MP4 -->|"asset protocol"| MAIN
```

## Rust module map

| Module | Owns | Key entry points |
|---|---|---|
| `lcu/lockfile.rs` | Finding the running client and its credentials | `discover`, `watch` |
| `lcu/gameflow.rs` | Phase changes (WebSocket, polling fallback) | `watch` |
| `lcu/match_data.rs` | Post-game summary (champion, KDA, win) | `fetch_match_summary` |
| `lcu/client.rs` | HTTPS + Basic auth against the client's self-signed cert | `LcuHttpClient` |
| `live_client/client.rs` | Port 2999 HTTPS client | `fetch_all_game_data` |
| `live_client/poller.rs` | 1 Hz poll loop with exponential backoff (cap 10 s) | `watch` |
| `live_client/events.rs` | Snapshot → markers, team-advantage samples, video-time alignment | `MarkerTracker`, `TimeAlignment`, `team_diff` |
| `state_machine/machine.rs` | The pure `(state, event) → (state, actions)` function | `StateMachine::handle` |
| `state_machine/supervisor.rs` | Spawning/aborting watchers, driving the recorder, finalizing | `Supervisor` |
| `recorder/mod.rs` | The `Recorder` trait and its config/error types | `Recorder`, `RecordConfig` |
| `recorder/libobs/` | Windows capture backend (WGC + hardware encode) | `LibObsRecorder` |
| `recorder/stub.rs` | Dev/macOS backend that copies a fixture MP4 | `StubRecorder` |
| `db/mod.rs` | Schema, migrations, every query | `Db` |
| `db/reconcile.rs` | Reconciling DB rows against files on disk | `reconcile` |
| `retention.rs` | Deletion policy and free-space preflight | `select_for_deletion`, `enforce_now`, `has_room_to_record` |
| `fixtures.rs` | Capturing live API responses to `fixtures/` | `enabled`, `record` |
| `dev/` | Dev portal backend, compiled out without `--features devtools` | `dev_*` commands |
| `lib.rs` | Tauri setup, app state, the command surface | `run` |

The consistent shape across `state_machine`, `db::reconcile` and `retention`
is **a pure decision function plus a thin I/O wrapper**. The decision is unit
tested directly; the wrapper is deliberately kept too small to hide a bug.

```mermaid
flowchart LR
    A["Inputs<br/><small>rows, events, clock</small>"] --> B["Pure function<br/><small>select_for_deletion<br/>StateMachine::handle<br/>reconcile</small>"]
    B --> C["Decision<br/><small>Vec&lt;Action&gt;, delete list</small>"]
    C --> D["Thin I/O wrapper<br/><small>enforce_now, Supervisor::execute</small>"]
    D --> E["Filesystem / SQLite / Recorder"]
    style B fill:#ede7f6,stroke:#5e35b1
    style D fill:#fff3e0,stroke:#ef6c00
```

## The `Recorder` trait boundary

Capture is the only genuinely platform-specific part of the app, so it sits
behind a three-method trait and nothing above it knows libobs exists.

```mermaid
flowchart TB
    SUP["Supervisor"] --> T{"Recorder trait<br/>start · stop · is_recording"}
    T -->|"#[cfg(windows)]"| L["LibObsRecorder<br/><small>WGC window capture,<br/>NVENC/AMF/QSV H.264,<br/>fragmented MP4 + faststart remux</small>"]
    T -->|"everything else"| S["StubRecorder<br/><small>copies fixtures/sample.mp4</small>"]
    style T fill:#ede7f6,stroke:#5e35b1
```

The stub is not a mock — it writes a real, playable file into the real
recordings directory and takes a real amount of time to do it. That is what
keeps the library, retention, review player and the whole state machine
developable on macOS with no Windows box in the loop.

## Frontend

Vanilla TypeScript, no framework, split by **state ownership** rather than by
widget — see [frontend.md](frontend.md) for the module graph and the IPC
surface.

## Process and window model

```mermaid
flowchart LR
    subgraph P["ninja-recorder.exe"]
        W1["Main window<br/>index.html"]
        W2["Dev portal window<br/>dev.html<br/><small>devtools feature only</small>"]
        RT["Rust core + tokio runtime"]
    end
    W1 -. invoke .-> RT
    W2 -. dev_* invoke .-> RT
    RT -. library-changed event .-> W1
```

Both windows talk to the same Rust state and the same database. The dev
portal is a second Vite entry point gated on the `NINJA_DEVTOOLS` env var and
a second command set gated on the `devtools` Cargo feature — a plain
`npm run build` cannot emit it, and a default `cargo build` cannot register
its commands. See [dev-portal.md](dev-portal.md).
