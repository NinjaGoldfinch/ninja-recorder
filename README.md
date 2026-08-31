# ninja-recorder

A lightweight League of Legends VOD recorder for Windows. Automatically records your games, tags the VOD timeline with in-game events (kills, deaths, objectives), and provides a review player built for improvement — with YouTube upload and native replay (`.rofl`) support planned.

> **Status:** Planning / early scaffolding. Nothing runs yet.

## Why

- **Zero-config recording** — detects when a game starts via the League Client API and records automatically. No OBS setup, no scene configuration.
- **Event-tagged VODs** — every kill, death, dragon, baron, and turret becomes a marker on the timeline. Jump straight to your deaths. Clip the teamfight.
- **Lightweight** — Tauri shell (~10 MB, uses OS WebView2) + embedded libobs. No Electron, no bundled Chromium.
- **Vanguard-safe by design** — capture uses Windows Graphics Capture (WGC) only. No process injection, no hooks, no memory reading. Ever.

## Architecture (planned)

```
┌─────────────────────────────────────────────────┐
│ Tauri v2 app                                    │
│                                                 │
│  ┌───────────────┐   ┌──────────────────────┐   │
│  │ Rust core     │   │ WebView2 frontend    │   │
│  │               │   │                      │   │
│  │ LCU client ───┼──▶│ VOD library UI       │   │
│  │ Live Client   │   │ Review player        │   │
│  │   Data poller │   │  (<video> + marker   │   │
│  │ Game state    │   │   timeline)          │   │
│  │   machine     │   │                      │   │
│  │ Recorder trait│   └──────────────────────┘   │
│  │  ├ libobs (Win)                              │
│  │  └ stub (dev/macOS)                          │
│  │ SQLite (VOD metadata + markers)              │
│  └───────────────┘                              │
└─────────────────────────────────────────────────┘
```

- **Capture:** [libobs](https://github.com/obsproject/obs-studio) embedded as a library (not OBS-the-app, no user install), driven programmatically. WGC window capture + hardware encode (NVENC/AMF/QSV).
- **Game detection:** LCU API (`/lol-gameflow/v1/gameflow-phase`) triggers record start/stop.
- **Events:** Live Client Data API (`https://127.0.0.1:2999`) polled ~1 Hz during games; events become timeline markers.
- **Storage:** MP4 files on disk + SQLite rows (match metadata, markers, file paths).

See [DEVELOPMENT.md](DEVELOPMENT.md) for the full design doc: constraints, API details, capture design, risks, and the reasoning behind each decision.

## Roadmap

Tracked as GitHub issues — each phase is an issue.

| Phase | What | Platform |
|---|---|---|
| 1 | Scaffold Tauri v2 project, `Recorder` trait + stub backend | macOS/any |
| 2 | LCU client: lockfile discovery, gameflow polling, match metadata | macOS/any |
| 3 | Live Client Data poller + event → marker pipeline (with fixture recording) | macOS/any |
| 4 | SQLite VOD library + data model | macOS/any |
| 5 | Review UI: player, marker timeline, VOD browser | macOS/any |
| 6 | libobs capture backend (WGC + hardware encode) | **Windows** |
| 7 | GitHub Actions Windows build → installer artifact | CI |
| 8 | Integration test on real hardware + Vanguard verification | **Windows** |
| 9 | Disk retention policy (max size / max age / pinned VODs) | any |
| 10 | YouTube upload (OAuth desktop flow, resumable upload) | any |
| 11 | ROFL replay download alongside video | any |

## Development

Primary development happens on macOS against the stub recorder (League runs natively on macOS and both local APIs behave identically — Practice Tool generates real events). The capture backend is developed on Windows via `cargo run`; installers are produced by CI, not built locally.

```bash
# prerequisites: Rust stable, Node.js
npm install
npm run tauri dev
```

The Rust project lives at `src-tauri/Cargo.toml`, not the repo root — bare
`cargo` commands (`cargo check`, `cargo test`) must be run from `src-tauri/`,
e.g. `cd src-tauri && cargo test`. `npm run tauri dev` handles this for you
from the repo root.

## Non-negotiable constraints

1. **No injection.** OBS "Game Capture" style hooking is permanently off the table — Riot Vanguard is a kernel anti-cheat and DLL injection is exactly what it exists to stop. WGC/display capture only.
2. **Official APIs only.** LCU + Live Client Data. No memory reading, no packet sniffing.
3. **Lightweight is a feature.** Idle RAM and install size are tracked, not vibes.

## License

TBD.
