# Development Guide

Design decisions, constraints, API references, and risks for ninja-recorder. This is the "why" document — read it before touching the recorder or game-integration code.

For the "what and how" — component diagrams, the runtime sequence, the schema, the CI job graph — see **[docs/](docs/)**.

> **Section numbers here are load-bearing.** Roughly 35 source comments cite this file as `DEVELOPMENT.md §2.2`, `§3.4` and so on. Add sections, rewrite their contents, but do not renumber them without updating every citation (`grep -rn 'DEVELOPMENT.md §' src src-tauri`).

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

The idle-CPU row is a target, not a freebie. `lcu::lockfile::watch` is the only
unconditional background work — it runs from launch to exit whether or not
League is even installed — so it is the floor under idle CPU, and it earns its
2 s cadence only while the client is up. A sustained absence ramps it to 30 s
(`lockfile::poll_delay`), with a short grace window first so a *client restart*
is still noticed at full speed. It also caches the resolved Windows install
directory rather than re-reading and re-parsing `RiotClientInstalls.json` on
every single tick.

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
    fn prepare(&mut self) -> Result<()>;     // warm up; default no-op
    fn release(&mut self);                   // go cold; default no-op
}
```

Backends:
- `LibObsRecorder` — Windows, the real one.
- `StubRecorder` — dev/macOS: sleeps, copies a fixture MP4 into place. Keeps the entire app layer developable and testable without Windows.

**Decision: the backend is warm only while the League client is.** Bringing
`LibObs` up spawns the out-of-process worker *and* sends it `Init`, which runs
`obs_startup` and loads every plugin — so a live backend is a D3D11 device and
the whole libobs plugin set resident in another process, not a dormant handle.
It used to be constructed in `lib.rs`'s `setup` and held until exit, which put
the single largest item on the idle-RAM budget (§1.2) on a machine that might
never open League.

`prepare`/`release` move that to the state machine's `ClientRunning` window:
warm when the client appears, cold when it goes away. Two alternatives were
rejected. Staying warm forever is the old behaviour and the thing being fixed.
Going lazy on the first `start` instead would put libobs init *inside* the
record path, where it lands on top of the existing bounded window-size wait and
risks losing the opening seconds of a game — whereas the client being open is a
reliable minutes-ahead signal that a game is plausible.

`prepare` is therefore a pre-warm and nothing depends on it: `start` calls the
same idempotent `ensure_up`, so the two racing (a client that goes straight into
a game) is harmless. `release` refuses to run while a recording is in flight.
The supervisor drives both from the resulting *state*, not from `Action`s — a
client restart emits a gameflow stop and start while staying in `ClientRunning`,
and acting on those would tear the backend down and rebuild it for nothing.

The cost is that init failure is no longer a startup event, so it can't swap in
a `FailedRecorder` any more. `LibObsRecorder::new` is now infallible (only the
worker-binary path lookup can fail that way) and `backend_name` carries the
diagnostic instead: `libobs (idle)`, `libobs (ready)`, or
`libobs (unavailable: …)`. There is also a **first-recording-of-a-session risk
that only real hardware can settle**: whether a backend brought up minutes
before `start` is still healthy, and whether repeated bring-up/tear-down across
several games in a session leaks anything on the libobs side
([docs/windows-verification.md](docs/windows-verification.md)).

Rules:
- No libobs types leak above the trait.
- Recording output is **MKV remuxed to MP4 on stop** (or fragmented MP4) so a crash mid-game doesn't produce an unplayable file. A recorder crash must never lose the game footage recorded so far.

Implemented in `src-tauri/src/recorder/`: `Recorder`, `RecordConfig`, `RecorderError` in `mod.rs` (with a `RecorderError::Backend(String)` catch-all for wrapping libobs/IPC failures without leaking their type above the trait); `stub.rs` unchanged; `libobs/mod.rs` + `libobs/window.rs` (Windows-only, `#[cfg(target_os = "windows")]`) are the real backend, wired into `lib.rs`'s `setup` behind the same cfg gate.

`LibObsRecorder` picks the game window (`FindWindowA` on title `"League of Legends (TM) Client"` / class `RiotWindowClass` / process `League of Legends.exe` — same identifiers league_record uses, verified against its actual source) and captures at its real client-area size (`GetClientRect`, retried briefly since the size can report (1,1) for a moment right after the window appears) rather than a hardcoded resolution, which is what §2.4's "resolution follows the game window" means in practice. Encoder choice walks `available_encoders()` (already returned in NVENC→AMD→QSV priority order by the crate) and picks the first **H.264** one, explicitly excluding both `OBS_X264` (§2.4's no-silent-software-fallback rule — `start()` errors instead) and the AV1 variants the crate would otherwise prefer for NVENC (§2.4's WebView2-native-H.264-decode requirement, §5). Audio is whatever the user's preset asks for, split across separate mp4 tracks (§2.5); rate control is `CBR(8000)` at 60fps per §2.4's defaults.

**Runtime files: staged outside Cargo, not via artifact-dependencies.** league_record gets `extprocess_recorder.exe` + its libobs DLLs into the build via Cargo's artifact-dependency feature (`artifact = "bin:..."`), which needs nightly Rust + the unstable `bindeps` flag — their whole project builds on nightly (CI: `dtolnay/rust-toolchain@nightly`). We can't do that: `-Z bindeps` syntax in `Cargo.toml` breaks manifest parsing *for every platform*, confirmed locally (`cargo check` on macOS failed until the artifact-dependency lines were removed) — it would force every macOS dev's `cargo check`/`npm run tauri dev` onto nightly + an unstable flag just to support an optional Windows-only binary, which is a real regression against §9's dual-platform dev loop. Instead, CI's "Stage libobs capture backend" step (`.github/workflows/ci.yml`'s `build` job, Windows leg only) builds the fork's `extprocess_recorder` binary as a fully separate `cargo build` invocation and copies it + the matching `libobs_<version>/` DLL folder into `src-tauri/target/libobs/` directly — no Cargo dependency-graph involvement, ordinary stable Rust throughout. `tauri.windows.conf.json` then bundles that folder as a resource, and `LibObsRecorder::new` (lib.rs) resolves it at runtime via Tauri's path resolver. Anyone working on the capture backend locally on the Windows box needs to run the same clone-build-copy sequence by hand before `cargo run`/`npm run tauri dev` until that's scripted for local use too.

**Faststart remux on stop, staged the same way.** The fork's `muxer_settings` (above) trade seekability for crash-safety: `frag_keyframe+empty_moov+default_base_moof` means no player — including the review UI's own WebView2 `<video>` — can reliably scrub the file, since there's no upfront seek index. `LibObsRecorder::stop` fixes this up after every *clean* stop with a stream-copy remux (`ffmpeg -c copy -movflags +faststart`, lossless, just rewrites the container index) before handing the path back. `ffmpeg.exe` is staged into the same `target/libobs/` resource folder by a sibling CI step ("Stage ffmpeg for faststart remux") that downloads a static build from BtbN's FFmpeg-Builds releases — optional at runtime (`lib.rs` resolves it with `.ok()`), so a failed download degrades to unseekable-but-still-playable recordings rather than breaking the build. **Not verified** — same caveat as the rest of this backend below; nothing has confirmed the remux actually runs against a real capture on a real Windows box yet, only that it type-checks.

**Every ffmpeg spawn gets `CREATE_NO_WINDOW`.** ffmpeg ships as a console-subsystem binary, so a GUI process spawning one makes Windows allocate it a fresh console — an empty black terminal window sitting over the game for the length of every faststart remux, and again each time the review player extracts a stem (§2.5). Both call sites capture stdout and stderr, so that window never had anything to display; it is pure noise, and on the remux path it lands at exactly the moment the player is reading the post-game screen. `lib.rs`'s `ffmpeg_command` is now the only way the bundled ffmpeg is launched and it sets the flag there, so a third call site cannot reintroduce the window by forgetting. The libobs worker needs no equivalent: the fork builds `extprocess_recorder.exe` with `windows_subsystem = "windows"` for release, so it is only ever visible in Task Manager — which is where [windows-verification.md](docs/windows-verification.md) checks for it.

**Not verified — no Windows machine touched this code.** Same caveat this doc already applies to the async supervisor glue (§3.4): written and cross-checked against league_record's real, working source (not guessed), but nothing here has run. Specific open questions for the first Windows pass (§9):
- Does `window_capture` forced to WGC actually produce frames for League's borderless/windowed modes, and does Vanguard tolerate it (the whole point of this fork — needs a real check, not just "should work").
- The CI staging step's assumption that `Sort-Object Name -Descending` on `libobs_<version>/` directory names picks the newest — string sort, not version-aware, but the fork's directory names so far (`libobs_28.1.1` … `libobs_32.0.4`) happen to sort correctly that way.
- The `tauri.windows.conf.json` resource path (`target/libobs` → bundled next to the installed .exe) matches league_record's own working config, but its interaction with `cargo tauri dev` — where the running binary is `target/debug/ninja-recorder.exe`, one level deeper than `target/libobs` — is unclear from reading the source alone; may need the staging step to also copy into `target/debug/libobs` for dev mode to work.
- Encoder priority and window-size retry timing are first-cut defaults, not tuned against real hardware.
- Does `wasapi_process_output_capture` produce non-silent samples for a Vanguard-protected `League of Legends.exe`? Per-application loopback is the source behind every preset that names "game audio" (§2.5), and it is the one part of the audio design with no fallback if the answer is no — desktop capture is the documented workaround.

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
- Audio is one AAC track per captured source at 160 kbps, track 0 being the combined mix (§2.5). MP4 rather than MKV even though OBS recommends MKV for multi-track: §2.2's crash-safety rule is already satisfied by fragmented MP4, and MKV would cost the review player its native `<video>` playback for no gain.

