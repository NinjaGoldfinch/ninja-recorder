# Development Guide

Design decisions, constraints, API references, and risks for ninja-recorder. This is the "why" document — read it before touching the recorder or game-integration code.

---

## 1. Hard constraints

### 1.1 Riot Vanguard (the constraint that shapes everything)

League of Legends runs under Riot Vanguard, a kernel-level anti-cheat that loads at boot. Consequences:

- **Never inject.** OBS-style "Game Capture" works by injecting a DLL into the game process to hook the graphics API. That is precisely the behavior Vanguard exists to detect. At best it silently fails; at worst it flags the user's account. This is not configurable, not an option we expose, not something we "try."
- **Capture path: Windows.Graphics.Capture (WGC).** WGC reads composited frames from DWM — no hooks, no injection, works with League in borderless/windowed mode. Display capture is the fallback.
- **No memory reading, no packet inspection.** Everything we need comes from two official local HTTP APIs (§3).
- **No VMs.** Vanguard refuses hypervisors and requires TPM 2.0 + Secure Boot. Integration testing needs real Windows hardware with a real GPU.

### 1.2 Lightweight is a tracked requirement

Targets (revisit once measured, but hold the line):

| Metric | Target |
|---|---|
| Installed size | ≤ 200 MB (libobs runtime dominates at ~150 MB) |
| Idle RAM | ≤ 100 MB |
| Recording overhead | Hardware encoder only (NVENC/AMF/QSV); no x264 on the gameplay machine |
| Idle CPU | ~0% (event-driven; LCU polling only when client is running) |

This is why the stack is Tauri (OS WebView2, ~10 MB shell) and not Electron (~400 MB+, 300 MB RAM).

---

## 2. Capture design

### 2.1 Decision: embed libobs

We embed **libobs as a library** — not "control an installed OBS via obs-websocket" (requires the user to install/configure OBS; bad product), and not fully from scratch (see §2.3).

libobs gives us, solved: frame pacing, WASAPI loopback audio capture, audio/video sync, hardware encoder integration, MP4/MKV muxing, and the WGC capture source. These are months of subtle drift bugs we do not want to own.

Reference implementation: [league_record](https://github.com/FFFFFFFXXXXXXX/league_record) (Tauri + libobs + LCU + Live Client Data). Read it before writing capture code.

**Rust bindings: a fork, not the crate as-is.** league_record's libobs FFI/IPC layer is [`libobs-recorder`](https://github.com/FFFFFFFXXXXXXX/libobs-recorder) — solid (out-of-process worker for crash isolation, bindgen bindings kept current with OBS releases, a real encoder-settings API) but its video source is hardcoded to OBS's `game_capture`, which DLL-injects the target process. That's exactly the behavior §1.1 forbids. We depend on [`NinjaGoldfinch/libobs-recorder`](https://github.com/NinjaGoldfinch/libobs-recorder), a patched fork: `game_capture` → `window_capture` forced to `method=2` (Windows.Graphics.Capture), plus `muxer_settings` for fragmented MP4 output (see §2.2's crash-safety rule — this replaces the MKV-remux approach; a fragmented MP4 needs no finalization step, so it stays playable even if the process dies mid-recording). Vendoring the crate directly wasn't viable — its `build-helper` subcrate checks in every historical libobs Windows binary release, ~900 MB — so it's a git dependency like upstream, not copied into this repo.

libobs is GPLv2; linking it makes the whole distributed binary GPL-2.0-only (see [LICENSE](LICENSE)), not a preference.

### 2.2 The `Recorder` trait

All capture lives behind a narrow trait so libobs stays an implementation detail:

```rust
trait Recorder {
    fn start(&mut self, config: RecordConfig) -> Result<()>;
    fn stop(&mut self) -> Result<PathBuf>;   // finalized MP4
    fn is_recording(&self) -> bool;
}
```

Backends:
- `LibObsRecorder` — Windows, the real one.
- `StubRecorder` — dev/macOS: sleeps, copies a fixture MP4 into place. Keeps the entire app layer developable and testable without Windows.

Rules:
- No libobs types leak above the trait.
- Recording output is **MKV remuxed to MP4 on stop** (or fragmented MP4) so a crash mid-game doesn't produce an unplayable file. A recorder crash must never lose the game footage recorded so far.