### 2.5 Multi-track audio

The user picks *what* to capture; the recorder writes each source to its own
MP4 audio track.

| Preset | Track 0 | Track 1 | Track 2 | Track 3 |
|---|---|---|---|---|
| Game | Game | — | — | — |
| Game + mic | Everything | Game | Mic | — |
| Game + mic + Discord | Everything | Game | Mic | Discord |
| Desktop | System audio | Game | — | — |

**Track 0 is always the combined mix.** This is the decision the rest of the
design follows from. It means a player that knows nothing about any of this —
including our own review player's `<video>` element, and whatever the user
drags the file into — plays the right thing by default. Everything after
track 0 is an isolated stem, so a clip exporter written later can cut the
microphone out of a VOD recorded today. That is the whole reason the stems
exist; without it the tracks would only be a settings screen.

**Only the sources a preset names are captured.** "Game audio only" writes one
track and never opens the microphone. Recording the mic anyway "just in case"
would be cheap (~160 kbps) and useful, and it is still the wrong default: the
preset names would stop being true, and a recorder that captures your voice
when you told it not to is a bug regardless of what it does with the result.

**Game-only is one track, not two.** With a single source the combined mix and
the stem are the same signal, so the second track would be a byte-identical
duplicate. Desktop is the only two-track preset — system audio already
contains the game, so track 1 isolates the game back out of it.

**Discord is captured as a named application, not a Discord-shaped special
case.** `AudioSourceKind::Application { exe }` takes any executable, matched
by `WINDOW_PRIORITY_EXE` rather than window title — Discord retitles itself to
whatever channel is open, so title matching would break constantly. The same
mechanism is what a future "custom" preset needs for Spotify or anything else.

**The capture fork had to change; a separately-captured mic was the
alternative.** Upstream `libobs-recorder` creates one AAC encoder on mixer 0
and mixes every source into it. The alternative to patching it was capturing
the microphone ourselves and muxing it in afterwards with ffmpeg — which means
owning A/V sync for the mic, exactly the class of bug §2.1 chose libobs to
avoid. The fork now creates one encoder per track; `obs_audio_encoder_create`
fixes an encoder's mixer index at creation with no setter, so encoder *i* is
permanently track *i*, and what varies per recording is a per-source mixer
bitmask. A libobs source can feed several mixes at once, which is what makes
the combined-mix-plus-stems layout nearly free — game audio on both track 0
and track 1 is one extra bit, not a second capture.

Two traps in that area, both of which fail silently:
- libobs defaults a source's `audio_mixers` to `0xFF` (every mix). Left alone,
  every track would contain an identical full mix.
- `num_audio_mixes` walks the output's encoder array and stops at the first
  null, so binding tracks 0 and 2 while leaving 1 unbound truncates the file
  to **one** track.

**The faststart remux had to be fixed in the same change.** `remux_faststart`
ran `-c copy` with no `-map`, so ffmpeg's default stream selection kept a
single "best" audio stream — which would have deleted every stem on the way
out, permanently, since the remux renames over the original. It now maps all
streams explicitly and marks track 0 as the default disposition, which
`obs-ffmpeg-mux` never sets.

**Track switching in the review player uses ffmpeg, not the browser.**
WebView2 offers no way to select among the audio tracks of one `<video>`:
`HTMLMediaElement.audioTracks` sits behind Chromium's `AudioVideoTracks`
Blink flag, which has been at status "test" for roughly a decade with no
standards track, on an Evergreen runtime whose version we don't control.
Enabling it via `additionalBrowserArgs` would also silently replace wry's
default `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`.
Selecting a stem instead extracts it to a cached sidecar (`-c copy`, so tens
of megabytes rather than the multi-gigabyte video) which a hidden `<audio>`
plays against the muted video, with drift correction in the playhead loop the
player already runs. Track 0 — the common case — needs none of it.

This does mean owning a small amount of A/V sync after all, which §2.1 says
we didn't want. The mitigating difference is that it is *playback* sync over
a file that already exists, recoverable by reloading, rather than capture sync
that would corrupt a recording. It is confined to the review player and
touches nothing on the recording path.

**Preferences.** The preset is one `settings_kv` row (`audio_preset`) holding
JSON — a zero-migration change, per §4's reasoning. Unlike `theme` it is read
and validated backend-side: a bad theme value looks wrong, a bad audio preset
changes what gets recorded, and an unreadable one falls back to game-audio-only
rather than to whatever parses. The per-recording layout is a separate,
nullable column (`recordings.audio_tracks_json`), because NULL is the honest
answer for the VODs that predate this and for anything a rescan imported.

---

## 3. League integration

Two official local HTTP APIs. Both use self-signed TLS on localhost — pin/accept the Riot self-signed cert for these connections only; never disable TLS verification globally.

### 3.1 LCU API (the client)

- **Discovery:** parse the `lockfile` next to the running client. macOS: `/Applications/League of Legends.app/Contents/LoL/lockfile`. Windows: install dir is user-configurable, so resolve it via `%PROGRAMDATA%\Riot Games\RiotClientInstalls.json`'s `associated_client` map first, falling back to the conventional `C:\Riot Games\League of Legends\lockfile`. Format: `name:pid:port:password:protocol`. Watch for the file appearing/disappearing — the client restarts, ports change. Implemented in `src-tauri/src/lcu/lockfile.rs`, with an `NINJA_RECORDER_LOCKFILE_PATH` env override for tests/non-standard installs.
- **Auth:** HTTP Basic, user `riot`, password from the lockfile.
- **Key endpoints:**
  - `GET /lol-gameflow/v1/gameflow-phase` — `None / Lobby / ChampSelect / InProgress / EndOfGame / ...`. Our record trigger. Also subscribable via the LCU WebSocket (`/lol-gameflow_v1_gameflow-phase` event) — prefer the WebSocket over polling.
  - `GET /lol-gameflow/v1/session` (during the game) — `gameData.gameId`, `gameData.queue.id` and `gameData.isCustomGame`. Read once when gameflow reaches `InProgress`. Working out *which* `gameId` just ended is the problem that kept `match_data` unwired; the client will say so while the game is still running, so it is read then rather than deduced afterwards.
  - `GET /lol-end-of-game/v1/eog-stats-block` (post-game) — *our own* stats block: `teams[].isPlayerTeam` + `isWinningTeam`, our `championId`, our scoreboard. Tried **first**, because it needs no participant join at all and is populated during `EndOfGame`.
  - `GET /lol-match-history/v1/games/{gameId}` (post-game) — champion, KDA, win/loss, queue id, `timeline.lane`/`.role` and `gameVersion`. The only source for `role` and `patch`, but it lags the end of the game and has to be joined back to us.
  - `GET /lol-game-data/assets/v1/champion-summary.json` — the client's own asset store: `{id, name, alias, contentId, description, squarePortraitPath, roles}` per champion, plus an `id: -1` "None" sentinel. Turns the `championId` the two post-game endpoints answer with into a name. **Confirmed off a real client (2026-09-07)**, unlike the two endpoints above.
  - `GET /lol-replays/v1/rofls/{gameId}/download` — native replay download (§8).
- **Identifying ourselves in a match-history response is the fragile part**, and it is not a matter of picking the "right" field. The response splits players across `participants[]` (stats, joined by `participantId`) and `participantIdentities[]` (accounts), so the identity has to be matched back to `/lol-summoner/v1/current-summoner` by an account key — and which keys the endpoint sends has moved over time. The LCU's own OpenAPI spec carries no `puuid` on a match-history participant identity at all, only `accountId`/`summonerId`/`summonerName`, while 74 other schemas in the same spec do have one. `match_data.rs` therefore treats every key as optional and tries `puuid`, then `summonerId`, then `accountId`, requiring the key to be present on *both* sides. A client that sends `puuid` and one that does not both work without the code needing to know which it is talking to.
- **The post-game fetch cannot happen during the finalize.** At the instant `Recording → Finalizing` fires the client is still in `WaitingForStats` — match history 404s — and the same transition tears down the gameflow watch that owned the LCU connection. So the row is written from what Live Client Data established during the game, and `match_summary::patch` fills in `role`, `patch` and a confirmed `queue`/`win`/`kda_*` afterwards, on a bounded retry schedule (2s, 4s, 8s, then three at 15s), then re-emits `library-changed` so the card fills itself in while the user watches. It also fills `champion` for the one case the live path cannot cover — a game whose Live Client Data poller never came up — resolving the id best-effort, so a name it cannot find never costs the row its outcome and queue id. Past the ceiling it gives up **silently**: the row already carries champion, KDA and outcome, and a missing queue id is not worth interrupting the next game over.
- **Preferring the end-of-game block over match history is a correctness decision, not a latency one.** The block is scoped to us already, so the outcome falls out of two booleans with no participant matching — and participant matching is precisely what had never worked (`extract_summary` joined on a `puuid` the endpoint does not send, so every fetch would have failed to *deserialize*). Where both sources answer, the block wins, because it cannot have matched the wrong player. Where they contradict what Live Client Data recorded, that is logged loudly rather than silently written: the two are views of one game, so a disagreement almost certainly means the wrong `gameId` was matched.
- **Champion id → name comes from the client, not from Data Dragon.** What the resolver produces has to be byte-identical to what Live Client Data writes, because `champion` is sorted on, filtered on and used as the card title: `MonkeyKing` and `Wukong` in one library is one champion in two places, and the split is invisible until somebody notices half their games are missing. The asset store is served by the client we are already authenticated against, on the patch that client is running — it cannot go stale, it needs no network, and it is up by definition whenever the patch runs, so a remote CDN and a version to pin would be a dependency bought for nothing. `alias` is the field that must not be read: it is exactly where those legacy spellings live. The map is fetched once per client session and cached against the lockfile, so a restart onto a new patch re-fetches rather than serving a table missing the champion released that morning. **This is a decision about names.** Champion *art* is a different question — a CDN is a fine place to keep images — and answering it does not disturb this.
- **One display name, several ids.** The real store carries a parallel `Jade_*` block in the 60000s: `Jade_Wukong` is id 60062 and its `name` is `Wukong` too, exactly like id 62. Reading it as id → name is unaffected, because both ids genuinely *are* Wukong and either is the right answer to "what was I playing?". What it rules out is the reverse map, and champion art is keyed the other way round — Data Dragon's image filenames are the champion's key, which is the LCU's `alias`, not its `name`. So art cannot be looked up from the `champion` column alone; #85 carries that.
- **Never match on `summonerName`, and never join on a zero id.** Display names are not unique and they change. `summonerId: 0` is what the LCU puts in the slot for a participant whose identity is hidden, so joining zero to zero would attach the first anonymous player in the list to the recording. Both would mislabel a VOD with a stranger's game, which is worse than leaving the metadata NULL.

### 3.2 Live Client Data API (in-game)

- `https://127.0.0.1:2999/liveclientdata/allgamedata` — no auth, only up while a game is running. A 3-second request timeout: reqwest applies none by default, and a stalled request on a loopback endpoint that normally answers in ten milliseconds is a hang, not a slow reply — it silently stopped markers while the recording carried on, because no error was ever returned to declare the endpoint down.
- **Failing to read a response is not the same as the game being gone**, and conflating the two cost a real game (#74). A payload we cannot parse proves the game is *running*; only a request that got no response at all means the process behind port 2999 has ended. The poller tolerates five consecutive transport failures before finalizing, and never finalizes on a parse failure. Events are parsed entry by entry so one unreadable event costs that event rather than the snapshot — the events array is the only part of the payload that both grows during a game and can fail to deserialize.
- Poll ~1 Hz. Relevant pieces:
  - `events.Events[]` — `ChampionKill`, `Multikill`, `TurretKilled`, `InhibKilled`, `DragonKill`, `BaronKill`, `HeraldKill`, `Ace`, `FirstBlood`, each with `EventTime` (seconds of game time). The neutral objectives also carry `Stolen`, which rides in the marker payload — and which is read leniently, because Riot has historically sent booleans in this API as the strings `"True"`/`"False"` and a bare `Option<bool>` rejected the whole snapshot over it. **Whether that is what actually broke #74 is not worth establishing**: the lenient read makes either answer survivable, and the LCU's post-game data could supply steals instead if the live value ever proves unreliable. Which source a steal flag comes from does not change anything the review player does with it.
  - **Not every event becomes a marker.** See "only events the player is named in" below.
  - `activePlayer.summonerName` / `allPlayers` — identify which events involve *us* (our kills/deaths vs. someone else's).
  - `gameData.gameTime` — for aligning game time to recording time.
  - `allPlayers[].championName` / `.scores` and `gameData.gameMode` — the library card's champion, KDA and mode. Taken here rather than from the LCU because the live API states the champion as a *name*, so nothing has to resolve a champion id, and because it works in Practice Tool and customs where match history does not.
  - The `GameEnd` event's `Result` (`Win`/`Lose`) — the only place this API states an outcome, and the whole of win/loss detection until `fetch_match_summary` is wired in.
- **Only events the player is named in become markers.** A marker is a seek target and a stop on the review player's `[`/`]` navigation, so the bar is not "did this happen" but "was this about me". `classify_event` keeps an event only when the recording player is its killer, victim, assister, acer or recipient; everything else is dropped at classification and never reaches the database.

  Being on the team that took an objective is explicitly *not* taking part in it. The filter reads the event's own name fields rather than team membership, because the complaint that prompted it was a friendly turret: reviewing your team taking a T1 while you were on the opposite side of the map is exactly the stop nobody wants.

  **Two alternatives were rejected.** Keeping the enemy's objectives (the "why did we lose that Baron" case) would have needed a team lookup through `allPlayers[].team` and still leaves the arbitrary question of which uninvolved events are interesting. Storing everything with a relevance flag and filtering in the review UI is strictly more recoverable, but costs a schema column and a UI control for a case nobody had asked for.

  **The cost is that it is irreversible per recording.** Live Client Data is gone the moment the game ends, so a marker not captured can never be recovered for that VOD. If the filter is later judged too aggressive, older recordings stay filtered; only new ones benefit. That is the accepted trade — a timeline nobody trusts because it is full of other people's turrets is worse than one that occasionally omits something.

  `FirstBrick` (the first turret of the game) is deliberately *not* handled even though it is in Riot's event list: the API emits an ordinary `TurretKilled` for the same structure under its own `EventID`, so the tracker — which dedupes on `EventID` — would let both through and the VOD would carry two markers a frame apart.
- **Why the summary accumulates instead of being read off the last poll:** `GameEnd` shows up on one poll and the game process routinely exits before the next, so the poll carrying the result is often the last that succeeds. `LiveSummary::absorb` lets newer values win but never gives a known one back for a `None`.
- **Timestamp alignment:** marker position in the VOD = event `EventTime` mapped through an offset between game time and video time. The offset is measured on every poll where `gameTime` is seen to **advance** — never on the first poll. Recording starts *on* the first poll, so its `elapsed` is ~0, and that poll lands on the loading screen where `gameTime` is a frozen `0`; measuring there yields offset 0 and places every marker one loading screen early. Waiting for the clock to move proves it is a clock, and makes `elapsed` naturally include the load. Re-measuring on each advancing poll (rather than latching once) also absorbs pauses, which freeze the clock while the video keeps rolling, and encoder frame drops, which skew a fixed offset over a long game. Markers and samples are therefore stored with `game_time_s` and mapped to video time **at finalize**, stamped with the alignment in force when they were observed; anything seen before the clock first moved falls back to the first alignment the recording proved, or to 1:1 if it never moved. Recording still starts before `gameTime` 0 in the normal case, so the offset is normally positive; a reconnect makes it negative. Implemented as `live_client::events::AlignmentTracker`. Rationale and diagram: [docs/recording-pipeline.md](docs/recording-pipeline.md#timestamp-alignment).

### 3.3 Fixtures

Every API response shape we depend on gets captured to `fixtures/` (JSON) the first time we see it, and the poller/state machine must be runnable in replay mode against fixtures. This is what makes the League integration, library and review layers developable and unit-testable with no League running at all. Practice Tool (30-second launch, on-demand kills/objectives) is the live-testing tool of choice — never iterate against real queued games.

**Capture is on by default until v1.0.** It was opt-in via `NINJA_RECORDER_RECORD_FIXTURES`, and that variable is now an *override* rather than a switch-on: unset means on, and only an explicitly falsey value (`0`, `false`, `off`, `no`) turns it off. The dev portal's Fixtures panel still flips it at runtime.

The reason is #74. Almost every shape this app parses was written by hand and has never been checked against a real client, and a payload the parser could not read ended a recording nine minutes into a game — with no copy of it kept, so the triage was archaeology on a samples table. `record` runs *before* the parse, so with capture on, the payload that broke something is on disk when you go looking.

What that costs: one file write per response, so roughly 1/s during a game. Each endpoint overwrites a single file rather than accumulating, so there is no growth — and because the Live Client Data events array is cumulative, the last capture of a game contains every event in it. Captured payloads carry the Riot IDs of all ten players, which is worth remembering before committing one as a fixture.

**Revert to opt-in for the v1.0 release** — `fixtures.rs`, `DEFAULT_ON_UNTIL_V1`.

### 3.4 Game state machine

```
Idle ──(lockfile appears)──▶ ClientRunning
ClientRunning ──(phase: InProgress | Reconnect)──▶ WaitingForGame
WaitingForGame ──(port 2999 responds)──▶ Recording   [Recorder::start]
Recording ──(phase: EndOfGame | 2999 gone)──▶ Finalizing [Recorder::stop]
Finalizing ──▶ ClientRunning
```

Rendered as a state diagram, with the actions each transition emits and the full edge-case table: [docs/recording-pipeline.md §2](docs/recording-pipeline.md#2-the-state-machine).

Implemented as a pure transition function (`state_machine::machine::StateMachine::handle`, 11 unit tests covering the edge cases below) driven by a thin async supervisor (`state_machine::supervisor::Supervisor`) that spawns/aborts the lockfile/gameflow/Live-Client-Data watchers per `Action` and calls `Recorder::start`/`stop`. The pure part is fully tested, as are the two pieces of the supervisor that hold real logic — `start_recording`/`stop_recording` against a stub recorder and an in-memory DB, and `RecordingSession::ingest`, which takes elapsed time as an argument so a full poll sequence (loading screen, pause, reconnect) can be replayed without a clock. The watcher-spawning glue around them is not tested — no League client is installed on the machine this was built on, so nothing here has touched a real LCU or Live Client Data connection yet. Closing that gap is [docs/windows-verification.md](docs/windows-verification.md).

Finalizing stops the recorder, time-aligns the collected markers, writes the `recordings` row plus its `markers` and `samples` (§4), enforces retention (§6) and emits `library-changed`. It writes `duration_s` from the session clock, `champion`/`kda_*`/`win`/`game_mode` from the Live Client Data summary the session accumulated while the game ran, and `game_id`/`queue` from the gameflow session read at `InProgress` (§3.1). It does **not** fetch the LCU's post-game summary inline — see §3.1 for why that cannot work — but it does hand the game's identifiers to `match_summary::patch`, which fills in `role` and `patch` once the client has them. That hand-off goes through a type-erased notifier installed from `lib.rs`, for the same reason `on_event` does: `stop_recording` is directly unit-tested, and a bare `tauri::async_runtime::spawn` in it would drag the Tauri runtime into a code path `cargo test` executes. The tests leave it unset, so nothing spawns. The last finalized recording is also held in memory and exposed via `game_state_status`, so a failed DB write doesn't lose it.

Edge cases handled by the pure transition function (see its tests): game crash mid-match (Live Client Data stops responding), client crash (lockfile disappears) at every stage, reconnect to an in-progress game (state machine has no memory of *how* it entered `WaitingForGame`, so a reconnect behaves identically to a fresh game start — recording begins once Live Client Data becomes reachable, later than a from-the-start recording would), practice tool (goes through the same `Reconnect`/`InProgress` phases as a real game), dodges/cancelled champ select (bounces `WaitingForGame` back to `ClientRunning` without ever recording), and a client restart mid-finalize (picked up correctly regardless of ordering against `FinalizeComplete`).

Two edge cases from the original list are *not* verified: **spectator mode** — the state machine simply doesn't special-case any phase name beyond `InProgress`/`Reconnect`/end-of-game ones, so if gameflow reports a distinct phase while spectating, it won't trigger recording; but if it turns out spectating also reports `InProgress`, this would incorrectly record it, and that can only be confirmed live. **Machine sleep** — not simulated in this environment at all; the poller's backoff and lockfile-watch would likely eventually recover state after wake, but this needs real testing on the Windows machine ([docs/windows-verification.md](docs/windows-verification.md)).

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

Implemented in `src-tauri/src/db/` (`Db` + `reconcile`), migrations via `rusqlite_migration`, `rusqlite`'s `bundled` feature so no system SQLite is required on a fresh machine. Reconciliation runs once at app startup and on demand (`rescan_recordings` command). The state machine's Finalizing step (§3.4) writes a `recordings` row + its `markers` on every stop. `duration_s` comes from the session clock, read *before* the recorder is stopped so the ffmpeg remux isn't counted as footage. `champion`/`kda_*`/`win`/`game_mode` come from Live Client Data and `game_id`/`queue` from the gameflow session, both captured during the game rather than fetched after it. `role` and `patch` arrive last, from `match_summary::patch` seconds to a minute after the finalize (§3.1). That patch is a plain `UPDATE` and never a re-`insert_recording`: the upsert takes `pinned`, `size_bytes`, `started_at` and `duration_s` from `excluded`, so re-upserting a summary would unpin the recording and zero its size. Every column it writes COALESCEs so a value the LCU could not establish never erases one the live client did — except `champion`, which COALESCEs the other way and may only be filled when NULL. Two writers reach that column — the live client during the game, and the id `lcu::champions` resolves afterwards — and both aim at the same display name (`Wukong`, never the internal `MonkeyKing` alias). Filling only when NULL means they cannot disagree *in the column* even if they ever disagree with each other, and one champion under two spellings would split its games in two wherever the library sorts and filters.

### 4.1 Decision: imported files get their duration from ffmpeg, not ffprobe

`duration_s` comes from the session clock for recordings this app made. Rows
`reconcile` imported have no session — it knows only the path, the size and
the mtime — so their `LENGTH` stayed `—` forever and they kept counting toward
the "N unknown" sub-label on the Recorded tile.

The obvious tool is `ffprobe -show_format`, which answers this in clean JSON.
**We don't ship it.** CI stages exactly one binary into the bundle,
`ffmpeg.exe` (`.github/workflows/ci.yml`, "Stage ffmpeg for faststart remux"),
and adding ffprobe would roughly double that download to obtain one number.

So the probe runs `ffmpeg -hide_banner -i <file>` with no output file. ffmpeg
prints the container header to **stderr**, then exits non-zero complaining
that no output was specified — so the exit status is ignored and stderr is
parsed for the `Duration: HH:MM:SS.ss` line.

That is prose-scraping, which this project otherwise avoids and which is
exactly the objection raised against reading capture health out of
`logs/libobs.log` (#81). The difference is the blast radius. There, a misparse
would put a wrong encoder name into a diagnostics report someone trusts; here
every failure path returns `None` and the column stays NULL — "unknown", which
is what it already said. A duration of zero is treated as unknown too, since a
truncated or still-growing file would otherwise render as a confident `0:00`.

The parse is a pure function (`probe::parse_duration_s`) with the spawn as a
thin wrapper over it, so the fragile half is unit tested on a box that has no
ffmpeg at all. The catch is that the same absence means the fixture it is
tested against is **written from ffmpeg's documented format, not captured from
a run** — so the tests pin the parser's behaviour, not the wording's accuracy.
Confirming the real wording is a `docs/windows-verification.md` item.

**Cost:** one subprocess per imported file, on the import branch only —
`reconcile` skips paths that already have rows, so a settled folder spawns
nothing on rescan. The case that costs is a first run against a large existing
folder, and startup reconcile is inline in `lib.rs`'s `setup`, so that is
startup latency. Header-only reads are milliseconds each; if it ever becomes a
problem the fix is to move the import loop off the startup path, not to drop
the probe.

---

### 4.2 Decision: the backfill matches on the clock, and refuses ties

Every column #49 added only ever gets filled for recordings made after it
shipped. Older rows keep their NULLs forever, and in a real library those are
most of the rows — which makes the win-rate tile wrong for as long as they
dominate. `reconcile`-imported files have the same problem for the same reason.

**There is no game id to ask about.** One is captured *during* the game, from
the gameflow session (§3.1), which is exactly what these rows never had. So the
only handle left is time: the recording ran from some instant for some length,
and so did a game. `backfill::match_recording` compares those two windows and
accepts a game when it overlaps at least half of the shorter one.

Half, rather than any overlap or near-exact agreement, because neither extreme
is right. A recording brackets the loading screen and the client's
`gameDuration` does not, so they never agree exactly; and consecutive games in
one session can brush each other at the edges, so any overlap would match the
wrong one.

**Two matches means write nothing.** Not "pick the better one" — the ranking
would be invented, and the failure it protects against is silent. A card
labelled with the wrong game looks exactly as plausible as a correct one, and
nobody re-checks a row that looks fine, so the error would live in the library
permanently. `—` is recoverable; a confident lie is not. This is the same rule
`match_data` applies to participant identity and `team_diff` applies to team
side: when the answer is not certain, the column stays NULL.

**Manual, never on startup.** It is a bulk read against the user's running
client, and the moment to do that is theirs to pick — not something that fires
while they are loading into a game. It is also cheap: the match-history list
response carries whole game documents, so the pass costs one request for the
history and one for the summoner regardless of how many rows it labels.

It adds no new writer. The matched game goes through `to_metadata`,
`champion_name` and `update_match_metadata` — the same three the deferred patch
uses, including the same "champion only when NULL" rule.

What it cannot do: reach further back than the client's own match history, or
label a custom game, which never gets a match-history entry at all. Both come
back as "matched no game", which the report says in as many words rather than
leaving the user to guess why nothing happened.

## 5. Review player

- WebView2 `<video>` element — H.264/AAC MP4 decodes natively, so seeking and playback rate are free.
- Custom timeline component: marker glyphs per event kind, click-to-jump, "next death" / "prev death" hotkeys.
- Later: clip export (`ffmpeg -ss .. -to .. -c copy` — stream copy, no re-encode; ship a minimal ffmpeg binary or use libobs's muxer).

Implemented in `src/review.ts` + `index.html`'s `#review-view`. The video loads via Tauri's asset protocol (`convertFileSrc`, scoped in `tauri.conf.json` to `$APPDATA/recordings/*` — needed the `protocol-asset` Cargo feature, not just config).

**The player chrome lives inside `.player-wrap`, and that placement is a constraint rather than a style choice.** `requestFullscreen` is called on `.player-wrap`; the Fullscreen API renders only the fullscreened element's subtree, so a control bar that is a *sibling* of the video is not drawn at all in fullscreen — which is exactly how it behaved. Moving the bar inside the frame is the only fix; no amount of CSS reaches an element outside the subtree. The `:fullscreen` rules in `styles.css` are the other half: without them the embedded `height: auto` still applies and the video renders as a band across the middle of a black screen. Embedded, the element is sized by the recording's own aspect ratio and nothing else — it was capped at `max-height: 60vh`, which kept the full width and made up the difference in black bars above and below, growing and shrinking them as the window was resized. The page scrolls instead; the window's default height (§12) is chosen so the player and the timeline both clear the fold. The rich `#vod-timeline` is deliberately left outside, so it is unavailable in fullscreen — duplicating the metric graph, glyphs and ruler into the overlay would be a second implementation of the most intricate widget in the app, and the `[` / `]` / `d` / `D` hotkeys already cover marker navigation there.

**Frame-stepping was dropped** along with that rework. It was a ±1/30s time nudge rather than a true frame seek — no per-recording frame rate is probed anywhere — so it was an approximation presented as precision, and it cost two buttons in a control bar that had to shed width to fit inside the frame. Closely-spaced markers (common near a teamfight) collapse into a single cluster glyph rather than colliding, with `MARKER_PRIORITY` deciding which icon the cluster shows. The library grid, filters, sort, and the stats bar above them are all client-side over the already-fetched row set — fine at solo-user library sizes, would need real pagination/querying if that stops being true.

Verified: layout/CSS visually in a browser (with injected mock data, since a plain browser tab has no Tauri IPC bridge to exercise real `invoke` calls) and a full `cargo tauri dev` launch (asset-protocol config + new `get_recording_markers` command, no capability/schema errors, stable). **Not verified**: the hotkey→seek interaction against real marker data (needs a loaded recording, which needs a live app session to click through manually) — and that gap did hide a bug. The hotkey handler stands aside for a focused form control so it does not steal a key the control uses itself, but it applied that to Space *and* the arrows, for any `<button>`. A timeline glyph is a button, so clicking one to jump left it focused and killed seeking entirely until you clicked elsewhere. A `<button>` does nothing with the arrows, so the exemption bought nothing there: Space now stands aside for a button or a select, the arrows only for a select, and a glyph no longer takes focus from a pointer at all. Video playback itself is now testable — `fixtures/sample.mp4` is checked in (§10), and the dev portal's Review-ready seed preset builds a recording around it.

### 5.1 App shell, theming and settings

The frontend is vanilla TS with no framework, split by state ownership
rather than by widget: `router.ts` owns which view is showing, `theme.ts`
owns `<html data-theme>`, `prefs.ts` owns the preference cache, `status.ts`
owns the poll timer, `library.ts` owns the row set and filters, and
`settings.ts` owns the settings form. `main.ts` is a composition root that
owns nothing. `dom.ts` and `format.ts` hold shared primitives — including
`escapeAttr`, which matters because `reconcile` imports any video file the
user drops in the folder, so a recording's displayed name is not
necessarily ours.

**Theming.** `data-theme` is written by JS and only ever holds `"light"` or
`"dark"`; there is no `prefers-color-scheme` media query in the stylesheet.
Resolving the OS preference once, in one place, keeps a single dark block
instead of two and makes an explicit "Light" on a dark OS win by
construction rather than by CSS specificity. The cost is that "System" no
longer follows the OS for free — `theme.ts` listens on the matchMedia
`change` event to put that back, and removing that listener is a silent
regression with no test to catch it.

**The window is not a page.** A webview brings the whole browser with it, and
most of what it brings is meaningless here: dragging across a card leaves half
of it highlighted, right-click offers to reload the app or save the video, F5
throws the UI away mid-recording without the backend hearing about it, and
icons peel off under the cursor as drag images. `desktop.ts` and one CSS block
suppress that. The split is not arbitrary — only CSS can hand selection back
per element (`.selectable`, plus form fields, `code` and `.mono`, because text
the user typed or might want to copy is the one kind worth keeping), and only
JS can see the events.

Both halves are narrow by construction: they suppress browser chrome, never app
behaviour, and each suppression names the one case where it would be a
regression. Text fields keep their context menu, because there it is the
ordinary Cut/Copy/Paste menu a desktop app would show anyway. A build you can
inspect — the vite dev server, or anything with the `devtools` Cargo feature —
keeps the native menu and the reload key outright, since "Inspect element" and
a reload are the two things most worth having while working on the frontend.
That check reuses `devportal.ts`'s existing probe (`hasDevCommands` in
`bridge.ts`, memoised) rather than adding a second flag that could disagree
with the Rust side.

Preferences live in `settings_kv` (migration 4), a deliberately unseeded
key/value table: a missing pref means "use the frontend default", so adding
one needs no migration. They also mirror into `localStorage` for exactly
one reason — the inline boot script in `index.html` has to pick a theme
*synchronously*, before first paint, and IPC resolves too late. SQLite
stays the source of truth and wins any disagreement.

**Idle while hidden.** Both of the frontend's continuous costs are now tied to
window visibility: an open VOD is paused (and with it the rAF playhead loop and
the stem `<audio>`), and the 60 s library safety refresh is skipped. Neither is
free to leave running behind a minimised window, and the second rebuilds the
whole grid with `innerHTML`. Note this leans on `document.hidden`, which is
reliable for a minimised window but **not guaranteed** for a window hidden via
`window.hide()` — when the tray work lands, visibility has to be pushed from
Rust instead.

**Status polling.** There are no Tauri events anywhere in this app; every
backend→frontend signal is pull-only. The header's live state therefore
comes from a `setTimeout` chain (not `setInterval` — `lcu_status` reads a
lockfile and makes two HTTPS round trips, and a slow tick would stack
calls). The interval scales with game state, and the library refreshes
itself off the `Finalizing` edge and `last_finalized.path` rather than
polling `list_recordings`, which would rebuild the grid every couple of
seconds and fight scroll and focus. If Tauri events are ever added on the
Rust side, this whole file becomes a subscription instead.

**No dev panel.** The stub start/stop, LCU check and game-state buttons are
gone. The information they exposed is now always on — the header strip, and
a read-only About block in settings carrying summoner, phase, state, and
the last finalized path with its marker count and `DB WRITE FAILED` signal.
`start_recording` / `stop_recording` / `is_recording` stay registered as
commands (unreferenced from the frontend): `start_recording` carries the
`has_room_to_record` preflight, and dropping them would make
`chrono_stamp` dead code, which fails CI's `clippy -D warnings`.

---

## 6. Disk management (launch feature, not a later one)

1080p60 @ 8 Mbps ≈ **3.5 GB/hour**. A ranked session ≈ 15 GB. Without retention, we fill the user's SSD in two weeks and get uninstalled.

- Retention policy: max total size AND max age, whichever bites first; `pinned` recordings are exempt.
- Enforce on app start and after each `Finalizing`.
- Show current usage in the UI; never delete without the policy being visible to the user.

Implemented in `src-tauri/src/retention.rs`, following the same pure-function-plus-thin-I/O-wrapper shape as `db::reconcile` and `state_machine::machine`: `select_for_deletion` is a pure decision (no I/O, unit-tested directly) over a `RecordingRow` slice + `RetentionPolicy` + an injected "now," and `enforce`/`enforce_now` apply it against the real DB and filesystem. Age is checked first (anything over the limit goes regardless of size), then size (oldest non-pinned recordings removed until under the cap) — usage totals include pinned recordings' bytes (they still occupy disk), but only non-pinned rows are ever deletion candidates.

The policy itself lives in a single-row `settings` table (`db/mod.rs`'s second migration), defaulting to 50 GiB / 30 days rather than unlimited — this is meant to protect the user out of the box, not only once they find a settings screen, matching this section's "launch feature, not a later one." Either limit can be turned off independently (`NULL` = unbounded) from the settings view's Storage section; current usage sits in the library's stats bar so nothing is deleted as a surprise. `preview_retention_policy` runs `select_for_deletion` as a dry run while the form is being edited, so a tightened limit says what it will delete *before* it is saved, and `set_retention_policy`'s `EnforcementReport` — previously returned and discarded — is now shown after it does. Enforcement runs at app startup (`lib.rs`'s `setup`, after reconcile) and after every finalize (`state_machine::supervisor::stop_recording`), plus immediately when the policy is changed from the UI (`set_retention_policy`) so a newly-tightened limit doesn't wait for the next finalize to take effect.

Record-start preflight (`retention::has_room_to_record`, via the `fs2` crate — std has no free-space API) refuses to start a new recording under 1 GiB free on the recordings volume, checked from both `Supervisor::start_recording` (the real path) and the dev panel's manual `start_recording` command. Fails open on a stat error rather than block recording over a check that couldn't even run.

Pinning is wired end-to-end: the library's 📌 calls `set_pinned` and refreshes. Recordings can also be deleted individually via `delete_recording`, which shares `retention::delete_recording_and_file` with the automatic sweep — the two differ deliberately on a file that won't delete: a user-initiated delete reports the failure and leaves the row alone, while `enforce` logs and drops the row anyway so an unattended sweep can't stall.

---

## 7. YouTube upload (designed, not built)

- YouTube Data API v3, OAuth 2.0 **desktop flow with loopback redirect** (the OOB flow is dead). Store refresh token in Windows Credential Manager / Keychain, not in the DB.
- **Quota reality:** an upload costs 1,600 units; default project quota is 10,000/day → **~6 uploads/day across all users** until Google grants a quota increase (requires an audit). Design consequence: upload is a deliberate per-VOD action with clear failure messaging, never auto-upload.
- Resumable upload protocol is mandatory (multi-GB files, flaky connections).
- Unlisted by default.

## 8. ROFL replays (designed, not built)

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

- **Run the app with `npm run tauri:dev`**, not `cargo tauri dev` — it passes `--features devtools`, which is what compiles in the dev portal (§10). Without it the portal's window and every `dev_*` command are absent, and the main window hides its own "Dev portal" button accordingly.
- **Never cross-compile the Windows build from macOS.** libobs linking + DLL bundling + installer generation via `cargo-xwin` is a fight with no payoff. GitHub Actions `windows-latest` builds the installer (NSIS); download the artifact.
- Vanguard verification (capture works during a real Vanguard-protected game, no flags) is a one-time check per significant capture change, not an iterative loop — capture iterates against any window (browser, video loop), no League needed.
- **CI** ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) — one workflow, four jobs: `test`, `version`, `build`, `release`. The job graph, the staging steps for the libobs runtime, and the release flow are documented in [docs/ci-and-releases.md](docs/ci-and-releases.md). The decisions worth defending here:
  - `test` runs the Rust half **twice**, with and without `--features devtools`. An off-by-default feature is otherwise never compiled by CI, and a broken `#[cfg]` would stay green until someone opened the portal.
  - macOS was dropped from `test` — the work is platform-independent and GitHub bills macOS runners at 10x against the free plan. `build` still compiles macOS natively, so cfg'd breakage is still caught before a release.
  - **Pull requests run `test` only.** Three Tauri bundles, two of them Windows, are the overwhelming majority of this workflow's minute spend and artifact storage, and nothing consumes a PR's bundles. A branch that needs an installer can get the full matrix from `gh workflow run ci.yml --ref <branch>`.
  - `build` deliberately does *not* `needs: test`. The two share no output, and gating cost the whole test job in latency before the slow Windows bundle even started. Nothing unreviewed escapes, because `release` needs both.
  - `version` is the commit's distance from the newest real tag, not "highest seen plus one" — a pure function of the commit, so simultaneous pushes cannot claim the same version and re-running a commit updates its own release rather than minting a second.
  - `release` publishes rather than drafts, because `needs: [version, test, build]` already withholds it until the commit's tests pass — "published" therefore means "tested", and a human clicking Publish added latency rather than a check. Publishing creates the tag, which becomes the base `version` counts from next time.

  Neither build is code-signed yet (no cert configured) — Windows SmartScreen and macOS Gatekeeper both warn on first run. macOS builds only exercise the stub recorder; they're a dev/testing convenience, not a shipping target (§1.1).

---

## 10. Dev portal

A second window (`dev.html`) that exercises the whole backend: every command, every table, the retention decision, and the state machine — none of which the app's own UI can reach. It replaces the three-button `#testing-view` that used to live in `index.html`.

**It is compiled out of shipped builds.** The `devtools` Cargo feature is off by default, and everything in `src-tauri/src/dev/` plus the `dev.html` Vite entry is behind it. `npm run tauri:dev` turns it on; `npm run build` cannot even emit `dev.html` (`vite.config.ts` gates the second rollup input on `NINJA_DEVTOOLS`). Availability is detected, not configured: the main window calls `dev_open_portal` and hides its button when the command isn't registered, so there is no second flag to keep in sync.

Why it exists, concretely — each of these was untestable before:

- **No way to insert data.** There was no seed script anywhere, so the library, its filters and sort, retention, and the entire review player could only be exercised by finishing a real game on Windows. The Seed panel writes real files, rows, markers with the payload shapes `classify_event` produces, and a 1 Hz advantage curve. Retention fixtures use sparse files, so a 3 GiB recording costs a few hundred bytes of disk.
- **The supervisor was only drivable by real League polling.** §3.4 notes its async glue has never touched a real LCU. The Simulate panel dispatches `StateEvent`s into the live supervisor (really starting and stopping the recorder), injects Live Client Data payloads through the real `MarkerTracker`, and replays a scripted game at a speed multiplier until it finalizes into a real row. This is the fixture replay mode §3.3 asked for.
- **Retention deleted files with no preview.** `set_retention_policy` saves *and* enforces. `select_for_deletion` is pure and takes an injected clock, so the Retention panel dry-runs it — including at a fabricated "now", to test an age rule without waiting days.
- **The in-flight recording session was invisible.** `game_state_status` only carries the *last finalized* recording; markers and samples accumulating during a recording could not be observed at all. `dev_session_snapshot` exposes them.
- **`fetch_match_summary` is implemented, unit-tested, and called from nowhere** — which is why every `RecordingRow`'s `role` and `patch` is NULL in practice. `champion`/`win`/`kda_*` come from Live Client Data during the game (§3.2), and `game_id`/`queue` from the gameflow session (§3.1). The portal at least makes the fetch runnable against a real client; wiring it into finalize is still open.

Two changes leaked usefully out of the portal into the app proper. `Supervisor` now emits a **`library-changed`** event after a finalize (and `set_retention_policy` after a deletion), which `src/main.ts` listens for — the first backend-to-frontend push in the codebase, and it fixes the standing bug where a recording that just finished stayed invisible until the user pressed Refresh. And `fixtures::enabled()` is now an `AtomicBool` seeded from `NINJA_RECORDER_RECORD_FIXTURES` rather than a per-call env read, so capture can be toggled at runtime instead of only at launch.

`tauri.devtools.conf.json` renames the product and binary to `ninja-recorder-dev` so it is a separate application to Windows. NSIS keys the uninstall entry, the default install directory and the shortcut off `productName`, so while the two shared one, this installer treated the real install as an older version of *itself* and uninstalled it first — a step that aborts the whole install with "Unable to uninstall!" if the old uninstaller returns non-zero or leaves the binary behind (a still-running app is enough). `mainBinaryName` splits the process name too, so neither build's "close the running app" check reaches across at the other; they install side by side. The `identifier` is deliberately *not* overridden, so the portal still opens the library the real app writes to.

**Panels, how to get a build with it, and its known limits** — including why the TS command registry is hand-maintained and why seeded placeholder files won't decode — are in [docs/dev-portal.md](docs/dev-portal.md).

---

## 11. Risks

| Risk | Mitigation |
|---|---|
| Riot changes LCU/Live Client endpoints | Unofficial-but-tolerated APIs; fixtures + thin client layer localize breakage. Watch league_record and lcu-driver communities |
| Vanguard behavior changes re: WGC | WGC is a core OS compositor API used by Xbox Game Bar itself — lowest-risk capture path that exists. No fallback plan needed beyond display capture |
| libobs Rust bindings immaturity | Using a patched fork of `libobs-recorder` (§2.1) rather than raw bindings — but it's still a young, single-maintainer ecosystem and now a fork we own the patch for. Budget time; fallback is a thin C shim over the (stable, C) libobs API. The trait keeps this contained |
| Our `libobs-recorder` fork falls behind upstream | The patch is now two commits, not one (capture source + muxer settings, then multi-track audio §2.5), and the second one touches encoder/source lifetime rather than just settings — so a re-base is no longer free. Still small and self-contained; watch for upstream libobs version bumps we might want (new encoders, bug fixes) |
| Per-app audio capture doesn't work for League | `wasapi_process_output_capture` needs Win10 2004+, is still flagged beta in OBS 30.x, and has never been tried against a Vanguard-protected process. Every preset naming "game audio" depends on it (§2.5). Fallback is the Desktop preset, which uses ordinary loopback — documented rather than automatic, so a silent game track is diagnosable |
| Stem playback drifts out of sync | The review player syncs a sidecar `<audio>` against the video by hand (§2.5). Bounded blast radius: playback only, over a file that already exists, fixed by reopening the VOD. Track 0 — the default — never uses this path |
| YouTube quota audit friction | Ship upload as "bring your own consent" early; apply for quota increase well before it matters |
| Disk-full during recording | Preflight free-space check at record start; stop gracefully + notify rather than corrupt |
| Window mode edge cases (exclusive fullscreen) | WGC needs a composited surface. Detect and nudge user toward borderless (the League default) |

---

## 12. Process model: a recorder daemon and a UI that can leave

The app records unattended, so its natural resting state is running with no
window. That is at odds with a single process whose command surface only
exists inside a webview.

**Decision: one binary, two modes.** `ninja-recorder --daemon` owns the
`Supervisor`, the database, the `Recorder` and the tray icon, with no windows
at all. `ninja-recorder` with no arguments is the UI: a window that attaches to
the daemon over a local socket and can exit without stopping a recording. One
binary rather than two because CI's build matrix bundles a single artifact per
entry — a second `[[bin]]` would need an `externalBin` entry and its own NSIS
story, and a flag needs neither.

**Be honest about what this buys.** Not much memory. A *hidden* window keeps
WebView2 fully resident, so "minimise to tray" reclaims nothing on its own, and
simply destroying the window in a single process would capture most of the
remaining win. While the UI is open, two processes cost *more* — a second host
process. What the split actually buys is **crash isolation**: today a WebView2
crash, or the WebView2 Runtime auto-updating underneath us, takes down an
in-progress recording. It also lets the UI be genuinely absent rather than
merely invisible. The idle-RAM win that matters came from §2.2's capture-backend
lifecycle, not from here.

### The `core` module is the precondition

Tauri v2 has no way to invoke a registered command by name from Rust —
`generate_handler!` only dispatches from a webview's IPC. A daemon therefore
cannot reuse `#[tauri::command]` functions at all.

So the logic moved into `src-tauri/src/core/`, as free functions over a plain
`Ctx { recorder, supervisor, db, recordings_dir, ffmpeg }`, and `lib.rs` keeps
only thin wrappers. Three commands used to take an `AppHandle` purely to
re-derive `recordings_dir` / `ffmpeg_path` on every call; both are now resolved
once at startup into `Ctx`.

**`core` must never name a `tauri` type.** Same rule, and the same reason, as
`Supervisor::on_library_changed` (§3.4): this module is unit-testable, and
making Wry reachable from a module with tests drags the Win32 GUI stack into the
`cargo test` binary, which carries no application manifest, so `comctl32`
resolves to v5 and the binary dies at load with `STATUS_ENTRYPOINT_NOT_FOUND`.
The one command that emits a Tauri event does it through a type-erased closure
handed in at startup.

`AppState` is consequently a newtype that `Deref`s to `Ctx`, which is what keeps
the dev portal's many `state.db` / `state.supervisor` field reads compiling
unchanged.

**Two commands deliberately did not move**: `open_recordings_folder` and
`dev_open_portal`. Both drive the desktop shell — an opener call and a window —
and an Explorer window launched from a background daemon can open behind the
foreground app. They stay in the UI process, which only needs `recordings_dir`
to do its job.

### The `rpc` passthrough

Built. `bridge.ts` sends every production command through one Tauri command,
`invoke("rpc", { command, args })`, and `core::dispatch` routes it by name. The
UI now registers two commands instead of twenty-three, and
`dev_registered_commands` derives its list from `core::command_names()` — the
same macro invocation that generates the `match` arms — so the Rust half of the
drift check cannot go stale. Adding a command is two edits (a table row and
`src/dev/registry.ts`) rather than four.

Three commands stay directly registered: `open_recordings_folder` and
`dev_open_portal` drive the desktop shell, and `dev_registered_commands` has to,
because `devportal.ts` detects whether the portal exists by watching that call
reject in a shipped build — routing it through `rpc` would make it reject in
*every* build and permanently hide the button.

**The cost, stated plainly.** `#[tauri::command]` used to generate argument
deserialization, camelCase→snake_case mapping included. The passthrough owns
that now, and a wrong name or type is a runtime failure rather than a compile
error. Two things hold it down: the table is the only place it's written, and
`every_command_round_trips` exercises every entry with the payload the frontend
really sends. Note the rename applies to *argument names* only — types nested
inside an argument keep their own serde attributes, and because those fields are
usually `Option`, a mis-cased nested key is silently dropped rather than
rejected. That was equally true of the Tauri macro, but it is worth knowing when
adding a nested argument type.

`dispatch` is async because `lcu_status` is; everything else is blocking work,
so `rpc` sends it to `spawn_blocking` and only awaits the one command that needs
it. `core` itself still names no async runtime — callers choose the thread.

### Launch modes and the window

`main.rs` reads a `Launch` mode out of argv before anything else:
`--daemon` (reserved), `--hidden`, or nothing. Parsing lives in
`src-tauri/src/launch.rs`, pure and unit-tested, and names no `tauri` type.

**The flags are an on-disk contract, which is why they were fixed before the
tray existed.** `tauri-plugin-autostart` writes the flag into `HKCU\…\Run`
once, at enable time; a flag that changes meaning later silently strands every
user who turned autostart on before the change. That is no longer hypothetical
— "Start on login" below registers `--hidden` — and `--daemon` is therefore
recognised but *rejected with a message and exit code 2* rather than falling
back to a normal window: a build that quietly ignored it would look like it
worked while recording nothing.

**The main window is created in Rust, not by `tauri.conf.json`.** `app.windows`
is now `[]`. Tauri creates entries in that array automatically, before `setup`
runs, so there was no way to *not* have a window — and `"visible": false` is
not a substitute: it still constructs the WebView2 instance and pays its full
cost, which is exactly what starting in the tray is meant to avoid. The label
stays `"main"` so `capabilities/default.json` matches unchanged.

Building it from `setup` is safe. The hazard `dev_open_portal` documents —
building a window re-entrantly from inside a WebView2 IPC callback yields a
blank window — applies to windows created from a command, which `setup` is not.
A window created later from a tray click will have to respect it.

**The default size is derived from the frontend, not picked by eye.** The
content column stops at `--content-max: 1120px`; add the container's padding
and room for a scrollbar and 1200 is the narrowest inner width at which it
reaches full width, so anything narrower squeezes every view and anything wider
only adds background. The 900 height clears the review player and its timeline
— the marker list under them is left to scroll, because a window tall enough to
show it as well would not fit on a 1080p desktop.

Verified on macOS against the real binary: a default start registers a GUI
window, `--hidden` starts with none and stays running, `--daemon` exits 2, and
unknown arguments are ignored rather than fatal (both OSes hand launched apps
arguments we never asked for). The windowless run also confirms Tauri's event
loop survives with no windows, which is the daemon's prerequisite.

### The tray, and what the close button does

The tray is the app's resting state: three items — Open ninja-recorder,
Settings, Quit. A "Start/Stop recording" item was considered and rejected,
because `start_recording` races the state machine, which doesn't know about the
call (`Supervisor::start_recording` spells out the divergence); promoting a
known-broken dev affordance into the product is not a feature.

**Close is a preference, defaulting to "close the window".** The three values
are `close-window`, `hide` and `quit`, in `settings_kv` under `closeAction`,
parsed by `core::CloseAction` — which falls back to the default on anything it
doesn't recognise, because that table is schemaless and shared across versions,
so a downgrade will one day read a value written by a newer build.

The default is `close-window`, not `hide`, and the difference is the whole
point: a *hidden* window keeps WebView2 fully resident and reclaims nothing.
Destroying the webview while the process lives on is what actually gets the
footprint down, and the recording is unaffected either way. `hide` stays
available for instant reopening. Measured on macOS: a window costs 4 WebKit
handles, `--hidden` costs 0.

Keeping the process alive after its last window closes is
`RunEvent::ExitRequested`. The discriminator is the exit code: `None` means
user interaction — here, the last window closing — and gets vetoed with
`api.prevent_exit()`; `Some(_)` means a programmatic `AppHandle::exit`, which
is how the tray's Quit gets out. No "am I quitting?" flag is needed.

**Quit finalizes first.** `Supervisor::finalize_for_shutdown` runs the same
finalize the state machine does, so quitting mid-match writes the row and its
markers instead of leaving a fragmented MP4 for the next startup's `reconcile`
to adopt without them. It runs on a blocking thread, never the main one: tray
menu handlers run on the main thread, and that finalize includes an ffmpeg
remux and a retention sweep — inline it would freeze the tray and every window
for seconds.

**The tray's "Settings" has two paths** because the window may not exist. A
live window gets a `navigate` event; a cold one is created at
`index.html#settings`, since a frontend that hasn't loaded cannot be listening
for an event yet. `router.ts`'s `initRouting` handles both, and deliberately
ignores a `#review` fragment — the review view with no recording loaded is not
a state worth restoring into.

`tray.rs` has **no tests and must not grow any**, for the reason
`state_machine::supervisor::on_library_changed` documents: it is reachable only
from `run()`, which is dead code in a test build and gets stripped, keeping the
Win32 GUI import stack out of the test binary. The one testable thing —
`CloseAction` parsing — lives in `core`, which names no `tauri` type.

### Start on login

Recording unattended is worth very little if the user has to remember to launch
the recorder first. `tauri-plugin-autostart` registers the app under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (a LaunchAgent on
macOS, a `.desktop` entry on Linux — which is what makes the whole thing
exercisable in the macOS dev loop), and it is registered with `--hidden`, so a
login start costs a tray icon and no webview at all.

**`--hidden`, not `--daemon`.** The daemon is still reserved and exits 2, so
registering it would produce a login start that launches nothing and records
nothing, with no console to say why. This is the reason the flags were fixed
before the tray existed: the string goes into the registry once, at enable
time, and the build that reads it back may be years newer. `launch.rs` owns
both flags as constants and has a test pinning their exact spelling — renaming
the constant is free, changing its value strands every machine that already has
the old one.

**Off until asked for, and never written by the installer.** Nothing registers
at install or first run; the entry appears only when the settings toggle is
turned on. An app that quietly adds itself to startup is one the user finds in
Task Manager and uninstalls.

**The registry is the source of truth — this is the one setting not mirrored
into `settings_kv`.** Every other preference is ours alone, but this one has a
second owner: the user can delete the entry from Task Manager's Startup tab, and
policy or another install can remove it. A cached copy in SQLite would be a
checkbox confidently describing a login start that will never happen, so
`get_autostart` reads the platform live and `set_autostart` **re-reads after
writing** and returns that, not what was asked for. A `Run` write can be
overruled; reporting success on "the call didn't error" is how the checkbox
starts lying.

`core` can't name an `AppHandle`, so the plugin sits behind a `core::Autostart`
trait implemented in `lib.rs` — the same seam shape as
`set_library_changed_notifier`. `Ctx::new` leaves it `None`, which does double
duty: the commands are unit-testable against a fake, and **`cargo test` cannot
reach a real registry**. A test that ran `set_autostart` for real on a
developer's Windows box would leave that machine launching the app on every
login, so `None` refuses the write rather than defaulting to the live one.

### Notifications

The window is closed most of the time, so a recording that saved — or failed —
has nowhere to show up. Four kinds, in `settings_kv`, all defaulting sensibly
so none needs a migration: **finished** and **failed** on, **started** off (the
user is about to be in a game and does not want a popup over it), plus a
one-time "still running in the tray" notice the first time the window is
closed. A master switch silences everything including that notice, because off
has to mean off.

"Reset one-time notices" **blanks** the key rather than deleting it: `set_ui_pref`
only writes, and adding a delete command would mean editing the dispatch table
and the dev registry for one button. So an empty value counts as unseen. There
is a test pinning that, because getting it backwards makes the reset button do
nothing.

The notice is marked seen *before* it is shown, not after — otherwise a broken
notification backend would retry on every close forever.

**Where this actually works.** The plugin sets the notification's
`System.AppUserModel.ID` to the bundle identifier only for an installed build;
it detects an exe under `target/debug` or `target/release` and skips it, so a
Windows dev run may show nothing. Windows resolves that AUMID through the
Start-menu shortcut NSIS creates, which is why real presentation can only be
checked on an installed build. On macOS in dev the plugin attributes
notifications to `com.apple.Terminal`, so the wiring is exercisable here even
though the Windows presentation is not.

**Display-only, deliberately.** Nothing promises that clicking a toast does
anything — the tray icon is the way back in.

This was first written down as "a clickable toast needs a registered COM
notification activator CLSID", which is only half right, and the half it gets
wrong is the interesting one. Investigated properly against
`tauri-plugin-notification` 2.4.0:

- A **COM activator CLSID** (`System.AppUserModel.ToastActivatorCLSID` on the
  Start-menu shortcut) is needed to activate an app that is *not running* —
  the cold-start case, and a toast clicked out of the Action Center after the
  process has exited. It is **not** needed for a toast clicked while the app
  is alive: `ToastNotification.Activated` is an in-process event handler and
  works without one. This app lives in the tray, so the running case is the
  normal case, and that route was assumed closed when it isn't.
- **The plugin is the real blocker, and it closes both routes.** Its Actions
  API (`registerActionTypes` / `onAction`) is documented mobile-only and
  exists only in the crate's `mobile.rs`. Its desktop path builds a
  `notify_rust::Notification` and calls `.show()` inside
  `tauri::async_runtime::spawn`, discarding the returned `NotificationHandle`
  — which is precisely the object carrying the activation-event receiver.
  `notify-rust`'s Windows backend *does* wire `on_activated`; the plugin
  simply throws the result away, and exposes no hook to get at it.

So activation is unreachable through the plugin at any currently shipping
version, CLSID or not. Reaching it means bypassing the plugin on Windows and
driving `tauri-winrt-notification` directly, holding each handle alive and
pumping its receiver — a block of Windows-only code that neither this
development machine nor a dev build can verify (the plugin's own AUMID skip
under `target/debug` is a symptom of the same problem). That was judged not
worth it for the payoff, which is saving one tray-icon click. Recorded here
so the question is not re-opened from the same wrong premise.

Everything in `notify.rs` is best-effort: a notification that fails to show is
a logged warning, never an error that propagates. It is feedback *about* a
recording and must never be able to affect one. Like `tray.rs`, it carries no
tests — the decisions live in `core::NotificationPrefs`, which is tested.

### One notifier, one seam

`Supervisor` now has a single `set_event_notifier` over a `SupervisorEvent`
enum — `LibraryChanged`, `RecordingStarted`, `Finalized`, `RecordingFailed` —
rather than a callback per signal. The finalize toast needed to know *what* was
written, which a bare "something changed" callback cannot say, and adding a
second one-off notifier would have meant a third later. When the recorder moves
into its own process this seam becomes a socket write, and there should be
exactly one place to change it.

### Still to build

The socket itself, and with it: per-request ids, because a slow
`extract_audio_track` must not head-of-line block a status poll; and a socket
name scoped by build identity — deferred until there is a socket to name rather
than added speculatively — because
`tauri.devtools.conf.json` overrides `productName` but **not** `identifier` — so
a dev build and an installed release already share `app_data_dir()`, the
database and the recordings folder. For the same reason a version-mismatched
handshake must refuse to attach and say so, never tell the other daemon to quit:
it might be recording.

---

## 13. Logging

`main.rs` sets `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`, which is what stops a console window flashing over the game. It also means a **release build has no console at all** — so the ~50 `eprintln!`/`println!` calls this app used to make were writing to a closed handle on the one machine where capture problems actually happen. The dev portal's Log panel recorded only the portal's own IPC calls, and said so in its header. In practice the app could not tell you why anything went wrong.

`log.rs` is the fix: a file under `app_data_dir()/logs/`, **written in release builds**, plus one way to write to it.

- **The log ships; the viewers do not.** CLAUDE.md forbids attaching the devtools build to a release, so a devtools-only log could never observe a real failure. The panels that read and present it stay behind `--features devtools` (#72), which keeps the shipped surface to one file and no UI.
- **`error!` / `warn!` / `info!` / `debug!`, each taking the `[tag]` the codebase already wrote by hand.** The tags (`state_machine`, `lcu`, `retention`, `recorder`, …) were already consistent and already greppable; the facade keeps them and adds a timestamp and a level. `error!` is reserved for what a user actually feels — a lost recording, a library that will not open, a deletion that did not free space. Everything that degraded and carried on is `warn!`.
- **`debug!` is off by default**, and exists for the high-volume streams: the per-poll Live Client Data tracker (`live-poll`, shipped — see [docs/recording-pipeline.md §3](docs/recording-pipeline.md)) and libobs's own log (`libobs`, #69). Either at `info` would rotate a session's real errors out of the file within one game. `NINJA_RECORDER_LOG_LEVEL=debug` turns them on. The exception is a poll failure that ends a recording, which is a `warn` — see #74 for why that one has to be visible without asking.
- **Rotation is 5 MiB × 3.** About two play sessions of history, which is the window a capture bug is diagnosed in.
- **Nothing here may fail the app.** A read-only data dir, a locked file, a full disk: each degrades to "no file logging this session", never to an error a caller has to handle. `write` returns `()` and swallows I/O errors — recording a game matters more than recording *about* recording one. A failed write drops the sink for the rest of the session rather than retrying every line, because the usual causes do not fix themselves.

### Reading it back

Nothing in the shipped app reads the log — the viewers are behind `devtools`, so a release build carries the file and no UI (#72). The dev portal's Log panel reads it through `dev_read_log`, which filters **in Rust**: the file is capped at 5 MiB, which is far too much to hand a webview in one string.

The *parsing* lives in `log.rs` beside the formatter that defines the format, not in `dev/`. A reader that re-describes the format somewhere else drifts from it the first time either changes; a round-trip test through the real `format_line` is what stops that.

Levels include and tags exclude, which looks inconsistent and is not. Levels are four known values a panel can list up front, so ticking them is an inclusion. Tags are discovered *from the file*, so a panel cannot say "everything except the noisy ones" as an inclusion list until it has already read the file once — and the noisy ones (`live-poll`, `libobs`) are exactly what should be hidden on the very first render.

### The libobs worker's log

libobs does not run in this process. `libobs-recorder` spawns
`extprocess_recorder.exe` and calls `obs_startup` **there**, so
`base_set_log_handler` called from here would attach a handler to a libobs
instance we never initialize — it would compile, run, and capture nothing.
The symbol being present in `libobs-sys` is what makes that look like a
local change; it is not one.

What the worker does do is write to stderr, via libobs's default handler.
The fork's `ipc-link` spawns it with stdin and stdout piped — those carry
the JSON IPC protocol — and **stderr inherited**. So the messages already
arrive at our stderr, which in a release build has no console behind it.

So `recorder::libobs::worker_log` points this process's stderr at
`logs/libobs.log` before the worker is spawned, and the child inherits it.
No change to the fork, no IPC change, and — a file rather than a pipe — no
way to block the worker by failing to drain it, which the piped version
would risk. One previous session is kept as `libobs.1.log`: appending
forever grows unbounded, and truncating outright loses the session that
crashed, which is the one anybody is looking for.

Only in builds with no console (`debug_assertions` is exactly the condition
`main.rs` gates `windows_subsystem` on), because taking stderr away from a
`tauri:dev` terminal would be a downgrade. `NINJA_RECORDER_LIBOBS_LOG=1`
forces it on so the path is exercisable from a dev build.

The costs, stated: the lines land in their own file rather than interleaved
with ours, and they carry no level to filter on, because the formatting is
libobs's and not ours. The dev portal lists the file alongside ours and its
level filter deliberately lets level-less lines through, or selecting it
would show an empty view. Both costs are what a handler *inside* the worker
would fix, and that is a change to the fork — worth making once a real
capture shows it is needed (#69).

### Why not `tracing`

`tracing`, and `log` + `fern`, both do this and more. What was needed was a timestamp, a level, a tag and a file that rotates; `tracing`'s value is spans and structured fields, and nothing in this app has asked for either. This project has kept its dependency tree deliberately small (§1.2), and a date crate would have been a second dependency purely to format a timestamp — so `log.rs` hand-rolls Howard Hinnant's `civil_from_days`, which is the same closed form a date crate would run, and pins it with tests for the leap-year and century rules. Revisit when something genuinely wants spans.

### The three things that deliberately do not go through it

- `launch.rs`'s unsupported-mode message, which runs in `run()` **before** `setup` and so before `log::init` — there is no file yet, and it exits immediately.
- The two lines reporting that logging itself could not start. Saying so through the log would say nothing.
- The `SchemaTooNew` block, which is a wall of actionable prose aimed at a person in a terminal. That one keeps its `eprintln!` *and* gets an `error!` line, so the fact is recorded and the explanation is still readable.