Implemented in `src-tauri/src/recorder/`: `Recorder`, `RecordConfig`, `RecorderError` in `mod.rs` (unchanged from Phase 1 except adding a `RecorderError::Backend(String)` catch-all for wrapping libobs/IPC failures without leaking their type above the trait); `stub.rs` unchanged; `libobs/mod.rs` + `libobs/window.rs` (Windows-only, `#[cfg(target_os = "windows")]`) are the new Phase 6 backend, wired into `lib.rs`'s `setup` behind the same cfg gate.

`LibObsRecorder` picks the game window (`FindWindowA` on title `"League of Legends (TM) Client"` / class `RiotWindowClass` / process `League of Legends.exe` — same identifiers league_record uses, verified against its actual source) and captures at its real client-area size (`GetClientRect`, retried briefly since the size can report (1,1) for a moment right after the window appears) rather than a hardcoded resolution — DEVELOPMENT.md's "resolution follows the game window" was aspirational until this phase. Encoder choice walks `available_encoders()` (already returned in NVENC→AMD→QSV priority order by the crate) and picks the first **H.264** one, explicitly excluding both `OBS_X264` (§2.4's no-silent-software-fallback rule — `start()` errors instead) and the AV1 variants the crate would otherwise prefer for NVENC (§2.4's WebView2-native-H.264-decode requirement, §5). Audio is `AudioSource::SYSTEM` (default output device loopback); rate control is `CBR(8000)` at 60fps per §2.4's defaults.

**Runtime files: staged outside Cargo, not via artifact-dependencies.** league_record gets `extprocess_recorder.exe` + its libobs DLLs into the build via Cargo's artifact-dependency feature (`artifact = "bin:..."`), which needs nightly Rust + the unstable `bindeps` flag — their whole project builds on nightly (CI: `dtolnay/rust-toolchain@nightly`). We can't do that: `-Z bindeps` syntax in `Cargo.toml` breaks manifest parsing *for every platform*, confirmed locally (`cargo check` on macOS failed until the artifact-dependency lines were removed) — it would force every macOS dev's `cargo check`/`npm run tauri dev` onto nightly + an unstable flag just to support an optional Windows-only binary, which is a real regression against §9's dual-platform dev loop. Instead, CI's "Stage libobs capture backend" step (`.github/workflows/ci.yml` and `release.yml`, Windows leg only) builds the fork's `extprocess_recorder` binary as a fully separate `cargo build` invocation and copies it + the matching `libobs_<version>/` DLL folder into `src-tauri/target/libobs/` directly — no Cargo dependency-graph involvement, ordinary stable Rust throughout. `tauri.windows.conf.json` then bundles that folder as a resource, and `LibObsRecorder::new` (lib.rs) resolves it at runtime via Tauri's path resolver. Anyone doing Phase 6 work locally on the Windows box needs to run the same clone-build-copy sequence by hand before `cargo run`/`npm run tauri dev` until that's scripted for local use too.

**Not verified — no Windows machine touched this code.** Same caveat this doc already applies to the async supervisor glue (§3.4): written and cross-checked against league_record's real, working source (not guessed), but nothing here has run. Specific open questions for the first Windows pass (§9):
- Does `window_capture` forced to WGC actually produce frames for League's borderless/windowed modes, and does Vanguard tolerate it (the whole point of this fork — needs a real check, not just "should work").
- The CI staging step's assumption that `Sort-Object Name -Descending` on `libobs_<version>/` directory names picks the newest — string sort, not version-aware, but the fork's directory names so far (`libobs_28.1.1` … `libobs_32.0.4`) happen to sort correctly that way.
- The `tauri.windows.conf.json` resource path (`target/libobs` → bundled next to the installed .exe) matches league_record's own working config, but its interaction with `cargo tauri dev` — where the running binary is `target/debug/ninja-recorder.exe`, one level deeper than `target/libobs` — is unclear from reading the source alone; may need the staging step to also copy into `target/debug/libobs` for dev mode to work.
- Encoder priority, window-size retry timing, and the `AudioSource::SYSTEM` choice are first-cut defaults, not tuned against real hardware.

### 2.3 Alternatives considered (and why not)

| Option | Why rejected |
|---|---|
| obs-websocket → installed OBS | User must install + configure OBS; fragile coupling to their scenes/settings |
| From scratch: WGC → D3D11 → Media Foundation SinkWriter | Legitimately clean (~1–1.5k lines, ~25 MB installed) but we'd own A/V sync, pacing, and WASAPI loopback bugs. Only revisit if libobs's footprint becomes disqualifying — the trait makes the swap possible |
| FFmpeg CLI (`ddagrab`) | No native WASAPI loopback on Windows; desktop audio would require shipping a virtual audio device. Dead end |
| Electron + obs-studio-node | Most proven path (Warcraft Recorder), but 400 MB+ / 300 MB RAM loses against "lightweight" |

### 2.4 Encoding defaults

- Detect encoder: NVENC → AMF → QSV → refuse-with-warning (no silent x264 fallback on the gameplay machine).
- 1080p60, H.264, ~8 Mbps CBR as defaults; resolution follows the game window.
- H.264 + AAC specifically: WebView2's `<video>` decodes it natively, which is what makes the review player trivial (§5).

---

## 3. League integration

Two official local HTTP APIs. Both use self-signed TLS on localhost — pin/accept the Riot self-signed cert for these connections only; never disable TLS verification globally.

### 3.1 LCU API (the client)

- **Discovery:** parse the `lockfile` next to the running client. macOS: `/Applications/League of Legends.app/Contents/LoL/lockfile`. Windows: install dir is user-configurable, so resolve it via `%PROGRAMDATA%\Riot Games\RiotClientInstalls.json`'s `associated_client` map first, falling back to the conventional `C:\Riot Games\League of Legends\lockfile`. Format: `name:pid:port:password:protocol`. Watch for the file appearing/disappearing — the client restarts, ports change. Implemented in `src-tauri/src/lcu/lockfile.rs`, with an `NINJA_RECORDER_LOCKFILE_PATH` env override for tests/non-standard installs.
- **Auth:** HTTP Basic, user `riot`, password from the lockfile.
- **Key endpoints:**
  - `GET /lol-gameflow/v1/gameflow-phase` — `None / Lobby / ChampSelect / InProgress / EndOfGame / ...`. Our record trigger. Also subscribable via the LCU WebSocket (`/lol-gameflow_v1_gameflow-phase` event) — prefer the WebSocket over polling.
  - `GET /lol-match-history/...` (post-game) — champion, KDA, win/loss, queue type for VOD metadata.
  - `GET /lol-replays/v1/rofls/{gameId}/download` — native replay download (§8).

### 3.2 Live Client Data API (in-game)

- `https://127.0.0.1:2999/liveclientdata/allgamedata` — no auth, only up while a game is running.
- Poll ~1 Hz. Relevant pieces:
  - `events.Events[]` — `ChampionKill`, `TurretKilled`, `DragonKill`, `BaronKill`, `HeraldKill`, `Ace`, `FirstBlood`, each with `EventTime` (seconds of game time).
  - `activePlayer.summonerName` / `allPlayers` — identify which events involve *us* (our kills/deaths vs. someone else's).
  - `gameData.gameTime` — for aligning game time to recording time.
- **Timestamp alignment:** record the wall-clock instant recording starts and the first observed `gameTime`; marker position in the VOD = event `EventTime` mapped through that offset. Loading screen means recording starts before `gameTime` 0 — handle the negative offset.

### 3.3 Fixtures

Every API response shape we depend on gets captured to `fixtures/` (JSON) the first time we see it, and the poller/state machine must be runnable in replay mode against fixtures. This is what makes phases 2–5 developable and unit-testable with no League running at all. Practice Tool (30-second launch, on-demand kills/objectives) is the live-testing tool of choice — never iterate against real queued games.

### 3.4 Game state machine

```
Idle ──(lockfile appears)──▶ ClientRunning
ClientRunning ──(phase: InProgress | Reconnect)──▶ WaitingForGame
WaitingForGame ──(port 2999 responds)──▶ Recording   [Recorder::start]
Recording ──(phase: EndOfGame | 2999 gone)──▶ Finalizing [Recorder::stop]
Finalizing ──▶ ClientRunning
```

Implemented as a pure transition function (`state_machine::machine::StateMachine::handle`, 11 unit tests covering the edge cases below) driven by a thin async supervisor (`state_machine::supervisor::Supervisor`) that spawns/aborts the lockfile/gameflow/Live-Client-Data watchers per `Action` and calls `Recorder::start`/`stop`. The pure part is fully tested; the supervisor's async glue is not — no League client is installed on the machine this was built on, so nothing here has touched a real LCU or Live Client Data connection yet (verification pending Phase 8, or earlier if League gets installed sooner).

Finalizing currently only stops the recorder and time-aligns whatever markers were collected — it does not yet call `lcu::fetch_match_summary` or write a DB row (no DB exists until Phase 4, and resolving *which* `gameId` just finished needs LCU endpoint research this machine can't verify live). The finalized recording + markers are held in memory and exposed via the `game_state_status` command for now.

Edge cases handled by the pure transition function (see its tests): game crash mid-match (Live Client Data stops responding), client crash (lockfile disappears) at every stage, reconnect to an in-progress game (state machine has no memory of *how* it entered `WaitingForGame`, so a reconnect behaves identically to a fresh game start — recording begins once Live Client Data becomes reachable, later than a from-the-start recording would), practice tool (goes through the same `Reconnect`/`InProgress` phases as a real game), dodges/cancelled champ select (bounces `WaitingForGame` back to `ClientRunning` without ever recording), and a client restart mid-finalize (picked up correctly regardless of ordering against `FinalizeComplete`).

Two edge cases from the original list are *not* verified: **spectator mode** — the state machine simply doesn't special-case any phase name beyond `InProgress`/`Reconnect`/end-of-game ones, so if gameflow reports a distinct phase while spectating, it won't trigger recording; but if it turns out spectating also reports `InProgress`, this would incorrectly record it, and that can only be confirmed live. **Machine sleep** — not simulated in this environment at all; the poller's backoff and lockfile-watch would likely eventually recover state after wake, but this needs real testing on the Windows machine (Phase 8).

---

## 4. Data model

SQLite (via `rusqlite`), one DB in app data dir. MP4s on disk are the source of truth for video; DB rows are metadata.

```
recordings:  id, path, started_at, duration_s, game_id, queue, champion,
             role, win, kda_k, kda_d, kda_a, patch, pinned, size_bytes
markers:     id, recording_id, game_time_s, video_time_s, kind, payload_json
             -- kind: kill | death | assist | dragon | baron | herald |
             --       turret | ace | first_blood | custom
```

- A DB row without its file (user deleted the MP4) is cleaned up on scan; a file without a row is imported as "unknown recording." The library must survive users touching the folder.

Implemented in `src-tauri/src/db/` (`Db` + `reconcile`), migrations via `rusqlite_migration`, `rusqlite`'s `bundled` feature so no system SQLite is required on a fresh machine. Reconciliation runs once at app startup and on demand (`rescan_recordings` command). The state machine's Finalizing step (§3.4) writes a `recordings` row + its `markers` on every stop — `game_id`/`queue`/`champion`/`role`/`win`/`kda_*`/`patch`/`duration_s` all stay `NULL` for now, since that data comes from `lcu::fetch_match_summary`, still unwired per §3.4's noted gap.

---

## 5. Review player

- WebView2 `<video>` element — H.264/AAC MP4 decodes natively, so seeking, playback rate, and frame-stepping are free.
- Custom timeline component: marker glyphs per event kind, click-to-jump, "next death" / "prev death" hotkeys.
- Later: clip export (`ffmpeg -ss .. -to .. -c copy` — stream copy, no re-encode; ship a minimal ffmpeg binary or use libobs's muxer).

Implemented in `src/review.ts` + `index.html`'s `#review-view`. The video loads via Tauri's asset protocol (`convertFileSrc`, scoped in `tauri.conf.json` to `$APPDATA/recordings/*` — needed the `protocol-asset` Cargo feature, not just config). Frame-step is a ±1/30s time nudge, not true frame-accurate seeking — no per-recording frame rate is probed anywhere yet, so this is an approximation good enough for review, not for precision editing. Closely-spaced markers (common near a teamfight) alternate a vertical lane on the timeline so their glyphs don't visually collide. The library list, filters, and sort are all client-side over the already-fetched row set — fine at solo-user library sizes, would need real pagination/querying if that stops being true.

Verified: layout/CSS visually in a browser (with injected mock data, since a plain browser tab has no Tauri IPC bridge to exercise real `invoke` calls) and a full `cargo tauri dev` launch (asset-protocol config + new `get_recording_markers` command, no capability/schema errors, stable). **Not verified**: actual video playback (no real MP4 fixture available in this environment — drop one in as `fixtures/sample.mp4` to test) and the hotkey→seek interaction against real marker data (needs a loaded recording, which needs either a live app session or a real video file to click through manually).

---

## 6. Disk management (launch feature, not a later one)

1080p60 @ 8 Mbps ≈ **3.5 GB/hour**. A ranked session ≈ 15 GB. Without retention, we fill the user's SSD in two weeks and get uninstalled.

- Retention policy: max total size AND max age, whichever bites first; `pinned` recordings are exempt.
- Enforce on app start and after each `Finalizing`.
- Show current usage in the UI; never delete without the policy being visible to the user.

---

## 7. YouTube upload (phase 10)

- YouTube Data API v3, OAuth 2.0 **desktop flow with loopback redirect** (the OOB flow is dead). Store refresh token in Windows Credential Manager / Keychain, not in the DB.
- **Quota reality:** an upload costs 1,600 units; default project quota is 10,000/day → **~6 uploads/day across all users** until Google grants a quota increase (requires an audit). Design consequence: upload is a deliberate per-VOD action with clear failure messaging, never auto-upload.
- Resumable upload protocol is mandatory (multi-GB files, flaky connections).
- Unlisted by default.

## 8. ROFL replays (phase 11)

- The LCU can download the native replay (~5 MB vs 3.5 GB video) — full camera control on playback.
- Caveats: `.rofl` files only play on the **exact patch** they were recorded on, and playback requires launching the game client. This is a companion to video, not a substitute. Saving both costs almost nothing.

---

## 9. Development workflow

| Layer | Where | Loop |
|---|---|---|
| LCU / Live Client Data / state machine | macOS, native (League runs on macOS; APIs identical) | seconds |
| VOD library, review UI, upload | macOS, stub recorder + fixture MP4s | seconds |
| Capture backend | Windows box, `git pull && cargo run` (Rust + MSVC Build Tools installed) | seconds |
| Full integration + Vanguard verification | Windows, CI-built installer | occasional |

- **Never cross-compile the Windows build from macOS.** libobs linking + DLL bundling + installer generation via `cargo-xwin` is a fight with no payoff. GitHub Actions `windows-latest` builds the installer (NSIS); download the artifact.
- Vanguard verification (capture works during a real Vanguard-protected game, no flags) is a one-time check per significant capture change, not an iterative loop — capture iterates against any window (browser, video loop), no League needed.
- **CI** ([`.github/workflows/`](../.github/workflows/)): `ci.yml` runs tests + clippy on every push to `main` and every PR (Windows + macOS, matrix), then builds installers natively on each platform and uploads them as workflow artifacts (7-day retention) — a PR's own build is downloadable from its Actions run. `release.yml` triggers on `v*.*.*` tags and publishes a draft GitHub Release with both installers via [`tauri-action`](https://github.com/tauri-apps/tauri-action). Neither build is code-signed yet (no cert configured) — Windows SmartScreen and macOS Gatekeeper both warn on first run. macOS builds only exercise the stub recorder; they're a dev/testing convenience, not a shipping target (§1.1).

---

## 10. Risks

| Risk | Mitigation |
|---|---|
| Riot changes LCU/Live Client endpoints | Unofficial-but-tolerated APIs; fixtures + thin client layer localize breakage. Watch league_record and lcu-driver communities |
| Vanguard behavior changes re: WGC | WGC is a core OS compositor API used by Xbox Game Bar itself — lowest-risk capture path that exists. No fallback plan needed beyond display capture |
| libobs Rust bindings immaturity | Using a patched fork of `libobs-recorder` (§2.1) rather than raw bindings — but it's still a young, single-maintainer ecosystem and now a fork we own the patch for. Budget time; fallback is a thin C shim over the (stable, C) libobs API. The trait keeps this contained |
| Our `libobs-recorder` fork falls behind upstream | We only carry a single small commit on top of upstream (capture source + muxer settings); re-basing onto a newer upstream tag is cheap. Watch for upstream libobs version bumps we might want (new encoders, bug fixes) |
| YouTube quota audit friction | Ship upload as "bring your own consent" early; apply for quota increase well before it matters |
| Disk-full during recording | Preflight free-space check at record start; stop gracefully + notify rather than corrupt |
| Window mode edge cases (exclusive fullscreen) | WGC needs a composited surface. Detect and nudge user toward borderless (the League default) |
