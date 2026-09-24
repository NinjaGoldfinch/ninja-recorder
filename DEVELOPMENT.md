# Development Guide

Design decisions, constraints, API references, and risks for ninja-recorder. This is the "why" document, so read it before touching the recorder or game-integration code.

For the "what and how" (component diagrams, the runtime sequence, the schema, the CI job graph) see **[docs/](docs/)**.

> **Section numbers here are load-bearing.** Roughly 35 source comments cite this file as `DEVELOPMENT.md §2.2`, `§3.4` and so on. Add sections, rewrite their contents, but do not renumber them without updating every citation (`grep -rn 'DEVELOPMENT.md §' src src-tauri`).

---

## 1. Hard constraints

### 1.1 Riot Vanguard (the constraint that shapes everything)

League of Legends runs under Riot Vanguard, a kernel-level anti-cheat that loads at boot. Consequences:

- **Never inject.** OBS-style "Game Capture" works by injecting a DLL into the game process to hook the graphics API. That is precisely the behavior Vanguard exists to detect. At best it silently fails; at worst it flags the user's account. This is not configurable, not an option we expose, not something we "try."
- **Capture path: Windows.Graphics.Capture (WGC).** WGC reads composited frames from DWM, with no hooks and no injection, and works with League in borderless/windowed mode. Display capture is the fallback.
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
unconditional background work, running from launch to exit whether or not
League is even installed, so it is the floor under idle CPU, and it earns its
2 s cadence only while the client is up. A sustained absence ramps it to 30 s
(`lockfile::poll_delay`), with a short grace window first so a *client restart*
is still noticed at full speed. It also caches the resolved Windows install
directory rather than re-reading and re-parsing `RiotClientInstalls.json` on
every single tick.

---

## 2. Capture design

### 2.1 Decision: embed libobs

We embed **libobs as a library**, rather than "control an installed OBS via obs-websocket" (requires the user to install/configure OBS; bad product) or writing it fully from scratch (see §2.3).

libobs gives us, solved: frame pacing, WASAPI loopback audio capture, audio/video sync, hardware encoder integration, MP4/MKV muxing, and the WGC capture source. These are months of subtle drift bugs we do not want to own.

Reference implementation: [league_record](https://github.com/FFFFFFFXXXXXXX/league_record) (Tauri + libobs + LCU + Live Client Data). Read it before writing capture code.

**Rust bindings: a fork, not the crate as-is.** league_record's libobs FFI/IPC layer is [`libobs-recorder`](https://github.com/FFFFFFFXXXXXXX/libobs-recorder). It is solid (out-of-process worker for crash isolation, bindgen bindings kept current with OBS releases, a real encoder-settings API) but its video source is hardcoded to OBS's `game_capture`, which DLL-injects the target process. That's exactly the behavior §1.1 forbids. We depend on [`NinjaGoldfinch/libobs-recorder`](https://github.com/NinjaGoldfinch/libobs-recorder), a patched fork: `game_capture` → `window_capture` forced to `method=2` (Windows.Graphics.Capture), plus `muxer_settings` for fragmented MP4 output (see §2.2's crash-safety rule; this replaces the MKV-remux approach, because a fragmented MP4 needs no finalization step and so stays playable even if the process dies mid-recording). Vendoring the crate directly wasn't viable, since its `build-helper` subcrate checks in every historical libobs Windows binary release at around 900 MB, so it's a git dependency like upstream, not copied into this repo.

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
- `LibObsRecorder`: Windows, the real one.
- `StubRecorder`: every non-Windows build. It sleeps, then copies a fixture MP4 into place. Keeps the entire app layer developable and testable without Windows. Nothing ships it; since the macOS bundle was dropped it exists purely for the dev loop and `cargo test` (§9).
- The own backend (Option B, `recorder/own/`): empty until WS1.6. Which of it and libobs the daemon builds is the `capture_backend` setting, and what happens when the chosen one cannot be built is §16's "The switch, and when it applies".

**Decision: the backend is warm only while the League client is.** Bringing
`LibObs` up spawns the out-of-process worker *and* sends it `Init`, which runs
`obs_startup` and loads every plugin, so a live backend is a D3D11 device and
the whole libobs plugin set resident in another process, not a dormant handle.
It used to be constructed in `lib.rs`'s `setup` and held until exit, which put
the single largest item on the idle-RAM budget (§1.2) on a machine that might
never open League.

`prepare`/`release` move that to the state machine's `ClientRunning` window:
warm when the client appears, cold when it goes away. Two alternatives were
rejected. Staying warm forever is the old behaviour and the thing being fixed.
Going lazy on the first `start` instead would put libobs init *inside* the
record path, where it lands on top of the existing bounded window-size wait and
risks losing the opening seconds of a game, whereas the client being open is a
reliable minutes-ahead signal that a game is plausible.

`prepare` is therefore a pre-warm and nothing depends on it: `start` calls the
same idempotent `ensure_up`, so the two racing (a client that goes straight into
a game) is harmless. `release` refuses to run while a recording is in flight.
The supervisor drives both from the resulting *state*, not from `Action`s. A
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

`LibObsRecorder` picks the game window (`FindWindowA` on title `"League of Legends (TM) Client"` / class `RiotWindowClass` / process `League of Legends.exe`, the same identifiers league_record uses, verified against its actual source) and captures at its real client-area size (`GetClientRect`, retried briefly since the size can report (1,1) for a moment right after the window appears) rather than a hardcoded resolution, which is what §2.4's "resolution follows the game window" means in practice. Encoder choice walks `available_encoders()` (already returned in NVENC→AMD→QSV priority order by the crate) and picks the first **H.264** one, explicitly excluding both `OBS_X264` (§2.4's no-silent-software-fallback rule, where `start()` errors instead) and the AV1 variants the crate would otherwise prefer for NVENC (§2.4's WebView2-native-H.264-decode requirement, §5). Audio is whatever the user's preset asks for, split across separate mp4 tracks (§2.5); rate control is `CBR(8000)` at 60fps per §2.4's defaults.

**Runtime files: staged outside Cargo, not via artifact-dependencies.** league_record gets `extprocess_recorder.exe` + its libobs DLLs into the build via Cargo's artifact-dependency feature (`artifact = "bin:..."`), which needs nightly Rust + the unstable `bindeps` flag, since their whole project builds on nightly (CI: `dtolnay/rust-toolchain@nightly`). We can't do that: `-Z bindeps` syntax in `Cargo.toml` breaks manifest parsing *for every platform*, confirmed locally (`cargo check` on macOS failed until the artifact-dependency lines were removed). It would force the dev box's `cargo check`/`npm run tauri dev` onto nightly + an unstable flag just to support an optional Windows-only binary, which is a real regression against §9's dev loop. Instead, CI's "Stage libobs capture backend" step (`.github/workflows/ci.yml`'s `build` job, Windows leg only) builds the fork's `extprocess_recorder` binary as a fully separate `cargo build` invocation and copies it + the matching `libobs_<version>/` DLL folder into `src-tauri/target/libobs/` directly, with no Cargo dependency-graph involvement and ordinary stable Rust throughout. `tauri.windows.conf.json` then bundles that folder as a resource, and `LibObsRecorder::new` (lib.rs) resolves it at runtime via Tauri's path resolver. Anyone working on the capture backend locally on the Windows box needs to run the same clone-build-copy sequence by hand before `cargo run`/`npm run tauri dev` until that's scripted for local use too.

**Faststart remux on stop, staged the same way.** The fork's `muxer_settings` (above) trade seekability for crash-safety: `frag_keyframe+empty_moov+default_base_moof` means no player, including the review UI's own WebView2 `<video>`, can reliably scrub the file, since there's no upfront seek index. `LibObsRecorder::stop` fixes this up after every *clean* stop with a stream-copy remux (`ffmpeg -c copy -movflags +faststart`, lossless, just rewrites the container index) before handing the path back. `ffmpeg.exe` is staged into the same `target/libobs/` resource folder by a sibling CI step ("Stage ffmpeg for faststart remux") that downloads a static build from BtbN's FFmpeg-Builds releases. It is optional at runtime (`lib.rs` resolves it with `.ok()`), so a failed download degrades to unseekable-but-still-playable recordings rather than breaking the build. **Not verified**, with the same caveat as the rest of this backend below: nothing has confirmed the remux actually runs against a real capture on a real Windows box yet, only that it type-checks.

**Every ffmpeg spawn gets `CREATE_NO_WINDOW`.** ffmpeg ships as a console-subsystem binary, so a GUI process spawning one makes Windows allocate it a fresh console: an empty black terminal window sitting over the game for the length of every faststart remux, and again each time the review player extracts a stem (§2.5). Both call sites capture stdout and stderr, so that window never had anything to display; it is pure noise, and on the remux path it lands at exactly the moment the player is reading the post-game screen. `lib.rs`'s `ffmpeg_command` is now the only way the bundled ffmpeg is launched and it sets the flag there, so a third call site cannot reintroduce the window by forgetting. The libobs worker needs no equivalent: the fork builds `extprocess_recorder.exe` with `windows_subsystem = "windows"` for release, so it is only ever visible in Task Manager, which is where [windows-verification.md](docs/windows-verification.md) checks for it.

**Not verified: no Windows machine touched this code.** Same caveat this doc already applies to the async supervisor glue (§3.4): written and cross-checked against league_record's real, working source (not guessed), but nothing here has run. Specific open questions for the first Windows pass (§9):
- Does `window_capture` forced to WGC actually produce frames for League's borderless/windowed modes, and does Vanguard tolerate it (the whole point of this fork, and it needs a real check rather than "should work").
- The CI staging step's assumption that `Sort-Object Name -Descending` on `libobs_<version>/` directory names picks the newest. That is a string sort, not version-aware, but the fork's directory names so far (`libobs_28.1.1` … `libobs_32.0.4`) happen to sort correctly that way.
- The `tauri.windows.conf.json` resource path (`target/libobs` → bundled next to the installed .exe) matches league_record's own working config, but its interaction with `cargo tauri dev`, where the running binary is `target/debug/ninja-recorder.exe`, one level deeper than `target/libobs`, is unclear from reading the source alone; may need the staging step to also copy into `target/debug/libobs` for dev mode to work.
- Encoder priority and window-size retry timing are first-cut defaults, not tuned against real hardware.
- Does `wasapi_process_output_capture` produce non-silent samples for a Vanguard-protected `League of Legends.exe`? Per-application loopback is the source behind every preset that names "game audio" (§2.5), and it is the one part of the audio design with no fallback if the answer is no; desktop capture is the documented workaround.

### 2.3 Alternatives considered (and why not)

| Option | Why rejected |
|---|---|
| obs-websocket → installed OBS | User must install + configure OBS; fragile coupling to their scenes/settings |
| From scratch: WGC → D3D11 → Media Foundation SinkWriter | Legitimately clean (~1–1.5k lines, ~25 MB installed) but we'd own A/V sync, pacing, and WASAPI loopback bugs. Only revisit if libobs's footprint becomes disqualifying; the trait makes the swap possible |
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
| Game | Game | none | none | none |
| Game + mic | Everything | Game | Mic | none |
| Game + mic + Discord | Everything | Game | Mic | Discord |
| Desktop | System audio | Game | none | none |

**Track 0 is always the combined mix.** This is the decision the rest of the
design follows from. It means a player that knows nothing about any of this,
including our own review player's `<video>` element and whatever the user
drags the file into, plays the right thing by default. Everything after
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
duplicate. Desktop is the only two-track preset, because system audio already
contains the game, so track 1 isolates the game back out of it.

**Discord is captured as a named application, not a Discord-shaped special
case.** `AudioSourceKind::Application { exe }` takes any executable, matched by
`WINDOW_PRIORITY_EXE` rather than window title, since Discord retitles itself
to whatever channel is open, so title matching would break constantly. The same
mechanism is what a future "custom" preset needs for Spotify or anything else.

**The capture fork had to change; a separately-captured mic was the
alternative.** Upstream `libobs-recorder` creates one AAC encoder on mixer 0
and mixes every source into it. The alternative to patching it was capturing
the microphone ourselves and muxing it in afterwards with ffmpeg, which means
owning A/V sync for the mic, exactly the class of bug §2.1 chose libobs to
avoid. The fork now creates one encoder per track; `obs_audio_encoder_create`
fixes an encoder's mixer index at creation with no setter, so encoder *i* is
permanently track *i*, and what varies per recording is a per-source mixer
bitmask. A libobs source can feed several mixes at once, which is what makes
the combined-mix-plus-stems layout nearly free: game audio on both track 0
and track 1 is one extra bit, not a second capture.

Two traps in that area, both of which fail silently:
- libobs defaults a source's `audio_mixers` to `0xFF` (every mix). Left alone,
  every track would contain an identical full mix.
- `num_audio_mixes` walks the output's encoder array and stops at the first
  null, so binding tracks 0 and 2 while leaving 1 unbound truncates the file
  to **one** track.

**The faststart remux had to be fixed in the same change.** `remux_faststart`
ran `-c copy` with no `-map`, so ffmpeg's default stream selection kept a
single "best" audio stream, which would have deleted every stem on the way
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
player already runs. Track 0, the common case, needs none of it.

This does mean owning a small amount of A/V sync after all, which §2.1 says
we didn't want. The mitigating difference is that it is *playback* sync over
a file that already exists, recoverable by reloading, rather than capture sync
that would corrupt a recording. It is confined to the review player and
touches nothing on the recording path.

**Preferences.** The preset is one `settings_kv` row (`audio_preset`) holding
JSON, which is a zero-migration change per §4's reasoning. Unlike `theme` it is
read and validated backend-side: a bad theme value looks wrong, a bad audio
preset changes what gets recorded, and an unreadable one falls back to
game-audio-only rather than to whatever parses. The per-recording layout is a
separate, nullable column (`recordings.audio_tracks_json`), because NULL is the
honest answer for the VODs that predate this and for anything a rescan
imported.

#### Decision: the own backend writes its own MP4

Everything above is how the **libobs** backend gets its tracks. The own
backend (§16, Option B) cannot take the obvious route. Plan §4.5 has it
encode with Media Foundation and write through the sink writer, "per-track
AAC", and that cannot be built: both of Media Foundation's MP4 sinks,
`MFCreateMPEG4MediaSink` and `MFCreateFMPEG4MediaSink`, take one video stream
and **one audio stream**, and do not support `AddStreamSink`
([MS Learn, "MPEG-4 File Sink"](https://learn.microsoft.com/en-us/windows/win32/medfound/mpeg-4-file-sink)).
Every preset in the table above except Game has two to four tracks.

So the own backend encodes with Media Foundation and **muxes with our own
code**: `src-tauri/src/mp4/write.rs`, a fragmented-MP4 writer for one H.264
track and any number of AAC tracks (#235). It is pure Rust with no Windows
calls, so it is built and tested on Linux, where the tests write files with
one, two and four audio tracks and check each with ffprobe and a full ffmpeg
decode. What it costs on Windows is doing without the sink writer's plumbing:
the encoders are driven directly (#239).

- **The file is fragmented, as libobs's is**: a `moof` + `mdat` per keyframe
  interval, so a killed process loses at most one GOP, and an `mfra` at the
  end. `repair` truncates a killed file to its last complete fragment and
  appends the `mfra`, which is recovery with no ffmpeg in it.
- **Stem sidecar files were considered and rejected.** Writing track 0
  through the sink and each stem to its own file, merged by ffmpeg at stop,
  would have kept the sink. It makes ffmpeg mandatory rather than optional,
  leaves a recording as a group of files until the merge has run, and gives
  recovery and retention a group to reason about instead of a file. A killed
  merge is a new way to lose a recording.
- **An existing crate was looked for first** (September 2026). None met the
  bar of fragmented output with any number of audio tracks under a licence
  `deny.toml` allows: `muxide` and `mp4e` (MIT/Apache, MIT) have APIs for
  one audio track; `mp4` (MIT, last released 2023) writes non-fragmented
  files; `mse_fmp4` (MIT) was last released in 2020 and targets a single
  MSE stream; `mp4-atom` (MIT/Apache) and `shiguredo_mp4`
  (Apache-2.0) encode boxes or segments but leave the muxer, the `mfra` and
  repair to the caller, which is most of the work.

---

## 3. League integration

Two official local HTTP APIs. Both use self-signed TLS on localhost, so pin or accept the Riot self-signed cert for these connections only; never disable TLS verification globally.

### 3.1 LCU API (the client)

- **Discovery:** parse the `lockfile` next to the running client. macOS: `/Applications/League of Legends.app/Contents/LoL/lockfile`. Windows: install dir is user-configurable, so resolve it via `%PROGRAMDATA%\Riot Games\RiotClientInstalls.json`'s `associated_client` map first, falling back to the conventional `C:\Riot Games\League of Legends\lockfile`. Format: `name:pid:port:password:protocol`. Watch for the file appearing and disappearing, because the client restarts and ports change. Implemented in `src-tauri/src/lcu/lockfile.rs`, with an `NINJA_RECORDER_LOCKFILE_PATH` env override for tests/non-standard installs.
- **Auth:** HTTP Basic, user `riot`, password from the lockfile.
- **Key endpoints:**
  - `GET /lol-gameflow/v1/gameflow-phase`: `None / Lobby / ChampSelect / InProgress / EndOfGame / ...`. Our record trigger. Also subscribable via the LCU WebSocket (`/lol-gameflow_v1_gameflow-phase` event); prefer the WebSocket over polling.
  - `GET /lol-gameflow/v1/session` (during the game): `gameData.gameId`, `gameData.queue.id` and `gameData.isCustomGame`. Read once when gameflow reaches `InProgress`. Working out *which* `gameId` just ended is the problem that kept `match_data` unwired; the client will say so while the game is still running, so it is read then rather than deduced afterwards.
  - `GET /lol-end-of-game/v1/eog-stats-block` (post-game): *our own* stats block, `teams[].isPlayerTeam` + `isWinningTeam`, our `championId`, our scoreboard. Tried **first**, because it needs no participant join at all and is populated during `EndOfGame`.
  - `GET /lol-match-history/v1/games/{gameId}` (post-game): champion, KDA, win/loss, queue id, `timeline.lane`/`.role` and `gameVersion`. The only source for `role` and `patch`, but it lags the end of the game and has to be joined back to us.
  - `GET /lol-game-data/assets/v1/champion-summary.json`: the client's own asset store, `{id, name, alias, contentId, description, squarePortraitPath, roles}` per champion, plus an `id: -1` "None" sentinel. Turns the `championId` the two post-game endpoints answer with into a name. **Confirmed off a real client (2026-09-07)**, unlike the two endpoints above.
  - `GET /lol-replays/v1/rofls/{gameId}/download`: native replay download (§8).
- **Identifying ourselves in a match-history response is the fragile part**, and it is not a matter of picking the "right" field. The response splits players across `participants[]` (stats, joined by `participantId`) and `participantIdentities[]` (accounts), so the identity has to be matched back to `/lol-summoner/v1/current-summoner` by an account key, and which keys the endpoint sends has moved over time. The LCU's own OpenAPI spec carries no `puuid` on a match-history participant identity at all, only `accountId`/`summonerId`/`summonerName`, while 74 other schemas in the same spec do have one. `match_data.rs` therefore treats every key as optional and tries `puuid`, then `summonerId`, then `accountId`, requiring the key to be present on *both* sides. A client that sends `puuid` and one that does not both work without the code needing to know which it is talking to.
- **The post-game fetch cannot happen during the finalize.** At the instant `Recording → Finalizing` fires the client is still in `WaitingForStats`, so match history 404s, and the same transition tears down the gameflow watch that owned the LCU connection. So the row is written from what Live Client Data established during the game, and `match_summary::patch` fills in `role`, `patch` and a confirmed `queue`/`win`/`kda_*` afterwards, on a bounded retry schedule (2s, 4s, 8s, then three at 15s), then re-emits `library-changed` so the card fills itself in while the user watches. It also fills `champion` for the one case the live path cannot cover, a game whose Live Client Data poller never came up, resolving the id best-effort, so a name it cannot find never costs the row its outcome and queue id. Past the ceiling it gives up **silently**: the row already carries champion, KDA and outcome, and a missing queue id is not worth interrupting the next game over.
- **Preferring the end-of-game block over match history is a correctness decision, not a latency one.** The block is scoped to us already, so the outcome falls out of two booleans with no participant matching, and participant matching is precisely what had never worked (`extract_summary` joined on a `puuid` the endpoint does not send, so every fetch would have failed to *deserialize*). Where both sources answer, the block wins, because it cannot have matched the wrong player. Where they contradict what Live Client Data recorded, that is logged loudly rather than silently written: the two are views of one game, so a disagreement almost certainly means the wrong `gameId` was matched.
- **The live client's position beats the LCU's inference, and the scoreboard had to learn that too.** `role` has always taken the live value and used Riot's `lane`/`role` pair only to fill a gap, because the inference works out where time was spent afterwards and confuses top with jungle. The *scoreboard's* per-player `position` did not follow the same rule, and the LCU rebuild (#127) replaces the board wholesale, so a patched recording ended up carrying the inference for all ten players. Two failures came out of that, and the second is the one worth naming: a missing position empties the matchup, which announces itself, while a **present but wrong** position picks the enemy in the wrong lane and shows a plausible opponent who is not the one you played. Nothing on the row looks broken in that case, which is exactly why the rule has to be the same in both places.

- **A rank is captured, never recomputed, and the gate is time rather than caller.** The client only ever reports the rank you hold *now*, so a rank is only true of a game if it is read while the game is still recent. `match_summary::rank_still_describes` is that rule: the live patch runs seconds after a finalize and always passes it, the resume sweep looks back two days and never does. Writing it as a freshness window rather than "only the live path may write these columns" means a third caller cannot get it wrong by existing, and the database write fills only when the columns are NULL so the reading taken closest to the game beats any later one. **The backfill must never fill them.** It matches recordings to games on the clock, so it would stamp this season's rank onto a game played in another, and it would look entirely plausible. That is the same rule #56 already applies to every other column, applied where it bites hardest.

- **A measured LP delta is a different proposition from a subtracted one, and only the second was rejected.** Nothing Riot sends reports a change, so #149 shipped `lp_after` alone. What makes a delta defensible is not arithmetic but *position*: the app reads the ladder when a game starts, where it is already resolving the gameflow session, and again when the patch lands, so the interval contains exactly one game. The confounders that sink a naive before-and-after are not mitigated, they are **absent**, because there is no room between the two readings for another game, a dodge, or decay. `lcu::ranked::lp_delta` is that measurement (#164), and it refuses rather than guesses: different ladders are not comparable, and a tier below Master whose division could not be read is four hundred LP of uncertainty. It is still *ours* and not Riot's, so an endpoint that reports a change directly would beat it outright, which is the same order of preference §5.2 sets for the gold curve.

- **The ladder has two models, for two different questions.** Ordering standings needs no distances, which is exactly what lets the tier list stop at Master and lets one rung hold Master, Grandmaster and Challenger together, which is all the lobby median needs. Measuring a delta needs to know *how far*, so it converts through four hundred LP a tier. **The scale runs continuously into the apex**, which took a correction: promotion out of Diamond I is instant at 100 LP with no placement cutoff to cross, so Diamond I 100 and Master 0 are the same position, and Grandmaster and Challenger change the label rather than the number, being leaderboard cutoffs over one shared LP pool. Master is therefore just the top rung with no division to add, and the apex needs no special case at all. Keeping the two functions apart is still deliberate: ordering needs no distances, so a single "rank as a number" would force the stricter question on a median that never asked it.

- **LP is stored as a value, not a change, because nothing reports a change.** The end-of-game stats block was captured from a real ranked game and carries no LP field at all; both ranked endpoints answer with current state. The tempting fallback, read before and read after and subtract, is wrong across a dodge, a remake, decay, a promotion series and any game played while the app was closed, and it cannot survive a restart mid-game without persisting a "before" whose staleness nothing can check. §5.2 settled this argument once already for the gold curve, and migration 8 renamed a column off `gold_diff_est` precisely because a computed value under an authoritative name is worse than no value. `lp_after` is the honest half; a delta column can be appended the day something reports one.

- **Champion id to name comes from the client, not from Data Dragon.** What the resolver produces has to be byte-identical to what Live Client Data writes, because `champion` is sorted on, filtered on and used as the card title: `MonkeyKing` and `Wukong` in one library is one champion in two places, and the split is invisible until somebody notices half their games are missing. The asset store is served by the client we are already authenticated against, on the patch that client is running, so it cannot go stale, it needs no network, and it is up by definition whenever the patch runs, so a remote CDN and a version to pin would be a dependency bought for nothing. `alias` is the field that must not be read: it is exactly where those legacy spellings live. The map is fetched once per client session and cached against the lockfile, so a restart onto a new patch re-fetches rather than serving a table missing the champion released that morning. **This is a decision about names.** Champion *art* is a different question, since a CDN is a fine place to keep images, and answering it does not disturb this.
- **One display name, several ids.** The real store carries a parallel `Jade_*` block in the 60000s: `Jade_Wukong` is id 60062 and its `name` is `Wukong` too, exactly like id 62. Reading it as id to name is unaffected, because both ids genuinely *are* Wukong and either is the right answer to "what was I playing?". What it rules out is the reverse map, and champion art is keyed the other way round: Data Dragon's image filenames are the champion's key, which is the LCU's `alias`, not its `name`. So art cannot be looked up from the `champion` column alone; #85 carries that.
- **`role` comes from the game, not from the LCU's inference.** Live Client Data's `allPlayers[].position` is what the client itself assigned; the LCU answers with `timeline.lane` + `timeline.role`, which is Riot working the position out afterwards from where a player spent time. That inference is weakest exactly between top and jungle: a Viego played top comes back as jungle, because that is where Viego usually is. So the live value is written at finalize and `update_match_metadata` COALESCEs `role` **the other way round**, like `champion`: the LCU fills a gap for a game the poller missed, it never corrects one. Both sources map onto the same five words (`Top`, `Jungle`, `Middle`, `Bottom`, `Support`), because one column carrying both `Support` and `UTILITY` is one nothing can group by, which is the same rule champion names live under. An unrecognised position, including the empty string a mode with no lanes reports, yields NULL and the row says `Unknown`.
- **Never match on `summonerName`, and never join on a zero id.** Display names are not unique and they change. `summonerId: 0` is what the LCU puts in the slot for a participant whose identity is hidden, so joining zero to zero would attach the first anonymous player in the list to the recording. Both would mislabel a VOD with a stranger's game, which is worse than leaving the metadata NULL.

### 3.2 Live Client Data API (in-game)

- `https://127.0.0.1:2999/liveclientdata/allgamedata`: no auth, only up while a game is running. A 3-second request timeout: reqwest applies none by default, and a stalled request on a loopback endpoint that normally answers in ten milliseconds is a hang, not a slow reply. It silently stopped markers while the recording carried on, because no error was ever returned to declare the endpoint down.
- **Failing to read a response is not the same as the game being gone**, and conflating the two cost a real game (#74). A payload we cannot parse proves the game is *running*; only a request that got no response at all means the process behind port 2999 has ended. The poller tolerates five consecutive transport failures before finalizing, and never finalizes on a parse failure. Events are parsed entry by entry so one unreadable event costs that event rather than the snapshot; the events array is the only part of the payload that both grows during a game and can fail to deserialize.
- Poll ~1 Hz. Relevant pieces:
  - `events.Events[]`: `ChampionKill`, `Multikill`, `TurretKilled`, `InhibKilled`, `DragonKill`, `BaronKill`, `HeraldKill`, `HordeKill`, `Ace`, `FirstBlood`, each with `EventTime` (seconds of game time). `HordeKill` is the Voidgrubs, and is stored under the name players use for them rather than the API's `Horde`; the API emits **one event per grub**, so a cleared camp is three markers a few seconds apart. They are left that way, because each is a separate kill with its own `EventID`, which is what the tracker dedupes on, and collapsing them would mean inventing a window over which three kills become one camp. The neutral objectives also carry `Stolen`, which rides in the marker payload and is read leniently, because Riot has historically sent booleans in this API as the strings `"True"`/`"False"` and a bare `Option<bool>` rejected the whole snapshot over it. **Whether that is what actually broke #74 is not worth establishing**: the lenient read makes either answer survivable, and the LCU's post-game data could supply steals instead if the live value ever proves unreliable. Which source a steal flag comes from does not change anything the review player does with it.
  - **Not every event becomes a marker.** See "only events the player is named in" below.
  - `activePlayer.summonerName` / `allPlayers`: identify which events involve *us* (our kills/deaths vs. someone else's). **`summonerName` is the champion name**, and the event name fields use it: a Ranked Solo capture (2026-09-07) had `summonerName` "Shyvana" against a `riotIdGameName` of "NinjaGoldfinch", and every event named "Shyvana". The same capture shows the API anonymises the other nine players outright (`riotId` "#", empty `riotIdGameName`) so the champion name is the only identifier the payload carries for them.
  - `gameData.gameTime`: for aligning game time to recording time.
  - `allPlayers[].championName` / `.scores` and `gameData.gameMode`: the library card's champion, KDA and mode. Taken here rather than from the LCU because the live API states the champion as a *name*, so nothing has to resolve a champion id, and because it works in Practice Tool and customs where match history does not.
  - The `GameEnd` event's `Result` (`Win`/`Lose`): the only place this API states an outcome, and the whole of win/loss detection until `fetch_match_summary` is wired in.
- **Only events the player is named in become markers.** A marker is a seek target and a stop on the review player's `[`/`]` navigation, so the bar is not "did this happen" but "was this about me". `classify_event` keeps an event only when the recording player is its killer, victim, assister, acer or recipient; everything else is dropped at classification and never reaches the database.

  Being on the team that took an objective is explicitly *not* taking part in it. The filter reads the event's own name fields rather than team membership, because the complaint that prompted it was a friendly turret: reviewing your team taking a T1 while you were on the opposite side of the map is exactly the stop nobody wants.

  **Two alternatives were rejected.** Keeping the enemy's objectives (the "why did we lose that Baron" case) would have needed a team lookup through `allPlayers[].team` and still leaves the arbitrary question of which uninvolved events are interesting. Storing everything with a relevance flag and filtering in the review UI is strictly more recoverable, but costs a schema column and a UI control for a case nobody had asked for.

  **The cost is that it is irreversible per recording.** Live Client Data is gone the moment the game ends, so a marker not captured can never be recovered for that VOD. If the filter is later judged too aggressive, older recordings stay filtered; only new ones benefit. That is the accepted trade: a timeline nobody trusts because it is full of other people's turrets is worse than one that occasionally omits something.

  `FirstBrick` (the first turret of the game) is deliberately *not* handled even though it is in Riot's event list: the API emits an ordinary `TurretKilled` for the same structure under its own `EventID`, so the tracker, which dedupes on `EventID`, would let both through and the VOD would carry two markers a frame apart.
- **Why the summary accumulates instead of being read off the last poll:** `GameEnd` shows up on one poll and the game process routinely exits before the next, so the poll carrying the result is often the last that succeeds. `LiveSummary::absorb` lets newer values win but never gives a known one back for a `None`. The champion has one more exception (#203): Live Client Data reports a possessing Viego under the possessed champion's name, so the last poll of a game that ended mid-possession names the wrong champion. Since no other champion changes mid-game, `settle_champion` treats a Viego once seen as settled and ignores any other name after it. Decided on the live side rather than left to the deferred patch's id-based `correct_champion` because that only runs where the LCU establishes a champion id, which a custom game did not reliably do.
- **Timestamp alignment:** marker position in the VOD = event `EventTime` mapped through an offset between game time and video time. The offset is measured on every poll where `gameTime` is seen to **advance**, never on the first poll. Recording starts *on* the first poll, so its `elapsed` is ~0, and that poll lands on the loading screen where `gameTime` is a frozen `0`; measuring there yields offset 0 and places every marker one loading screen early. Waiting for the clock to move proves it is a clock, and makes `elapsed` naturally include the load. Re-measuring on each advancing poll (rather than latching once) also absorbs pauses, which freeze the clock while the video keeps rolling, and encoder frame drops, which skew a fixed offset over a long game. Markers and samples are therefore stored with `game_time_s` and mapped to video time **at finalize**, stamped with the alignment in force when they were observed; anything seen before the clock first moved falls back to the first alignment the recording proved, or to 1:1 if it never moved. Recording still starts before `gameTime` 0 in the normal case, so the offset is normally positive; a reconnect makes it negative. Implemented as `live_client::events::AlignmentTracker`. Rationale and diagram: [docs/recording-pipeline.md](docs/recording-pipeline.md#timestamp-alignment).

### 3.3 Fixtures

Every API response shape we depend on gets captured to `fixtures/` (JSON) the first time we see it, and the poller/state machine must be runnable in replay mode against fixtures. This is what makes the League integration, library and review layers developable and unit-testable with no League running at all. Practice Tool (30-second launch, on-demand kills/objectives) is the live-testing tool of choice; never iterate against real queued games.

**Capture is on by default until v1.0.** It was opt-in via `NINJA_RECORDER_RECORD_FIXTURES`, and that variable is now an *override* rather than a switch-on: unset means on, and only an explicitly falsey value (`0`, `false`, `off`, `no`) turns it off. The dev portal's Fixtures panel still flips it at runtime.

The reason is #74. Almost every shape this app parses was written by hand and has never been checked against a real client, and a payload the parser could not read ended a recording nine minutes into a game, with no copy of it kept, so the triage was archaeology on a samples table. `record` runs *before* the parse, so with capture on, the payload that broke something is on disk when you go looking.

**`fixtures/live-client/captured-allgamedata.json` is a real one.** Every other file in that directory was written by hand from Riot's documentation, and the difference matters: `HordeKill`, `Primal Smite` and `Unleashed Teleport` were all shipped-and-broken because nothing here had seen a real payload, and `Stolen` turns out to arrive as the string `"False"`. Do not reshape that file to match an invented one. It is pinned by tests that check its marker counts against the KDA the payload states independently, so a "tidy" that changes its meaning fails. It is a mid-game capture, so it carries no `GameEnd`.

What that costs: one file write per response, so roughly 1/s during a game. Each endpoint overwrites a single file rather than accumulating, so there is no growth, and because the Live Client Data events array is cumulative, the last capture of a game contains every event in it. Captured payloads carry the Riot IDs of all ten players, which is worth remembering before committing one as a fixture.

**Revert to opt-in for the v1.0 release**: `fixtures.rs`, `DEFAULT_ON_UNTIL_V1`.

### 3.4 Game state machine

```
Idle ──(lockfile appears)──▶ ClientRunning
ClientRunning ──(phase: InProgress | Reconnect)──▶ WaitingForGame
WaitingForGame ──(port 2999 responds)──▶ Recording   [Recorder::start]
Recording ──(phase: EndOfGame | 2999 gone)──▶ Finalizing [Recorder::stop]
Finalizing ──▶ ClientRunning
```

Rendered as a state diagram, with the actions each transition emits and the full edge-case table: [docs/recording-pipeline.md §2](docs/recording-pipeline.md#2-the-state-machine).

Implemented as a pure transition function (`state_machine::machine::StateMachine::handle`, 11 unit tests covering the edge cases below) driven by a thin async supervisor (`state_machine::supervisor::Supervisor`) that spawns/aborts the lockfile/gameflow/Live-Client-Data watchers per `Action` and calls `Recorder::start`/`stop`. The pure part is fully tested, as are the two pieces of the supervisor that hold real logic: `start_recording`/`stop_recording` against a stub recorder and an in-memory DB, and `RecordingSession::ingest`, which takes elapsed time as an argument so a full poll sequence (loading screen, pause, reconnect) can be replayed without a clock. The watcher-spawning glue around them is not tested, because no League client is installed on the machine this was built on, so nothing here has touched a real LCU or Live Client Data connection yet. Closing that gap is [docs/windows-verification.md](docs/windows-verification.md).

Finalizing stops the recorder, time-aligns the collected markers, writes the `recordings` row plus its `markers` and `samples` (§4), enforces retention (§6) and emits `library-changed`. It writes `duration_s` from the session clock, `champion`/`kda_*`/`win`/`game_mode` from the Live Client Data summary the session accumulated while the game ran, and `game_id`/`queue` from the gameflow session read at `InProgress` (§3.1). Those summary columns are already on the row by then, written as the polls established them so a killed daemon keeps them (§4.3); the finalize rewrites them rather than checking, because it is rewriting the row either way. It does **not** fetch the LCU's post-game summary inline (see §3.1 for why that cannot work) but it does hand the game's identifiers to `match_summary::patch`, which fills in `role` and `patch` once the client has them. That hand-off goes through a type-erased notifier installed from `lib.rs`, for the same reason `on_event` does: `stop_recording` is directly unit-tested, and a bare `tauri::async_runtime::spawn` in it would drag the Tauri runtime into a code path `cargo test` executes. The tests leave it unset, so nothing spawns. The last finalized recording is also held in memory and exposed via `game_state_status`, so a failed DB write doesn't lose it.

Edge cases handled by the pure transition function (see its tests): game crash mid-match (Live Client Data stops responding), client crash (lockfile disappears) at every stage, reconnect to an in-progress game (state machine has no memory of *how* it entered `WaitingForGame`, so a reconnect behaves identically to a fresh game start, with recording beginning once Live Client Data becomes reachable, later than a from-the-start recording would), practice tool (goes through the same `Reconnect`/`InProgress` phases as a real game), dodges/cancelled champ select (bounces `WaitingForGame` back to `ClientRunning` without ever recording), and a client restart mid-finalize (picked up correctly regardless of ordering against `FinalizeComplete`).

Two edge cases from the original list are *not* verified. **Spectator mode:** the state machine simply doesn't special-case any phase name beyond `InProgress`/`Reconnect`/end-of-game ones, so if gameflow reports a distinct phase while spectating, it won't trigger recording; but if it turns out spectating also reports `InProgress`, this would incorrectly record it, and that can only be confirmed live. **Machine sleep:** not simulated in this environment at all; the poller's backoff and lockfile-watch would likely eventually recover state after wake, but this needs real testing on the Windows machine ([docs/windows-verification.md](docs/windows-verification.md)).

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

- A DB row without its file (user deleted the MP4) is cleaned up on scan; a file without a row is imported as "unknown recording." The library must survive users touching the folder. A recording still in flight is neither of those and is skipped by both halves: its row exists from the start, and is hidden until it is finished (§4.3).

Implemented in `src-tauri/src/db/` (`Db` + `reconcile`), migrations via `rusqlite_migration`, `rusqlite`'s `bundled` feature so no system SQLite is required on a fresh machine. Reconciliation runs once at app startup and on demand (`rescan_recordings` command). A `recordings` row is opened when capture starts and completed by the state machine's Finalizing step (§3.4) on every stop; its `markers`, its advantage-curve `samples` and the match-summary columns are written as the polls produce them and rewritten at that finalize (§4.3). `duration_s` comes from the session clock, read *before* the recorder is stopped so the ffmpeg remux isn't counted as footage. `champion`/`kda_*`/`win`/`game_mode` come from Live Client Data and `game_id`/`queue` from the gameflow session, both captured during the game rather than fetched after it. `role` and `patch` arrive last, from `match_summary::patch` seconds to a minute after the finalize (§3.1). That patch is a plain `UPDATE` and never a re-`insert_recording`: the upsert takes `pinned`, `size_bytes`, `started_at` and `duration_s` from `excluded`, so re-upserting a summary would unpin the recording and zero its size. Every column it writes COALESCEs so a value the LCU could not establish never erases one the live client did, except `champion`, which COALESCEs the other way and may only be filled when NULL. Two writers reach that column, the live client during the game and the id `lcu::champions` resolves afterwards, and both aim at the same display name (`Wukong`, never the internal `MonkeyKing` alias). Filling only when NULL means they cannot disagree *in the column* even if they ever disagree with each other, and one champion under two spellings would split its games in two wherever the library sorts and filters. **That reasoning covers a different spelling, not a different champion, and the two are not the same problem.** Live Client Data reports a possessed Viego as whoever he possessed, so a game that ends mid-possession writes a real champion who is the wrong one, and no rename table can catch it, because the name it wrote is a genuine champion. An id cannot be possessed, so the deferred patch corrects the column from the one the client answers with (`Db::correct_champion`). That correction is a method of its own rather than a flipped `COALESCE`, because the backfill shares the patch and matches games *on the clock*: letting it overwrite a champion would let a mismatched game rename a row that was already right, which is the failure #56 exists to refuse. The exact-id path may correct; the heuristic path may only fill.

### 4.1 Decision: imported files get their duration from ffmpeg, not ffprobe

`duration_s` comes from the session clock for recordings this app made. Rows
`reconcile` imported have no session, since it knows only the path, the size
and the mtime, so their `LENGTH` stayed unknown forever and they kept counting
toward the "N unknown" sub-label on the Recorded tile.

The obvious tool is `ffprobe -show_format`, which answers this in clean JSON.
**We don't ship it.** CI stages exactly one binary into the bundle,
`ffmpeg.exe` (`.github/workflows/ci.yml`, "Stage ffmpeg for faststart remux"),
and adding ffprobe would roughly double that download to obtain one number.

So the probe runs `ffmpeg -hide_banner -i <file>` with no output file. ffmpeg
prints the container header to **stderr**, then exits non-zero complaining
that no output was specified, so the exit status is ignored and stderr is
parsed for the `Duration: HH:MM:SS.ss` line.

That is prose-scraping, which this project otherwise avoids and which is
exactly the objection raised against reading capture health out of
`logs/libobs.log` (#81). The difference is the blast radius. There, a misparse
would put a wrong encoder name into a diagnostics report someone trusts; here
every failure path returns `None` and the column stays NULL, meaning "unknown",
which is what it already said. A duration of zero is treated as unknown too,
since a truncated or still-growing file would otherwise render as a confident
`0:00`.

The parse is a pure function (`probe::parse_duration_s`) with the spawn as a
thin wrapper over it, so the fragile half is unit tested on a box that has no
ffmpeg at all. The catch is that the same absence means the fixture it is
tested against is **written from ffmpeg's documented format, not captured from
a run**, so the tests pin the parser's behaviour, not the wording's accuracy.
Confirming the real wording is a `docs/windows-verification.md` item.

**Cost:** one subprocess per imported file, on the import branch only, since
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
most of the rows, which makes the win-rate tile wrong for as long as they
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
 **Two matches means write nothing.** Not "pick the better one": the ranking
would be invented, and the failure it protects against is silent. A card
labelled with the wrong game looks exactly as plausible as a correct one, and
nobody re-checks a row that looks fine, so the error would live in the library
permanently. An unlabelled row is recoverable; a confident lie is not. This is
the same rule `match_data` applies to participant identity and `team_diff`
applies to team
side: when the answer is not certain, the column stays NULL.
 **Manual, never on startup.** It is a bulk read against the user's running
client, and the moment to do that is theirs to pick, rather than something that
fires while they are loading into a game. It is also cheap: the match-history
list response carries whole game documents, so the pass costs one request for
the
history and one for the summoner regardless of how many rows it labels.

It adds no new writer. The matched game goes through `to_metadata`,
`champion_name` and `update_match_metadata`, the same three the deferred patch
uses, including the same "champion only when NULL" rule.

**It rebuilds the scoreboard too**, for a recording that has none. The
match-history document already carries every participant's champion id, items,
spell ids, perks, level and minion counts, so it costs no extra request, and
`fill_scoreboard` writes only where the column is NULL, for the same reason
`champion` and `role` do. A scoreboard captured live is the game's own account;
a rebuilt one is Riot's afterwards, and is missing what the live path had. The
one visible difference is spells: match history gives ids, the live client
gives names, and both are stored as they arrive rather than one being converted
into the other; converting would need Data Dragon in a path that otherwise
only talks to the League client.

What it cannot do: reach further back than the client's own match history, or
label a custom game, which never gets a match-history entry at all. Both come
back as "matched no game", which the report says in as many words rather than
leaving the user to guess why nothing happened.

### 4.3 Decision: a recording row exists before the recording finishes

Markers used to live in the supervisor's memory for the whole game and reach
SQLite once, at finalize. That was the only behaviour the code had, not a race:
`insert_markers` had exactly one production caller, immediately after
`insert_recording`. So a daemon killed mid-game took every marker with it, while
leaving a perfectly playable file behind. A fragmented MP4 is valid up to the
point it was cut off, which is the guarantee the whole process split is built
on, and the markers had no equivalent.

The half that was worse was the next recording. The Live Client Data API serves
the game's **whole event list**, not the events since the last poll, so a fresh
session starting mid-game ingested everything that had already happened and
attributed it to the file it was writing, at offsets computed against that
file's start. A recording with no markers is a loss. A recording with another
recording's markers at invented timestamps is wrong in a way that looks right.

So a row is written when recording starts, and markers are written as each poll
produces them. Three things follow, and each of them was a decision.

**An in-progress recording is not a library entry.** `finished_at` is NULL until
a finalize or a recovery pass sets it, and `list_recordings` filters on it. The
alternative was a row with a status the UI renders, which reads better in the
abstract and worse in practice: a row left behind by a killed daemon would sit
in the grid forever looking broken. Today one already appears by accident,
because `reconcile` imports the growing file as an unknown recording; the row
existing from the start is also what stops that, since `find_by_path` now finds
it and the scan skips it.

**The finalize matches by id, not by path.** The path a recording starts with is
a prediction (`RecordConfig::expected_output_path`) and the path it ends with is
a fact (`Recorder::stop`). Upserting on a path that moved would finish a
different row and strand the game's markers on an unfinished one that nothing
ever shows, which is worse than the bug being fixed. `insert_recording` stays
for `reconcile`, and as the finalize's fallback for a recording with no id: one
already in flight when this shipped, or one whose start-insert failed.

**Recovery had to land in the same change.** An abandoned row is hidden from
`list_recordings`, so `reconcile`'s orphan sweep cannot see it, and its file is
skipped by the import pass because the row exists. Shipping the start-insert
without a startup pass that finishes those rows would have made a killed
recording *invisible*, where before it at least turned up as an unknown
recording. `recover_unfinished` runs at daemon startup only, which is the one
moment when nothing is recording: from the database an in-progress recording and
an abandoned one are the same thing, so a pass that ran on demand would finish
the row the supervisor was still writing.

The cost is a write per marker rather than one batch per game, which is a few
dozen writes across a thirty-minute game, and rows that exist for recordings
that never finish, which is what the recovery pass is for. The markers are
written twice: once as they arrive, and once at finalize, because a marker
captured during the loading screen resolves against a 1:1 fallback until the
clock is first seen to advance. `AlignmentTracker::fallback` returns the
*first* proven alignment rather than a running average, so every marker after
that point resolves identically at both writes and only the early ones actually
move. The finalize deletes and re-inserts rather than working out which.

**The match summary is covered too, and it was the worst of the three.**
Champion, KDA, game mode and outcome are established by the polls, held in
`session.live` across the whole game by `LiveSummary::absorb`, and were written
once at finalize. A daemon killed mid-game therefore left a recovered recording
whose markers were intact, whose graph was drawn, and whose card had no title
on it. Nothing had to be *inferred* to fix that: the first poll that finds us
in `allPlayers` already knows the champion.

The row is now kept current as the polls land, and two things make that a plain
write rather than a merge. `absorb` never hands back a field it once knew, so
an overwrite cannot erase anything; and this is the live client writing the
columns it owns while it still owns them, unlike `update_match_metadata`, which
merges a second source into a finished row and has to protect what is there. A
`win` that arrives on the last poll of the game has to be able to land on a
column that already held `NULL`, and a `role` that resolves late on one that
already held a value.

**Only when a poll establishes something new.** The comparison is against the
last row that actually reached the database, so a failed write is retried by
the next poll without anything having to remember to. Most polls change
nothing, because a KDA moves on a kill rather than on a tick, and the loading
screen and the end-of-game screen produce repeats and nothing else.

**What stays at the finalize is what the finalize is the source of**: the
duration from the session clock, the file's size, the audio layout and the
diagnostics. The scoreboard blob was on that list and should not have been
(#200): it is read off the polls exactly as the KDA is, so a recovered card had
a champion and a score and no items, spells or runes. It now rides the same
write, held to the same rule, because the session keeps the last poll that
carried a player list and never hands one back for `None`. That makes the write
roughly poll-rate while the game runs, since someone's CS moves most seconds,
which is what the curve already costs; the loading screen, a pause and the
end-of-game screen still write nothing. A recovered recording's scoreboard is
the one standing at the last poll before the kill, which is also where its
footage ends. The resume sweep still replaces it with the LCU's where the game
has a document; the backfill, being fill-only (§4.2), no longer does.

**The game identity rides along with it**, and it is not from the polls at
all: `game_id` and `queue` are read once from the gameflow session at
`InProgress` and were held on the supervisor until the finalize. They are
absorbed onto the session the same way the summary is, so a client that goes
away mid-game cannot hand back an id the recording already read, and the
finalize takes the union of the session's copy and the supervisor's rather
than either alone, because a recording with no successful poll has only the
second.

What that buys is not a label on the card. It is which of the two patch paths
a recovered recording is eligible for. The deferred patch and the resume sweep
need an exact `game_id` and may correct a champion; the backfill matches on the
clock and may only fill (§4.2). A recovered row now carries the id, so
`backfill::resolve_game` uses it and never consults the clock: the identity was
what the client said at the time, and a clock match is an inference drawn
afterwards from two timestamps. It also means a recovered recording is still
resolvable after its game has aged out of match history, where before it was
unmatched forever.

**Samples are covered by the same rule, in the same way.** They were not at
first: a sample was pushed from the poll that produced it but reached SQLite
only at finalize, so a daemon killed mid-game left a recovered recording with
its markers intact and an empty advantage curve behind them. That loss was
less wrong than the markers' had been, because a session starting mid-game
begins its curve mid-game rather than inheriting another recording's, but it
was the same loss. A sample is now written by the poll that produced it, and
the finalize deletes and re-inserts the whole curve exactly as it does the
markers. A poll that does not move the game clock still writes nothing:
`ingest` skips it so that a loading screen or a pause cannot draw a vertical
run of points through the graph, and the live write skips precisely the polls
`ingest` did.

### 4.4 Decision: the library is the first view to cross, and it crosses whole

WS4 is a strangler, so each task moves one view and deletes its vanilla
counterpart in the same commit. The library went first because it is the view
with the most logic behind it and the least coupling to a media element, which
makes it the one where a half-migration would have been most tempting and most
expensive.

**It crosses whole or not at all.** `library.ts` is deleted rather than reduced,
and the `#library-view` markup goes with it. A view rendered by Svelte while its
data still lived in a module that wrote to elements would be two owners of one
piece of state, which is the failure the frontend's whole organising principle
exists to prevent.

Four things were dropped rather than translated, and each was there to work
around `innerHTML`:

- **`render()` and `pendingRender`.** Rebuilding a grid that is not showing is
  wasted work, and doing it as the user navigates back would move the card they
  came from. Svelte does not render an unmounted component.
- **`paintArt` / `paintAll`.** Art is still a second pass, because the first
  sighting of an icon is a CDN round trip and the app has to work offline. What
  has gone is re-finding each row by `data-id` and re-inserting `<img>` tags,
  which was needed only because every re-render threw the previous ones away.
- **`revealRowInspectors`.** The devtools probe answers once and the grid was
  rebuilt many times, so its answer had to be remembered and re-applied to every
  button. The answer is a prop now.
- **Hand-applied escaping.** Every value went into a template string with
  `escapeHtml` or `escapeAttr` around it, chosen per site. Getting that pairing
  wrong is an injection bug with a plausible trigger, since `vodTitle` falls
  back to a filename and `reconcile` imports whatever is in the folder. Default
  interpolation removes the choice.

**The delete confirmation moved from module state to component state**, and that
is the clearest illustration of why the migration is worth doing. A two-step
delete needs to remember which button is armed. In `library.ts` that could not
live on the button, because the grid was rebuilt underneath it, so it was a
module-level id plus a `querySelector` to find the element again plus a timer to
clear it. In `RowActions.svelte` it is a `let armed = $state(false)` beside the
button it describes.

**One thing was deliberately not improved.** A row is a focusable, clickable
`role="listitem"`, which is what the v1 markup did. ARIA has no good role for an
activatable list item, and putting a real button inside every row is a UX change
rather than a migration, so the warnings are suppressed in `Row.svelte` with the
reasoning written next to them. A parity task is the wrong place for a silent
redesign.

**Registration changed shape, and that was a bug worth catching before it
shipped.** A Svelte view has no id to look up before it renders, so it hands its
node to `registerView` when it mounts, which is after `initRouting` has read the
URL fragment. A window opened at `#settings` would therefore have shown the
settings section and the library at once, because the library's node missed the
`showView` that hid everything else. `registerView` now sets `hidden` from the
current view rather than trusting the node's default.

### 4.5 Decision: a view's markup can have more owners than its module

WS4.3 moved one module and one block of markup, and the two lined up.
`#settings-view` did not. Four things wrote into it, and only two of them were
`settings.ts` and `update.ts`.

`status.ts` was the interesting one. It owns the app bar's pills and a poll
timer, and it also owned three rows of the About block, by reaching into
`#about-lcu`, `#about-game-state` and `#about-last-finalized`. That is not a
module doing two jobs by accident: those lines are pushed by a poll on its own
schedule rather than read when the view opens, so somebody outside the view has
to produce them. What was wrong was where they landed. The wording is now in
`lib/settings/about.ts`, where it has tests, and the values go to a store the
component reads. **The poll did not move**, and should not: it is not a
settings concern.

The fourth owner is `index.html`'s app bar, which holds the settings button and
the update dot. It is not a view and WS4.4 does not delete it, so
`appbar.svelte.ts` exists to own those two elements until WS4.6 does. The
alternative was keeping all of `update.ts` alive to toggle one element's
`hidden`. It reads a rune from a plain module through `$effect.root`, and never
tears that root down, which is correct for elements that live as long as the
window.

**WS4.6 has since landed** (#164). The vanilla shell is gone, `index.html` is
one div, and the app bar is `lib/components/shell/AppBar.svelte`;
`appbar.svelte.ts` was deleted with it. The paragraph above is why it existed
for the three workstreams in between.

**Preferences are mirrored rather than moved.** `prefs.ts` keeps the localStorage
cache that the inline boot script in `index.html` reads before first paint,
which is the only thing preventing a theme flash, and SQLite stays the source of
truth. A store that owned preferences would have to reproduce both. So it holds
a reactive copy for controls to bind to, every write still goes through
`savePref`, and `syncFromPrefs` fills the copy in when SQLite answers.

**`theme.ts` is untouched, deliberately.** It owns `html[data-theme]` and the
matchMedia `change` listener that makes "System" follow the OS as it changes.
`Appearance.svelte` asks it to change and never writes the attribute itself,
because a second writer would race the listener. That listener has no test, and
removing it is a silent regression; WS4.4's job was not to give it one, but it
was to avoid being the change that broke it.

The one place the environment pushed back: jsdom has no `matchMedia`, and
`theme.ts` calls it at module scope, so every test that reached the settings
view failed at import. `src/test-setup.ts` shims the environment. Moving the
call would have been the easier fix and the wrong one.

### 4.6 Decision: the player migrated last, and stayed imperative

WS4 moved four views. The player went last because it is the one where the
framework's central offer does not apply: a `<video>`'s `currentTime` is not
state anything should be diffing. It changes sixty times a second while
playing, the element is its own source of truth, and a seek is a command
rather than an assignment. Making it reactive would mean re-deriving a
position that the element already knows, and then fighting it.

So `Review.svelte` holds a real element reference and talks to it directly.
The rAF playhead loop, pointer-capture scrubbing, the stem `<audio>`,
fullscreen and the visibility pause are all imperative and unchanged in shape
from `review.ts`.

**What the migration bought is a defined boundary rather than a rewrite.**
One number leaves the island per frame, the playhead position, which is the
smallest seam that still lets the timeline be declarative. Everything else
crosses inward: the window, the markers, the samples, and callbacks to seek.

The timeline is where that paid. Its five states were five early returns
inside one function, each writing different text into different elements and
each having to undo what the last call did. `graphView` returns which of the
five a recording is in, and the component renders one of five things. The
distinction that matters most is now impossible to lose: a recording where we
were never matched in `allPlayers` has samples but no side, so every diff's
sign is unknowable, and drawing the curve anyway would risk telling someone
they were ahead in a game they lost.

**The store holds the recording, never the player.** `currentTime`, `paused`,
`volume`, the selected track and the fullscreen state stay on the component.
Putting any of them in a store would claim they are shared, and nothing else
has any business reading them.

Two guards changed on the way. `seekTo` and `togglePlay` tested
`Number.isFinite(video.duration)`; they test `review.window.span > 0` instead,
which is the same fact read from the side the rest of the component already
reads, and makes an unknown duration a no-op rather than a seek to zero.

### 4.7 A frontend that boots is not something any gate was checking

WS4.4 shipped a broken `main`. It deleted `#settings-view` from `index.html`
and left `registerView("settings", el("#settings-view"))` behind. `el` throws
on a miss by design, and that line ran before `mountApp`, so the composition
root threw and the entire frontend failed to boot: a window with static markup
and no behaviour at all.

Every gate passed. `biome`, `tsc`, `svelte-check`, 313 unit tests, both builds,
the Rust suite and both Windows smoke tests. None of them was wrong; none of
them was looking. No test imported `main.ts`, and the smoke tests assert that
the *process* reaches `setup` and connects, which is a claim about Rust and
says nothing about whether the webview rendered.

`src/main.boot.test.ts` is the test that was missing. It loads the shipped
`index.html` and the real `main.ts` together, dispatches `DOMContentLoaded`,
and asserts nothing threw and the Svelte root mounted. Every module it reaches
is mocked to nothing, because what is under test is the wiring rather than the
modules.

It is worth having beyond this one bug. **The markup is being deleted a view at
a time**, and `el()` throwing is the designed behaviour for a missing element,
so this exact failure is available on every remaining WS4 task. WS4.6 deletes
the rest of `index.html`.

### 4.8 Decision: the stylesheet moved, and did not scatter

WS4.6's brief was to delete `dom.ts`, the markup and `styles.css`. Two of the
three happened. The third is half done on purpose, and the half that is missing
is the interesting one.

Svelte scopes a component's `<style>` block to that component's own template.
A rule written in one component that matches an element a *child* renders
silently stops applying, and the failure is not an error, a warning or a
crash: it is a box in the wrong place. `.vod-items .vod-slot` is the shape of
it, where the positioning belongs to `Loadout` and the box belongs to `Slot`.
Distributing 1,900 lines of that correctly means placing `:global()` in exactly
the places it is easiest to get wrong.

This repo has no visual test of any kind, and nothing in CI renders a pixel.
So a CSS redistribution done in one pass is a change whose failure mode is
invisible to whoever makes it and whose verification is somebody opening the
app and looking. That is the wrong shape for one commit.

What did happen: the sheet moved to `src/lib/styles/app.css`, the `<link>`
moved with it, and every element it dresses is now rendered by a component. The
rules can be moved into those components one at a time, each verified against a
running window, which is the only way the result is actually known.

**It stays a `<link>` either way.** The anti-flash boot script has to run
before first paint and the tokens it depends on have to be there when it does,
so a stylesheet arriving over a JS import would be the flash that script exists
to prevent.

### 4.9 Two bugs the shell migration turned up, both in `<dialog>`

Neither was in the code being migrated, which is the point of writing them
down: they were in what the migration made testable.

**The dialog answered "no" to "Quit anyway".** `QuitDialog` read
`returnValue` inside its `close` handler, which is the obvious way to tell the
two buttons apart. Closing a `<dialog>` by submitting a `method="dialog"` form
is a *default action*, and it races the click handler that records which button
was pressed: the close event can arrive before the choice is known. The buttons
are `type="button"` now and close the dialog themselves, which puts the two in
an order that does not depend on the environment. Escape still means no,
because it never touches either button.

**A test passed for the wrong reason.** `mount` is synchronous but `bind:this`
is assigned by an effect, so a method called in the same tick finds its element
undefined. `ask()` answers `false` in that case by design, which is exactly
what the "dismissed" test expected, so it passed without the dialog ever having
opened. The fix is a `renderSettled` helper that awaits a tick; the lesson is
that a test asserting a falsy default is the one most likely to be lying.

jsdom contributed a third of its own: it *defines* `showModal` and then throws
"not implemented" from it, so a shim guarded on `typeof showModal !== "function"`
installs nothing and the dialog silently never opens. `test-setup.ts` overrides
it unconditionally and says why.

## 5. Review player

- WebView2 `<video>` element: H.264/AAC MP4 decodes natively, so seeking and playback rate are free.
- Custom timeline component: marker glyphs per event kind, click-to-jump, "next death" / "prev death" hotkeys.
- Later: clip export (`ffmpeg -ss .. -to .. -c copy`, a stream copy with no re-encode; ship a minimal ffmpeg binary or use libobs's muxer).

Implemented in `src/review.ts` + `index.html`'s `#review-view`. The video loads via Tauri's asset protocol (`convertFileSrc`, scoped in `tauri.conf.json` to `$APPDATA/recordings/*`, which needed the `protocol-asset` Cargo feature and not just config).

**The player chrome lives inside `.player-wrap`, and that placement is a constraint rather than a style choice.** `requestFullscreen` is called on `.player-wrap`; the Fullscreen API renders only the fullscreened element's subtree, so a control bar that is a *sibling* of the video is not drawn at all in fullscreen, which is exactly how it behaved. Moving the bar inside the frame is the only fix; no amount of CSS reaches an element outside the subtree. The `:fullscreen` rules in `styles.css` are the other half: without them the embedded `height: auto` still applies and the video renders as a band across the middle of a black screen. Embedded, the element is sized by the recording's own aspect ratio and nothing else. It was capped at `max-height: 60vh`, which kept the full width and made up the difference in black bars above and below, growing and shrinking them as the window was resized. The page scrolls instead. Relying on the window's default height (§12) to keep the player and the timeline both above the fold was not enough, since it holds at that one size and stops holding the moment the window is shorter, which is most windows. So **the player is now capped by height, expressed as a width**: `.player-wrap` takes a `max-width` of the remaining vertical space times the recording's own aspect ratio, published from `review.ts` as `--player-ratio` once metadata lands. Capping the width is the point rather than an implementation detail. A `max-height` is the obvious spelling and reintroduces exactly the letterboxing described above, because the element keeps its full width and `object-fit` fills the difference with black; making the player *narrower* instead leaves nothing to letterbox. The height budget itself (`--player-chrome`) is one deliberately slightly generous number, since a rem too many costs a marginally smaller player and a rem too few puts the ruler back under the fold. The rich `#vod-timeline` is deliberately left outside, so it is unavailable in fullscreen. Duplicating the metric graph, glyphs and ruler into the overlay would be a second implementation of the most intricate widget in the app, and the `[` / `]` / `d` / `D` hotkeys already cover marker navigation there.

**Frame-stepping was dropped** along with that rework. It was a ±1/30s time nudge rather than a true frame seek (no per-recording frame rate is probed anywhere) so it was an approximation presented as precision, and it cost two buttons in a control bar that had to shed width to fit inside the frame. Closely-spaced markers (common near a teamfight) collapse into a single cluster glyph rather than colliding, with `MARKER_PRIORITY` deciding which icon the cluster shows. The library grid, filters, sort, and the stats bar above them are all client-side over the already-fetched row set. That is fine at solo-user library sizes, and would need real pagination/querying if that stops being true.

Verified: layout/CSS visually in a browser (with injected mock data, since a plain browser tab has no Tauri IPC bridge to exercise real `invoke` calls) and a full `cargo tauri dev` launch (asset-protocol config + new `get_recording_markers` command, no capability/schema errors, stable). **Not verified**: the hotkey→seek interaction against real marker data (needs a loaded recording, which needs a live app session to click through manually), and that gap did hide a bug. The hotkey handler stands aside for a focused form control so it does not steal a key the control uses itself, but it applied that to Space *and* the arrows, for any `<button>`. A timeline glyph is a button, so clicking one to jump left it focused and killed seeking entirely until you clicked elsewhere. A `<button>` does nothing with the arrows, so the exemption bought nothing there: Space now stands aside for a button or a select, the arrows only for a select, and a glyph no longer takes focus from a pointer at all. Video playback itself is now testable: `fixtures/sample.mp4` is checked in (§10), and the dev portal's Review-ready seed preset builds a recording around it.

### 5.1 App shell, theming and settings

The frontend is vanilla TS with no framework, split by state ownership
rather than by widget: `router.ts` owns which view is showing, `theme.ts`
owns `<html data-theme>`, `prefs.ts` owns the preference cache, `status.ts`
owns the poll timer, `library.ts` owns the row set and filters, and
`settings.ts` owns the settings form. `main.ts` is a composition root that
owns nothing. `dom.ts` and `format.ts` hold shared primitives, including
`escapeAttr`, which matters because `reconcile` imports any video file the
user drops in the folder, so a recording's displayed name is not
necessarily ours.

**Theming.** `data-theme` is written by JS and only ever holds `"light"` or
`"dark"`; there is no `prefers-color-scheme` media query in the stylesheet.
Resolving the OS preference once, in one place, keeps a single dark block
instead of two and makes an explicit "Light" on a dark OS win by
construction rather than by CSS specificity. The cost is that "System" no
longer follows the OS for free. `theme.ts` listens on the matchMedia
`change` event to put that back, and removing that listener is a silent
regression with no test to catch it.

**The window is not a page.** A webview brings the whole browser with it, and
most of what it brings is meaningless here: dragging across a card leaves half
of it highlighted, right-click offers to reload the app or save the video, F5
throws the UI away mid-recording without the backend hearing about it, and
icons peel off under the cursor as drag images. `desktop.ts` and one CSS block
suppress that. The split is not arbitrary: only CSS can hand selection back
per element (`.selectable`, plus form fields, `code` and `.mono`, because text
the user typed or might want to copy is the one kind worth keeping), and only
JS can see the events.

Both halves are narrow by construction: they suppress browser chrome, never app
behaviour, and each suppression names the one case where it would be a
regression. Text fields keep their context menu, because there it is the
ordinary Cut/Copy/Paste menu a desktop app would show anyway. A build you can
inspect, whether the vite dev server or anything with the `devtools` Cargo
feature, keeps the native menu and the reload key outright, since "Inspect
element" and a reload are the two things most worth having while working on the
frontend.
That check reuses `devportal.ts`'s existing probe (`hasDevCommands` in
`bridge.ts`, memoised) rather than adding a second flag that could disagree
with the Rust side.

Preferences live in `settings_kv` (migration 4), a deliberately unseeded
key/value table: a missing pref means "use the frontend default", so adding
one needs no migration. They also mirror into `localStorage` for exactly
one reason: the inline boot script in `index.html` has to pick a theme
*synchronously*, before first paint, and IPC resolves too late. SQLite
stays the source of truth and wins any disagreement.

**Idle while hidden.** Both of the frontend's continuous costs are now tied to
window visibility: an open VOD is paused (and with it the rAF playhead loop and
the stem `<audio>`), and the 60 s library safety refresh is skipped. Neither is
free to leave running behind a minimised window, and the second rebuilds the
whole grid with `innerHTML`. Note this leans on `document.hidden`, which is
reliable for a minimised window but **not guaranteed** for a window hidden via
`window.hide()`. When the tray work lands, visibility has to be pushed from
Rust instead.

**Status polling.** There are no Tauri events anywhere in this app; every
backend→frontend signal is pull-only. The header's live state therefore
comes from a `setTimeout` chain (not `setInterval`, because `lcu_status` reads a
lockfile and makes two HTTPS round trips, and a slow tick would stack
calls). The interval scales with game state, and the library refreshes
itself off the `Finalizing` edge and `last_finalized.path` rather than
polling `list_recordings`, which would rebuild the grid every couple of
seconds and fight scroll and focus. If Tauri events are ever added on the
Rust side, this whole file becomes a subscription instead.

**No dev panel.** The stub start/stop, LCU check and game-state buttons are
gone. The information they exposed is now always on: the header strip, and
a read-only About block in settings carrying summoner, phase, state, and
the last finalized path with its marker count and `DB WRITE FAILED` signal.
`start_recording` / `stop_recording` / `is_recording` stay registered as
commands (unreferenced from the frontend): `start_recording` carries the
`has_room_to_record` preflight, and dropping them would make
`chrono_stamp` dead code, which fails CI's `clippy -D warnings`.

### 5.2 Decision: the gold curve is Riot's number, not ours

The advantage curve's gold series used to be computed live, by summing the
price of the items each team was holding and adding our own unspent gold. It
was wrong. Not miscalibrated, but wrong in a way no coefficient fixes:

- **The enemy's unspent gold is invisible and ours is not.** Live Client Data
  has exactly one gold field and it is ours, so the estimate is biased in our
  favour by whatever the other team is carrying, which is several thousand
  while five of them are backing.
- **Sold and consumed items subtract from it but not from gold earned.** Every
  potion drunk and every ward placed walks the curve backwards for a player who
  is doing fine.
- **Trinkets and wards price at zero**, so support gold is under-counted on
  both sides, unevenly, depending on who happens to be holding what.

Item value is a lower bound on gold earned whose deficit is unbounded,
per-team and time-varying. A 33-minute game read as a flat band around zero,
which is not the shape of any real game, and the honest caveat in the tooltip
did not rescue a chart that told somebody they were even in a game they lost by
eight thousand. **A lie with a footnote is still the thing people read off the
screen.**

`/lol-match-history/v1/game-timelines/{gameId}` carries per-participant
`totalGold` per frame. `lcu::timeline` sums each side and signs the difference
from ours; `lcu::match_data::split_sides` says which participants those are,
because "which of these ten players are we" already has one home on the LCU
side and a second answer would be the bug #60 documents, again.

**What it costs, and why each cost is acceptable.** Resolution drops from 1 Hz
to one frame a minute, about 35 points for a 35-minute game rather than 2100,
which is no loss, because the renderer already buckets down to roughly a
thousand points and per-second precision on a number wrong by thousands was
fake precision. There is **no live gold** any more, since it does not exist
until the game is over; kill and CS diffs stay live and stay exact. **Custom
and practice games get no gold curve at all**, because they never reach match
history, and that renders as "no gold data for this recording" rather than a
flat zero line, because a zero line reads as "you were even", which is the exact
failure this replaced.

It arrives with the deferred summary patch (§3.1), on the same
`summary_fetcher` seam and the same retry schedule, for the same reason: at
`Recording → Finalizing` the client is in `WaitingForStats` and the gameflow
watch is being torn down in the same transition. No second spawn point, no
second retry loop, no second set of magic numbers.

The frames carry a game clock, so they go through the alignment the 1 Hz
samples already used, recovered from an existing sample row rather than
recomputed, because the API that produced it stops answering the moment the
game ends. A recording with no samples gets no gold: there is no alignment to
place frames through, and a guessed one would draw the right curve at the wrong
times.

### 5.3 Decision: art comes from a CDN, names do not

§3.1 refuses Data Dragon for champion *names*, and this uses it for champion
*art*. That is not an inconsistency; the two are different problems.

A name is a value the library sorts on, filters on and titles cards with, so
it has to be byte-identical to what Live Client Data writes, and the client
is up by definition when the code that needs it runs, so a remote dependency
buys nothing. **Art is the opposite on every count.** Nothing sorts on a
picture, the client is usually *not* running while somebody browses their
library, and Riot publishes the images on a CDN precisely so applications do
not ship them.

**Nothing is bundled.** Files are fetched the first time a champion appears
and cached under `<app data>/ddragon/<version>/`, so the installer grows by
zero bytes and the disk cost is only what the user actually met, and a champion
square is about 7 KB. Drawing both team compositions on the row raised that
ceiling from the champions someone *played* to the ones they were *in a game
with*, which converges on most of the roster; at roughly 170 champions the
whole set is still only about 1.2 MB, and it is bounded by the game rather
than by how many recordings the library holds. `tauri.conf.json`'s
asset-protocol scope covers that directory, so the webview loads them as local
files rather than reaching the network itself.

**Offline is the normal case here, not the edge case.** This is a local VOD
library; people open it with League closed and sometimes with nothing
connected. Every failure returns `None` and the card renders the text it
always did. A missing icon is never an error, never a toast and never a broken
image, which is also why the portraits are filled in *after* the grid paints
rather than being awaited before it.

The version is resolved at most once a day and written beside the cache, so a
session with no network reuses the last known one instead of failing. Art is
**not** pinned to each recording's own patch: a game played on 15.16 drawn with
15.17 icons is not a problem worth a cache generation per patch.

**Art is filed under the champion's key, not its name.** `MonkeyKing.png` is
Wukong's portrait, so `champion.json` is fetched as a display-name to key map.
That is the mirror of what §3.1 does and the reason this cannot be a URL the
frontend builds out of the `champion` column on its own.

**Summoner spell art comes from Data Dragon, and from nothing else.** It
briefly came from two other places. The reasoning was that `img/spell/` is the
pre-refresh icon set and a Flash drawn from it would not match the one in the
game, so art was taken from the running client's own asset store, by
definition what the game is using, falling back to Community Dragon, which
mirrors the same data publicly, for the usual case of browsing with League
closed.

**The art was never the problem.** `img/spell/SummonerFlash.png` is the icon
the game draws. What made spells look broken was `summoner.json` itself:
**it has one entry per game-mode variant, not one per spell.** `Flash` names
three of them (`SummonerFlash` (4), `SummonerFlash_Jade` (74) and
`SummonerCherryFlash` (2202)) and nine other display names collide the same
way. Collecting those straight into a name to id map let `HashMap` iteration
order pick the winner, so a row drew the Arena set's armoured figure on one run
and the right picture on the next. Three sources on top of that meant three
ways for one row to be wrong, and a cache holding `74.client.png`,
`2202.client.png` and `SummonerFlash_Jade.png` side by side with no way to tell
which a row would use.

`spell_art_map` picks deliberately instead: prefer an art key with **no
underscore** (what separates `SummonerFlash` from `SummonerFlash_Jade`), then
the **lowest id** (what separates it from `SummonerCherryFlash`). It is a
preference and not a filter, so a name that exists *only* as a variant, such as
`Fortify`, `Revive` and the rest of the retired set, still resolves to the one
picture there is. The id map is keyed on **every** variant's id but stores the
name's chosen key, so a scoreboard rebuilt from match history carrying 74 or
712 draws the standard art too.

**A spell is renamed in place when it is upgraded, and Data Dragon has an
entry for none of the upgraded names.** The jungle item turns `Smite` into
`Primal Smite`; the 14-minute upgrade turns `Teleport` into `Unleashed
Teleport`. A single Ranked Solo capture (2026-09-07) carried both, with two
players on Primal Smite and four on Unleashed Teleport, so six of the ten
scoreboard rows drew an empty circle for one of their two spells.

`art_key_for` tries the full name first and falls back to its **last word**,
which is the base spell in every one of these (`Smite`, `Teleport`). Listing
the upgrade names instead would need editing every time Riot renames one, and
it has already shipped `Chilling Smite` and `Challenging Smite` under the same
scheme. Trying the full name first is what keeps the multi-word spells that
are spells in their own right, such as `Poro Toss` and `To the King!`, off the
fallback
path, and a last word that resolves to nothing still yields no icon rather
than the wrong one.

Because the fallback happens at lookup rather than at capture, recordings
already in the library get the right icon too; their hover text still says
what the client said at the time.

Both maps come from one parse, and name and id resolve through the same
`spell_art_file`, so the cache holds one `SummonerFlash.png` rather than a copy
per id per source.

**Runes are the odd one out twice over**: the
icon is a path rather than a filename, and it is served from an *unversioned*
part of the CDN. `runesReforged.json` is trees of slots of runes, and a row
wants both the keystone and a tree crest, so all of it flattens into one id to
path map. Items need no map at all: Data Dragon files them under the numeric id
the game itself reports.

**One request per page, not per icon.** A row carries up to twenty-two pieces
of art (a portrait, two spells, two runes, seven items and the ten champions
of the two team compositions) and a library shows dozens of rows, so
`resolve_icons` takes four lists and answers with four maps.

**Six icons in flight, not one and not all of them.** The first version
resolved them strictly in series, on the reasoning that firing a cold cache at
a CDN as a hundred simultaneous requests is how an application gets
rate-limited. That was the right worry and the wrong end of the trade: a page
with fourteen distinct icons meant fourteen round trips one after another,
which is long enough that a person watches the art arrive. `CONCURRENCY = 6` is
what a browser allows per host, for the same reason: a couple of rounds
instead of a couple of dozen, without ever looking like a burst.

The three shared JSON documents (`champion.json`, `summoner.json`,
`runesReforged.json`) are warmed **before** the fan-out, in series. Six tasks
starting against an empty cache would each fetch the same document, which is
the exact duplicate-request problem the cache exists to prevent, multiplied by
the concurrency.

**The frontend paints in chunks of eight rows** rather than waiting for the
whole page. The total wait is unchanged; what changes is that it stops being
one wait for everything, so the rows someone is actually looking at fill in
first. On a warm cache every chunk resolves without a request and it is
indistinguishable from the single pass it replaced.

### 5.4 Decision: the dead ends are skipped, not cut

A recording starts when the client says the game is in progress, which is the
loading screen, which is roughly twenty seconds of a static splash before
anything happens. Opening a VOD landed on it every time.

**The player skips it; the file keeps it.** `review.ts` treats the recording as
a window `[game start − 2s, end]`: playback opens there, the scrubber spans it,
the ruler reads 0:00 at its start, and every seek is clamped into it. Two
seconds rather than zero because cutting to the exact frame the clock starts on
opens a VOD mid-fade with no sense of where it began, and the alignment comes
from a 1 Hz poll so it is only accurate to about a second anyway.

**Where the number comes from is the point.** Markers already know it: every
one is stamped with the game-time to video-time alignment measured during the
game (§3.2), so the difference between a sample's two clocks *is* the length of
the loading screen. The player reads it back out of the samples the timeline
already fetches. No column, no migration, nothing to keep in sync, and it
works on every recording ever made, including the ones that predate this.

**The file is trimmed too, at finalize** (`crate::trim`), which saves the
twenty-odd megabytes a game that the skipped lead occupies. The player's window
stays regardless: it is what makes an untrimmed recording, whether made before
this or produced by a build with no ffmpeg, open in the same place as a
trimmed one.

**It fires only on a measured answer, never a guess.** The two cases split
cleanly, which is what makes automating it defensible. Capture started before
the game and the sample gap says by how much: cut that. Capture started at or
after it, as in a reconnect or a client that reported the game late, and the gap
is zero or negative, so there is no loading screen in the file and nothing is
touched. A recording with no samples has no alignment and is likewise left
alone.

Three things keep it from being reckless. It **probes the result** rather than
trusting the request, because a stream copy cuts on the nearest keyframe and
removes up to a GOP less than asked, so the real figure is what markers and
samples are rebased by. It **refuses** when the measured removal is not close
to the requested one, since that means something other than a stream copy
happened. And it **moves the original aside** rather than overwriting, putting
it back on any failure, including a database write that fails after the file
is already in place, which would otherwise leave every marker out by a loading
screen.

It is best effort throughout: a failed trim is logged and the recording stands
as it was. A VOD with its loading screen still on it is a working VOD.

Whether multi-track audio, stream dispositions and the faststart index survive
the copy was the open question here, and real footage has since answered it:
trimmed recordings play, seek, and still carry their separate stems. It is
borne out rather than proven, since nothing asserts it automatically: CI
runs unit tests rather than video. `dev_trim_lead_in` runs the same path by
hand, for recordings that predate this and for a file the finalize skipped.

A recording whose live poller never came up has no alignment and no samples, so
its window is the whole file, which is the honest outcome, since nothing knows
where its game started.

#### The same argument at the other end

A recording brackets the game on **both** sides. Capture keeps running after
the game window is destroyed, because neither signal that ends a recording
knows at the instant the game ends: `LiveClientDown` waits for five failed
polls at 1 Hz (§3.2, and it waits five because of #74, where one dropped request
used to cost half an hour of a real game), and the gameflow phase trails the
window closing too. A window that no longer exists captures as **black** under
WGC, not as a frozen last frame, so every VOD ended on several seconds of it.

The player's window is therefore `[game start − 1s, game end]`, and both
numbers come from the same place: the last sample is the last thing the game
reported. Playback stops there rather than running on.

**The two ends do not get the same margin, and that is deliberate.** The tail
carried two seconds for a while, so the 1 Hz sample cadence could not clip the
final moment, and it still left VODs ending on black, because two seconds is
not the whole gap. The ends are not worth the same: the head margin buys the
opening of a game, while everything past the last report is the post-game end
screen. Losing up to a second of that costs nothing anyone goes back for, and
a VOD that ends on black is a defect people notice. So the tail margin is
zero and the head margin stays.

**Where this differs from the head, and why it has to.** The loading screen is
bounded, always about twenty seconds, and being wrong about it costs a
few seconds of splash. The tail is not: a stretch where Live Client Data
answered with something the parser could not read keeps recording and produces
*no samples at all*, so real gameplay can sit after the last one. Clipping
there would hide the game rather than the black.

So the tail clip refuses in three cases and falls back to the end of the file
in each: no samples, a tail already shorter than the margin, and a gap wider
than 60 s, which is not a post-game tail, since one is five to fifteen
seconds. That is the same rule `trim.rs` applies to the head: act on a measured
answer, never on a guessed one, and when the numbers do not agree, do nothing.

The file is cut at both ends too, in **one pass**. `-ss` for the front and
`-t` for the length, a duration rather than `-to`, because with `-ss` ahead
of `-i` the output timeline restarts at zero and a stop *time* would be
measured from the new start, which is an interaction whose failure mode is a
file cut in the wrong place.

**The halves have to stay separable, and that is the whole difficulty.**
Markers and samples rebase by what came off the *front*; a tail cut shifts
nothing. One pass reports one duration, so the head component is recovered
from where the cut was told to stop: the output spans `stop_at` back to
wherever ffmpeg actually started, so the difference is what it skipped. That
formula reduces to `before - after` when there is no tail cut, which is
exactly what the head-only trim always computed, so recordings that get only
a head cut rebase by the same number they always did.

Two guards, because rebasing by a wrong number puts every marker out by a
loading screen: the existing keyframe-drift check runs on the head component
only, and a tail-*only* cut additionally asserts that the front did not move.

---

## 6. Disk management (launch feature, not a later one)

1080p60 @ 8 Mbps ≈ **3.5 GB/hour**. A ranked session ≈ 15 GB. Without retention, we fill the user's SSD in two weeks and get uninstalled.

- Retention policy: max total size AND max age, whichever bites first; `pinned` recordings are exempt.
- Enforce on app start and after each `Finalizing`.
- Show current usage in the UI; never delete without the policy being visible to the user.

Implemented in `src-tauri/src/retention.rs`, following the same pure-function-plus-thin-I/O-wrapper shape as `db::reconcile` and `state_machine::machine`: `select_for_deletion` is a pure decision (no I/O, unit-tested directly) over a `RecordingRow` slice + `RetentionPolicy` + an injected "now," and `enforce`/`enforce_now` apply it against the real DB and filesystem. Age is checked first (anything over the limit goes regardless of size), then size (oldest non-pinned recordings removed until under the cap). Usage totals include pinned recordings' bytes (they still occupy disk), but only non-pinned rows are ever deletion candidates.

The policy itself lives in a single-row `settings` table (`db/mod.rs`'s second migration), defaulting to 50 GiB / 30 days rather than unlimited, because this is meant to protect the user out of the box, not only once they find a settings screen, matching this section's "launch feature, not a later one." Either limit can be turned off independently (`NULL` = unbounded) from the settings view's Storage section; current usage sits in the library's stats bar so nothing is deleted as a surprise. `preview_retention_policy` runs `select_for_deletion` as a dry run while the form is being edited, so a tightened limit says what it will delete *before* it is saved, and `set_retention_policy`'s `EnforcementReport`, previously returned and discarded, is now shown after it does. Enforcement runs at app startup (`lib.rs`'s `setup`, after reconcile) and after every finalize (`state_machine::supervisor::stop_recording`), plus immediately when the policy is changed from the UI (`set_retention_policy`) so a newly-tightened limit doesn't wait for the next finalize to take effect.

Record-start preflight (`retention::has_room_to_record`, via the `fs2` crate, since std has no free-space API) refuses to start a new recording under 1 GiB free on the recordings volume, checked from both `Supervisor::start_recording` (the real path) and the dev panel's manual `start_recording` command. Fails open on a stat error rather than block recording over a check that couldn't even run.

Pinning is wired end-to-end: the library's 📌 calls `set_pinned` and refreshes. Recordings can also be deleted individually via `delete_recording`, which shares `retention::delete_recording_and_file` with the automatic sweep. The two differ deliberately on a file that won't delete: a user-initiated delete reports the failure and leaves the row alone, while `enforce` logs and drops the row anyway so an unattended sweep can't stall.

---

## 7. YouTube upload (designed, not built)

- YouTube Data API v3, OAuth 2.0 **desktop flow with loopback redirect** (the OOB flow is dead). Store refresh token in Windows Credential Manager / Keychain, not in the DB.
- **Quota reality:** an upload costs 1,600 units; default project quota is 10,000/day, giving **~6 uploads/day across all users** until Google grants a quota increase (requires an audit). Design consequence: upload is a deliberate per-VOD action with clear failure messaging, never auto-upload.
- Resumable upload protocol is mandatory (multi-GB files, flaky connections).
- Unlisted by default.

## 8. ROFL replays (designed, not built)

- The LCU can download the native replay (~5 MB vs 3.5 GB video), with full camera control on playback.
- Caveats: `.rofl` files only play on the **exact patch** they were recorded on, and playback requires launching the game client. This is a companion to video, not a substitute. Saving both costs almost nothing.

---

## 9. Development workflow

| Layer | Where | Loop |
|---|---|---|
| LCU / Live Client Data / state machine | Dev box, against captured fixtures (`fixtures/`) | seconds |
| VOD library, review UI, upload | Dev box, stub recorder + fixture MP4s | seconds |
| Capture backend | Windows box, `git pull && cargo run` (Rust + MSVC Build Tools installed) | seconds |
| Full integration + Vanguard verification | Windows, CI-built installer | occasional |

- **Run the app with `npm run tauri:dev`**, not `cargo tauri dev`: it passes `--features devtools`, which is what compiles in the dev portal (§10). Without it the portal's window and every `dev_*` command are absent, and the main window hides its own "Dev portal" button accordingly.
- **Never cross-compile the Windows build.** libobs linking + DLL bundling + installer generation via `cargo-xwin` is a fight with no payoff. GitHub Actions `windows-latest` builds the installer (NSIS); download the artifact.
- Vanguard verification (capture works during a real Vanguard-protected game, no flags) is a one-time check per significant capture change, not an iterative loop; capture iterates against any window (browser, video loop), no League needed.
- **CI** ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) is one workflow with four jobs: `test`, `version`, `build`, `release`. The job graph, the staging steps for the libobs runtime, and the release flow are documented in [docs/ci-and-releases.md](docs/ci-and-releases.md). The decisions worth defending here:
  - `test` runs the Rust half **twice**, with and without `--features devtools`. An off-by-default feature is otherwise never compiled by CI, and a broken `#[cfg]` would stay green until someone opened the portal.
  - **Windows is the only platform in the workflow.** macOS left `test` first (the work is platform-independent, and GitHub bills those runners at 10x against the free plan), then `build`, once it was clear a `.dmg` shipping the stub recorder was an installer nobody could record with. The cost is real and accepted: **no CI job compiles the non-Windows paths any more**, so a break in `StubRecorder` or anything behind `cfg(not(target_os = "windows"))` surfaces on the dev box rather than in CI. That loop hits it in seconds, which is why it is not worth a runner.
  - **Pull requests run `test` only.** Three Tauri bundles, two of them Windows, are the overwhelming majority of this workflow's minute spend and artifact storage, and nothing consumes a PR's bundles. A branch that needs an installer can get the full matrix from `gh workflow run ci.yml --ref <branch>`.
  - `build` deliberately does *not* `needs: test`. The two share no output, and gating cost the whole test job in latency before the slow Windows bundle even started. Nothing unreviewed escapes, because `release` needs both.
  - `version` is the commit's distance from the newest real tag, not "highest seen plus one". It is a pure function of the commit, so simultaneous pushes cannot claim the same version and re-running a commit updates its own release rather than minting a second.
  - `release` publishes rather than drafts, because `needs: [version, test, build]` already withholds it until the commit's tests pass, so "published" therefore means "tested", and a human clicking Publish added latency rather than a check. Publishing creates the tag, which becomes the base `version` counts from next time.

  The build is not code-signed yet (no cert configured), so Windows SmartScreen warns on first run. Not to be confused with the *update* signing added in §14, which is configured and does something else entirely.

---

## 10. Dev portal

A second window (`dev.html`) that exercises the whole backend: every command, every table, the retention decision, and the state machine, none of which the app's own UI can reach. It replaces the three-button `#testing-view` that used to live in `index.html`.

**It is compiled out of shipped builds.** The `devtools` Cargo feature is off by default, and everything in `src-tauri/src/dev/` plus the `dev.html` Vite entry is behind it. `npm run tauri:dev` turns it on; `npm run build` cannot even emit `dev.html` (`vite.config.ts` gates the second rollup input on `NINJA_DEVTOOLS`). Availability is detected, not configured: the main window calls `dev_open_portal` and hides its button when the command isn't registered, so there is no second flag to keep in sync.

Why it exists, concretely. Each of these was untestable before:

- **No way to insert data.** There was no seed script anywhere, so the library, its filters and sort, retention, and the entire review player could only be exercised by finishing a real game on Windows. The Seed panel writes real files, rows, markers with the payload shapes `classify_event` produces, and a 1 Hz advantage curve. Retention fixtures use sparse files, so a 3 GiB recording costs a few hundred bytes of disk.
- **The supervisor was only drivable by real League polling.** §3.4 notes its async glue has never touched a real LCU. The Simulate panel dispatches `StateEvent`s into the live supervisor (really starting and stopping the recorder), injects Live Client Data payloads through the real `MarkerTracker`, and replays a scripted game at a speed multiplier until it finalizes into a real row. This is the fixture replay mode §3.3 asked for.
- **Retention deleted files with no preview.** `set_retention_policy` saves *and* enforces. `select_for_deletion` is pure and takes an injected clock, so the Retention panel dry-runs it, including at a fabricated "now", to test an age rule without waiting days.
- **The in-flight recording session was invisible.** `game_state_status` only carries the *last finalized* recording; markers and samples accumulating during a recording could not be observed at all. `dev_session_snapshot` exposes them.
- **`fetch_match_summary` is implemented, unit-tested, and called from nowhere**, which is why every `RecordingRow`'s `role` and `patch` is NULL in practice. `champion`/`win`/`kda_*` come from Live Client Data during the game (§3.2), and `game_id`/`queue` from the gameflow session (§3.1). The portal at least makes the fetch runnable against a real client; wiring it into finalize is still open.

Two changes leaked usefully out of the portal into the app proper. `Supervisor` now emits a **`library-changed`** event after a finalize (and `set_retention_policy` after a deletion), which `src/main.ts` listens for. It is the first backend-to-frontend push in the codebase, and it fixes the standing bug where a recording that just finished stayed invisible until the user pressed Refresh. And `fixtures::enabled()` is now an `AtomicBool` seeded from `NINJA_RECORDER_RECORD_FIXTURES` rather than a per-call env read, so capture can be toggled at runtime instead of only at launch.

`tauri.devtools.conf.json` renames the product and binary to `ninja-recorder-dev` so it is a separate application to Windows. NSIS keys the uninstall entry, the default install directory and the shortcut off `productName`, so while the two shared one, this installer treated the real install as an older version of *itself* and uninstalled it first, a step that aborts the whole install with "Unable to uninstall!" if the old uninstaller returns non-zero or leaves the binary behind (a still-running app is enough). `mainBinaryName` splits the process name too, so neither build's "close the running app" check reaches across at the other; they install side by side. Since #222 it also overrides `identifier`, so the devtools build has its own data folder and the portal no longer opens the release build's library (§17, "Each build has its own data folder").

**Panels, how to get a build with it, and its known limits**, including why the TS command registry is hand-maintained and why seeded placeholder files won't decode, are in [docs/dev-portal.md](docs/dev-portal.md).

---

## 11. Risks

| Risk | Mitigation |
|---|---|
| Riot changes LCU/Live Client endpoints | Unofficial-but-tolerated APIs; fixtures + thin client layer localize breakage. Watch league_record and lcu-driver communities |
| Vanguard behavior changes re: WGC | WGC is a core OS compositor API used by Xbox Game Bar itself, the lowest-risk capture path that exists. No fallback plan needed beyond display capture |
| libobs Rust bindings immaturity | Using a patched fork of `libobs-recorder` (§2.1) rather than raw bindings, but it's still a young, single-maintainer ecosystem and now a fork we own the patch for. Budget time; fallback is a thin C shim over the (stable, C) libobs API. The trait keeps this contained |
| Our `libobs-recorder` fork falls behind upstream | The patch is now two commits, not one (capture source + muxer settings, then multi-track audio §2.5), and the second one touches encoder/source lifetime rather than just settings, so a re-base is no longer free. Still small and self-contained; watch for upstream libobs version bumps we might want (new encoders, bug fixes) |
| ~~Per-app audio capture doesn't work for League~~ **closed** | `wasapi_process_output_capture` is beta in OBS 30.x and the presets naming "game audio" all depend on it (§2.5), so this was the audio risk worth naming. It works: game audio lands on its own track across real Vanguard-protected games. The Desktop preset remains the documented fallback if a driver stack ever refuses |
| Stem playback drifts out of sync | The review player syncs a sidecar `<audio>` against the video by hand (§2.5). Bounded blast radius: playback only, over a file that already exists, fixed by reopening the VOD. Track 0, the default, never uses this path |
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
entry, and a second `[[bin]]` would need an `externalBin` entry and its own NSIS
story, and a flag needs neither.

**Be honest about what this buys.** Not much memory. A *hidden* window keeps
WebView2 fully resident, so "minimise to tray" reclaims nothing on its own, and
simply destroying the window in a single process would capture most of the
remaining win. While the UI is open, two processes cost *more*, by a second host
process. What the split actually buys is **crash isolation**: today a WebView2
crash, or the WebView2 Runtime auto-updating underneath us, takes down an
in-progress recording. It also lets the UI be genuinely absent rather than
merely invisible. The idle-RAM win that matters came from §2.2's capture-backend
lifecycle, not from here.

### The `core` module is the precondition

Tauri v2 has no way to invoke a registered command by name from Rust, since
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
`dev_open_portal`. Both drive the desktop shell, one an opener call and one a
window, and an Explorer window launched from a background daemon can open
behind the foreground app. They stay in the UI process, which only needs
`recordings_dir` to do its job.

### The `rpc` passthrough

Built. `bridge.ts` sends every production command through one Tauri command,
`invoke("rpc", { command, args })`, and `core::dispatch` routes it by name. The
UI now registers two commands instead of twenty-three, and
`dev_registered_commands` derives its list from `core::command_names()`, the
same macro invocation that generates the `match` arms, so the Rust half of the
drift check cannot go stale. Adding a command is two edits (a table row and
`src/dev/registry.ts`) rather than four.

Three commands stay directly registered: `open_recordings_folder` and
`dev_open_portal` drive the desktop shell, and `dev_registered_commands` has to,
because `devportal.ts` detects whether the portal exists by watching that call
reject in a shipped build. Routing it through `rpc` would make it reject in
*every* build and permanently hide the button.

**The cost, stated plainly.** `#[tauri::command]` used to generate argument
deserialization, camelCase→snake_case mapping included. The passthrough owns
that now, and a wrong name or type is a runtime failure rather than a compile
error. Two things hold it down: the table is the only place it's written, and
`every_command_round_trips` exercises every entry with the payload the frontend
really sends. Note the rename applies to *argument names* only; types nested
inside an argument keep their own serde attributes, and because those fields are
usually `Option`, a mis-cased nested key is silently dropped rather than
rejected. That was equally true of the Tauri macro, but it is worth knowing when
adding a nested argument type.

`dispatch` is async because `lcu_status` is; everything else is blocking work,
so `rpc` sends it to `spawn_blocking` and only awaits the one command that needs
it. `core` itself still names no async runtime, so callers choose the thread.

### Launch modes and the window

`main.rs` reads a `Launch` mode out of argv before anything else: `--daemon`,
or nothing. Parsing lives in `src-tauri/src/launch.rs`, pure and unit-tested,
and names no `tauri` type. There was a third, `--hidden`, removed at 2.0.0 by
#71 and covered below.

**The flags are an on-disk contract, which is why they were fixed before the
tray existed.** `tauri-plugin-autostart` writes the flag into `HKCU\…\Run`
once, at enable time; a flag that changes meaning later silently strands every
user who turned autostart on before the change. That is not hypothetical:
"Start on login" below registered `--hidden` before WS3.5, which is what made
removing it a decision with a cost rather than a tidy-up. `--daemon` was
recognised but *rejected with a message and exit code 2* while it was
reserved, rather than falling back to a normal window, because a build that
quietly ignored it would look like it worked while recording nothing.

**The main window is created in Rust, not by `tauri.conf.json`.** `app.windows`
is now `[]`. Tauri creates entries in that array automatically, before `setup`
runs, so there was no way to *not* have a window, and `"visible": false` is
not a substitute: it still constructs the WebView2 instance and pays its full
cost, which is exactly what starting in the tray is meant to avoid. The label
stays `"main"` so `capabilities/default.json` matches unchanged.

Building it from `setup` is safe. The hazard `dev_open_portal` documents,
building a window re-entrantly from inside a WebView2 IPC callback and getting a
blank window, applies to windows created from a command, which `setup` is not.
A window created later from a tray click will have to respect it.

**The default size is derived from the frontend, not picked by eye.** The
content column stops at `--content-max: 1120px`; add the container's padding
and room for a scrollbar and 1200 is the narrowest inner width at which it
reaches full width, so anything narrower squeezes every view and anything wider
only adds background. The 900 height clears the review player and its timeline.
The marker list under them is left to scroll, because a window tall enough to
show it as well would not fit on a 1080p desktop.

Verified on macOS against the real binary: a default start registers a GUI
window, a windowless start stays running, `--daemon` exited 2 while it was
reserved, and unknown arguments are ignored rather than fatal (both OSes hand
launched apps arguments we never asked for). The windowless run also confirms
Tauri's event loop survives with no windows, which is the daemon's
prerequisite. That run used `--hidden`, which no longer exists; `--daemon`
makes the same point more strongly, since it builds no `tauri::App` at all.

### The tray, and what the close button does

The tray is the app's resting state, with three items: Open ninja-recorder,
Settings, Quit. A "Start/Stop recording" item was considered and rejected,
because `start_recording` races the state machine, which doesn't know about the
call (`Supervisor::start_recording` spells out the divergence); promoting a
known-broken dev affordance into the product is not a feature.

**Close is a preference, defaulting to "close the window".** The three values
are `close-window`, `hide` and `quit`, in `settings_kv` under `closeAction`,
parsed by `core::CloseAction`, which falls back to the default on anything it
doesn't recognise, because that table is schemaless and shared across versions,
so a downgrade will one day read a value written by a newer build.

The default is `close-window`, not `hide`, and the difference is the whole
point: a *hidden* window keeps WebView2 fully resident and reclaims nothing.
Destroying the webview while the process lives on is what actually gets the
footprint down, and the recording is unaffected either way. `hide` stays
available for instant reopening. Measured on macOS: a window costs 4 WebKit
handles, and a start that builds none costs 0.

Keeping the process alive after its last window closes is
`RunEvent::ExitRequested`. The discriminator is the exit code: `None` means
user interaction, which here is the last window closing, and gets vetoed with
`api.prevent_exit()`; `Some(_)` means a programmatic `AppHandle::exit`, which
is how the tray's Quit gets out. No "am I quitting?" flag is needed.

**Quit finalizes first.** `Supervisor::finalize_for_shutdown` runs the same
finalize the state machine does, so quitting mid-match writes the row and its
markers instead of leaving a fragmented MP4 for the next startup's `reconcile`
to adopt without them. It runs on a blocking thread, never the main one: tray
menu handlers run on the main thread, and that finalize includes an ffmpeg
remux and a retention sweep, so inline it would freeze the tray and every window
for seconds.

**The tray's "Settings" has two paths** because the window may not exist. A
live window gets a `navigate` event; a cold one is created at
`index.html#settings`, since a frontend that hasn't loaded cannot be listening
for an event yet. `router.ts`'s `initRouting` handles both, and deliberately
ignores a `#review` fragment, because the review view with no recording loaded
is not a state worth restoring into.

`tray.rs` has **no tests and must not grow any**, for the reason
`state_machine::supervisor::on_library_changed` documents: it is reachable only
from `run()`, which is dead code in a test build and gets stripped, keeping the
Win32 GUI import stack out of the test binary. The one testable thing,
`CloseAction` parsing, lives in `core`, which names no `tauri` type.

### Start on login

Recording unattended is worth very little if the user has to remember to launch
the recorder first. `daemon::autostart` registers the app under
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` with `--daemon`, so a
login start is the recorder and nothing else: no window, no WebView2, nothing
that costs anything until it is asked for.

**`--daemon`, since WS3.5.** It was `--hidden` while the daemon was reserved
and exited 2, because registering a flag that launched nothing would have
produced a login start that recorded nothing with no console to say why. The
string goes into the registry once, at enable time, and the build that reads it
back may be years newer, so `launch.rs` owns the flag as a constant with a test
pinning its spelling. Every machine that enabled this before WS3.5 still has
`--hidden` in its `Run` key, and what happens to those is below.

**The daemon writes it, and that took two goes.** §3.1's table always put this
with the daemon, for the obvious reason that the daemon is what login starts.
The implementation did not follow. `Ctx`'s autostart seam was set in `lib.rs`,
in the window, and WS3.4 moved the commands that read it into the daemon, so
for several workstreams the settings row said "start-on-login is not available
in this build" on builds where it was. Nothing failed; an unset seam refuses
with a message. See §17's note on seams for the pattern, of which this was the
third instance.

`tauri-plugin-autostart` could not be the fix because its API hangs off an
`AppHandle` and the daemon builds no Tauri app. `auto-launch`, the crate that
plugin wraps, was already in the tree through it, so the daemon uses that
directly and the plugin is gone. That keeps the behaviour rather than
reimplementing it, and one part of that behaviour is worth naming: `is_enabled`
consults Task Manager's `StartupApproved\Run` override, so an entry that exists
but has been switched off from the Startup tab reads as disabled, which is
exactly what "the registry is the source of truth" has to mean.

**The entry's *name* is as much a contract as its value.** The plugin filed it
under `productName`, and `is_enabled` looks a value up by name without checking
the path, so the daemon uses the same string or every entry written before this
becomes invisible and the checkbox silently unticks itself. It is scoped by
build for the reason the pipe is: the devtools bundle overrides `productName`,
installs beside the release build, and would otherwise share one login entry
with it.

**Off until asked for, and never written by the installer.** Nothing registers
at install or first run; the entry appears only when the settings toggle is
turned on. An app that quietly adds itself to startup is one the user finds in
Task Manager and uninstalls.

**The registry is the source of truth, and this is the one setting not mirrored
into `settings_kv`.** Every other preference is ours alone, but this one has a
second owner: the user can delete the entry from Task Manager's Startup tab, and
policy or another install can remove it. A cached copy in SQLite would be a
checkbox confidently describing a login start that will never happen, so
`get_autostart` reads the platform live and `set_autostart` **re-reads after
writing** and returns that, not what was asked for. A `Run` write can be
overruled; reporting success on "the call didn't error" is how the checkbox
starts lying.

`core` can't name an `AppHandle`, so the plugin sits behind a `core::Autostart`
trait implemented in `lib.rs`, the same seam shape as
`set_library_changed_notifier`. `Ctx::new` leaves it `None`, which does double
duty: the commands are unit-testable against a fake, and **`cargo test` cannot
reach a real registry**. A test that ran `set_autostart` for real on a
developer's Windows box would leave that machine launching the app on every
login, so `None` refuses the write rather than defaulting to the live one.

### Notifications

The window is closed most of the time, so a recording that saved, or failed,
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

The notice is marked seen *before* it is shown, not after; otherwise a broken
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
anything, and the tray icon is the way back in.

This was first written down as "a clickable toast needs a registered COM
notification activator CLSID", which is only half right, and the half it gets
wrong is the interesting one. Investigated properly against
`tauri-plugin-notification` 2.4.0:

- A **COM activator CLSID** (`System.AppUserModel.ToastActivatorCLSID` on the
  Start-menu shortcut) is needed to activate an app that is *not running*:
  the cold-start case, and a toast clicked out of the Action Center after the
  process has exited. It is **not** needed for a toast clicked while the app
  is alive: `ToastNotification.Activated` is an in-process event handler and
  works without one. This app lives in the tray, so the running case is the
  normal case, and that route was assumed closed when it isn't.
- **The plugin is the real blocker, and it closes both routes.** Its Actions
  API (`registerActionTypes` / `onAction`) is documented mobile-only and
  exists only in the crate's `mobile.rs`. Its desktop path builds a
  `notify_rust::Notification` and calls `.show()` inside
  `tauri::async_runtime::spawn`, discarding the returned `NotificationHandle`,
  which is precisely the object carrying the activation-event receiver.
  `notify-rust`'s Windows backend *does* wire `on_activated`; the plugin
  simply throws the result away, and exposes no hook to get at it.

So activation is unreachable through the plugin at any currently shipping
version, CLSID or not. Reaching it means bypassing the plugin on Windows and
driving `tauri-winrt-notification` directly, holding each handle alive and
pumping its receiver, a block of Windows-only code that neither this
development machine nor a dev build can verify (the plugin's own AUMID skip
under `target/debug` is a symptom of the same problem). That was judged not
worth it for the payoff, which is saving one tray-icon click. Recorded here
so the question is not re-opened from the same wrong premise.

Everything in `notify.rs` is best-effort: a notification that fails to show is
a logged warning, never an error that propagates. It is feedback *about* a
recording and must never be able to affect one. Like `tray.rs`, it carries no
tests; the decisions live in `core::NotificationPrefs`, which is tested.

### One notifier, one seam

`Supervisor` now has a single `set_event_notifier` over a `SupervisorEvent`
enum of `LibraryChanged`, `RecordingStarted`, `Finalized` and `RecordingFailed`,
rather than a callback per signal. The finalize toast needed to know *what* was
written, which a bare "something changed" callback cannot say, and adding a
second one-off notifier would have meant a third later. When the recorder moves
into its own process this seam becomes a socket write, and there should be
exactly one place to change it.

### The split, as it actually stands

The daemon runs, serves clients, and owns the recording (§17). The UI is a
client of it: since WS3.4 `invoke('rpc', ...)` forwards over the pipe rather
than dispatching in this process, and the window builds no capture backend,
starts no supervisor, and runs no startup reconcile or retention pass. Killing
it stops nothing, which is the sentence the whole workstream exists to make
true.

The UI starts a daemon when none answers (`daemon::spawn`), so a first launch
on a machine where start-on-login was never enabled still works.

**A connect factory with a side effect was a mistake, and this is the fix.**
`connect_or_start` is handed to `ui::client::spawn` as the thing it calls to
open a connection, and the reconnect loop calls it again on every backoff round.
With a daemon that starts, nobody notices. With a daemon that *cannot* start,
the UI spawned a fresh process every round, each of which flashed a console
window and died, with nothing anywhere saying why.

Two changes, because the loop was only half of it. A cooldown means at most one
daemon is started every fifteen seconds, so a failure is one flash rather than a
strobe, and a daemon killed mid-game still comes back quickly. And the spawned
child's handle is kept long enough to ask `try_wait()` when nothing ever
answers: a daemon that *exited* is a different problem from one that is slow,
and its exit code is the only thing the UI side can learn about why. The error
now names the code and points at `daemon.log`, which is where the reason
actually is.

The daemon checks for updates and installs them since WS3.6.

### Moving execution moved the seams, and three of them were left behind

`Ctx` carries type-erased seams for the things `core` may not name itself: the
library-changed notifier, the autostart implementation, the update requester,
and since #136 the quit requester. Each is set by the process that can provide
it, and `core` refuses when one is absent.

WS3.4 moved every command into the daemon. The seams did not all follow, and
because an unset seam refuses with a *message* rather than failing to compile,
each one shipped as a feature that politely explained it was unavailable.

Three were found by using the app, one at a time, months apart:

- **Quit** had no seam at all. The daemon could not be asked to stop, so the
  window's Quit ended the window and left the recorder running (#136).
- **`tray::request_quit`** finalized through the UI's supervisor, which WS3.4
  stopped starting. It read as protecting a recording and protected nothing.
- **Start on login** is set only in `lib.rs`, so the daemon answers
  "start-on-login is not available in this build" on a build where it is
  (#151).

(A fourth seam arrived later and was set in the daemon from the start: the
capture backends `set_capture_backend` chooses between, WS1.7, #11.)

The pattern is worth naming because it is not a bug in any of those three. It
is one consequence of a correct change, arriving three times, in code that
every gate passed. The compiler cannot help: the seams are `Option`s by design,
and `None` is a legitimate state in the UI, in the tests, and off Windows.

What would help is a check that the daemon sets every seam any command it
dispatches can reach. The seams are a short list on `Ctx`, the commands are a
generated table, and `daemon::start` is the single place that fills them, so
the three facts needed to write that test all already exist in one place each.
Nothing has written it yet, and until something does, the next seam will be
found the same way.

### The check moved because the refusal has to

`tauri-plugin-updater` ran the check in the UI, and its API hangs off an
`AppHandle`, so the daemon could not use it. It also should not: the question
"may this install run now" is answered by whether a game is being recorded, and
the daemon is the process that knows.

What replaced it is two pure functions and a fetch. `update::evaluate` reads the
manifest Tauri's format already publishes and decides what it means;
`update::is_newer` compares with semver rather than as text, which is the bug
that would otherwise have shipped: `0.10.0` sorts before `0.9.0` as a string, so
the first double-digit minor version would have silently stopped offering
updates to everybody. Both are tested against documents rather than a network.

**It was also already broken, in a way nothing reported.** WS3.4 forwarded every
command to the daemon, so `get_update_status` was answered from the daemon's
cell, which nothing had ever filled because the check ran in the UI. The status
was "Checking" forever while a perfectly good check ran in the wrong
process and wrote to a cell nobody read.

The stable endpoint is now written in Rust as well as in `tauri.conf.json`,
because the daemon cannot read the plugin's config. A test pins the two
together, the same way `daemon::IDENTIFIER` is pinned: a mismatch would not
crash, it would quietly check the wrong place forever.

### Installing one

`reqwest` fetches the artifact, `minisign-verify` checks it against the baked
public key, the bytes are written to a temp directory, and the daemon runs them
with `/S /UPDATE /R` and exits.

**`/R` is what starts the app again**, and its absence made the first working
install look like a failed one: the update completed and nothing came back, so
the machine had no recorder until someone launched it by hand. All three flags
are parsed by the generated `installer.nsi`, which makes this list a contract
with the bundle rather than with NSIS in general.

**`/R` carries no `/ARGS`, and that is the decision rather than an omission.**
It relaunches the main binary with no arguments, which is `Launch::Ui`, a
window. The daemon is what was updating and the daemon is what has to exist
afterwards, so `/ARGS --daemon` reads as the more correct answer. It is the
worse one. Someone pressed Install in a window and watched it disappear;
bringing back an invisible background process and nothing else is
indistinguishable from an update that broke the app. A window comes back, finds
nothing listening, and starts a daemon through `connect_or_start`, so both
exist a second later and the visible one is the one that was asked for.

**`installMode: "passive"` in `tauri.conf.json` is dead config.** It is read by
`tauri-plugin-updater`, which stopped performing the install in WS3.6. The
bundle's script does parse `/P` for passive mode, which would show a progress
bar rather than nothing, and that is arguably the nicer experience. It is not
worth taking on trust: `/S` is what has been observed installing correctly on
real hardware, and changing it means verifying the change rather than assuming
it.

Three things about that order are deliberate.

**The manifest is re-read rather than remembered.** It owns the download URL and
its signature, and holding one for up to six hours across a release means
installing something the endpoint has moved on from.

**Nothing is written where it could be run until it verifies.** A download that
does not verify is not an update, it is whatever happened to be served. The
public key is a second copy of what `tauri.conf.json` carries, pinned by a test:
a mismatch fails closed, which is the right way round, and is still worth
checking rather than hoping.

**The artifact is the installer, and this took a first run to find out.** The
code unzipped, on the strength of a comment saying Tauri's NSIS updater artifact
is a zip with the setup executable inside it. That was Tauri v1's shape. Since
v2 `createUpdaterArtifacts` emits `<app>_<version>_x64-setup.exe` beside a
`.sig` signing those exact bytes, and the manifest's `url` points at the `.exe`.
So the daemon downloaded a PE, looked for an end-of-central-directory record
that a PE does not have, and every install ended at "The update is not a
readable archive: invalid Zip archive: Could not find EOCD".

It passed every gate. The download and the signature check are covered against
a local server, the refusal paths are covered, and the one step in between was
described by a comment rather than exercised by anything, because exercising it
needs a published release and an installed build to update from. This is what
§5.0.4 meant by "never run end to end", and it is what running it found on the
first attempt.

Dropping the unzip dropped a whole class of problem with it. An archive's entry
names are remote input and had to be defended against naming a path outside the
temp directory; a file name taken from our own manifest's URL, reduced to its
final component and required to end in `.exe`, has nowhere else to go. The
`zip` crate went with the code that used it.

The refusal while recording is `core::install_update`'s gate, unchanged, and it
is the reason the whole updater is in this process: `installable` takes the
supervisor's view *and* the recorder's own, and either saying yes is enough to
refuse. The cost of a needless refusal is one more click; the cost of a wrong
permit is the game the user was in.

### The tray owns the main thread

A tray icon is a window-station object: its messages arrive on the thread that
created it, and that thread must be pumping a message queue or nothing ever
fires. So the daemon's main thread runs `GetMessage`/`DispatchMessage` and the
tokio runtime lives beside it. That is the concrete reason `daemon::run` is not
a `#[tokio::main]`, and it is why startup is split into `start`, which sets
everything up on the runtime, and a main thread that then waits.

`tray-icon` and `muda` rather than Tauri's wrappers around them, because the
daemon has no Tauri. They are the same crates by the same authors, already in
the tree through Tauri, so this adds no dependency.

**Menu handlers must not block.** They run on the pump's thread, inside
`DispatchMessage`, so each one sends a `TrayCommand` and returns. The exception
is the quit confirmation, which is a `MessageBoxW` and is supposed to block:
that is what a modal is, and the question it asks decides whether a game is
lost.

**Open and Settings cross a process boundary now.** In v1 the tray was in the UI
and could show a window directly. The daemon publishes `Event::ShowUi` instead,
which a connected UI answers by showing itself, and starts a UI when none is
connected. It knows which case it is in by asking whether anything is subscribed
to its events, because a UI that is running is by definition connected.

That event is the one addition to the plan's Appendix B, which calls itself a
draft. It is not a state change like everything else on the wire; it is a
request. The alternative was a second channel between the two processes, which
is a worse answer to "how does one process ask another for a window" than the
channel that already exists.

**Notifications took two commits to land, and the gap between them is worth
recording.** They were raised from the supervisor's event notifier, which went
to the daemon with the supervisor in WS3.4; for those two commits nothing could
raise one, because `tauri-plugin-notification`'s API hangs off an `AppHandle`
and the daemon builds no Tauri app.

Wiring them back into the UI was considered and rejected: a notification that
only appears while a window is open is the opposite of what one is for. So
WS3.3 rebuilt the notifier on `notify-rust` directly, which is the crate the
plugin wraps and was already in the tree through it, and the daemon raises them
now. That is what §3.1's ownership table said all along.

The one platform difference is the `System.AppUserModel.ID`, which exists only
on Windows and is set only for an installed build, because Windows resolves it
through a Start-menu shortcut that a `cargo run` binary does not have. That rule
is the plugin's and it was kept, because it is a fact about Windows rather than
about Tauri.

### Login starts the daemon, and `--hidden` is gone

The Run key points at `--daemon` since WS3.5. Login starts the recorder and
nothing else: no window, no WebView2, nothing costing anything until the user
asks for it.

It waited for two things, and both had to be true rather than nearly true. The
daemon needed a **reachable** tray, which WS3.3 built and which CI then showed
was invisible on a build with no embedded icon resource, so the icon gained a
fourth fallback compiled into the binary. And the UI had to stop building a
supervisor, which WS3.4 did, so opening the app after a login start no longer
means two state machines watching one game.

The argument list is an on-disk contract. `tauri-plugin-autostart` writes it
once, when the box is ticked, and Windows hands it back to whatever build is
installed years later. Every user who enabled autostart before WS3.5 still has
`--hidden` in their `Run` key and will until something rewrites it.

**Q5 asked whether to keep that flag as an alias for one release or drop it at
2.0.0, and the answer is to drop it** (#71). The case for keeping it was that
an old entry goes on working; the case against is that it has no caller. No
code path produces `--hidden`, nothing in the product offers it, and nobody
types it. An alias would be a second name for `--daemon` that exists only to be
found in a registry key, and "for one release" is not a thing the registry
respects: it hands back what it was given for as long as the machine lives, so
the alias would have to be carried forever or removed later with exactly this
cost.

**What the removal costs is one window, once.** The flag is now an unknown
argument, and unknown arguments are ignored, so such an entry is an ordinary
start and a window opens at login. That window starts a daemon
(`daemon::spawn`), and the daemon calls
`RegistryAutostart::refresh_if_enabled`, which rewrites an **already enabled**
entry with the current arguments. So the correction happens on the same login
that showed the window, and the next login is a daemon start.

Two things about that rewrite are deliberate. It only touches an entry that is
already enabled, because start-on-login is the user's choice and an app that
turned it on by itself would be doing something nobody asked for; it changes
what an entry says, never whether one exists. And it writes unconditionally
rather than reading first and comparing: `enable()` on an enabled entry is a
plain overwrite, and writing the right answer is cheaper than working out
whether it is already the right answer.

It is not unit tested, because it writes to the real `Run` key of whoever runs
the suite. What is tested is that the arguments it writes parse back to a
daemon start, so it cannot replace one broken entry with another.
`windows-verification.md` §5.0.2 has the row that checks the rest, and it is
about the window appearing exactly once.

### The tray icon is the daemon's, and only the daemon's

The UI built one too, until WS3.5 removed it. Two processes each building an
identical icon meant two icons in the notification area whenever both were
running, which since WS3.4 is whenever the window is open at all. §3.1's
ownership table always said the tray was the daemon's; this is that being true
rather than nearly true.

What stayed in `tray.rs` is what the window still needs: showing itself, and the
close button's Quit. Those are still the tray's requests, they just arrive from
another process now as an `Event::ShowUi` off the pipe.

---

## 13. Logging

`main.rs` sets `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`, which is what stops a console window flashing over the game. It also means a **release build has no console at all**, so the ~50 `eprintln!`/`println!` calls this app used to make were writing to a closed handle on the one machine where capture problems actually happen. The dev portal's Log panel recorded only the portal's own IPC calls, and said so in its header. In practice the app could not tell you why anything went wrong.

`log.rs` is the fix: a file under `app_data_dir()/logs/`, **written in release builds**, plus one way to write to it.

- **The log ships; the viewers do not.** CLAUDE.md forbids attaching the devtools build to a release, so a devtools-only log could never observe a real failure. The panels that read and present it stay behind `--features devtools` (#72), which keeps the shipped surface to one file and no UI.
- **`error!` / `warn!` / `info!` / `debug!`, each taking the `[tag]` the codebase already wrote by hand.** The tags (`state_machine`, `lcu`, `retention`, `recorder`, …) were already consistent and already greppable; the facade keeps them and adds a timestamp and a level. `error!` is reserved for what a user actually feels: a lost recording, a library that will not open, a deletion that did not free space. Everything that degraded and carried on is `warn!`.
- **`debug!` is off by default**, and exists for the high-volume streams: the per-poll Live Client Data tracker (`live-poll`, shipped; see [docs/recording-pipeline.md §3](docs/recording-pipeline.md)) and libobs's own log (`libobs`, #69). Either at `info` would rotate a session's real errors out of the file within one game. `NINJA_RECORDER_LOG_LEVEL=debug` turns them on. The exception is a poll failure that ends a recording, which is a `warn`; see #74 for why that one has to be visible without asking.
- **A thing that happens on a timer should say so once, not once per event.** The LCU event socket lives two to three minutes and is then re-established. Only the failure was logged, so two of the three closes in an alpha.49 session produced no line at all and the reconnects looked unexplained (#146). It now writes one `info!` when each connection ends, carrying how long it lived, the frame counts by kind, and the close code and reason if the peer sent one. Counting frames rather than logging each is the point: the question is about a whole connection, and a line per frame would bury the answer in the thing it explains. The same reasoning puts it at `info` rather than `debug`, since a line nobody has enabled cannot explain a session that has already happened.
- **Rotation is 5 MiB × 3.** About two play sessions of history, which is the window a capture bug is diagnosed in.
- **Nothing here may fail the app.** A read-only data dir, a locked file, a full disk: each degrades to "no file logging this session", never to an error a caller has to handle. `write` returns `()` and swallows I/O errors, because recording a game matters more than recording *about* recording one. A failed write drops the sink for the rest of the session rather than retrying every line, because the usual causes do not fix themselves.

### Reading it back

Nothing in the shipped app reads the log; the viewers are behind `devtools`, so a release build carries the file and no UI (#72). The dev portal's Log panel reads it through `dev_read_log`, which filters **in Rust**: the file is capped at 5 MiB, which is far too much to hand a webview in one string.

The *parsing* lives in `log.rs` beside the formatter that defines the format, not in `dev/`. A reader that re-describes the format somewhere else drifts from it the first time either changes; a round-trip test through the real `format_line` is what stops that.

Levels include and tags exclude, which looks inconsistent and is not. Levels are four known values a panel can list up front, so ticking them is an inclusion. Tags are discovered *from the file*, so a panel cannot say "everything except the noisy ones" as an inclusion list until it has already read the file once, and the noisy ones (`live-poll`, `libobs`) are exactly what should be hidden on the very first render.

### The libobs worker's log

libobs does not run in this process. `libobs-recorder` spawns
`extprocess_recorder.exe` and calls `obs_startup` **there**, so
`base_set_log_handler` called from here would attach a handler to a libobs
instance we never initialize: it would compile, run, and capture nothing.
The symbol being present in `libobs-sys` is what makes that look like a
local change; it is not one.

What the worker does do is write through libobs's default handler, which
splits by level: **errors to stderr, info and warnings to stdout.** The fork's
`ipc-link` spawns it with stdin and stdout piped, since those carry the JSON
IPC protocol, and **stderr inherited**. So the two halves arrive in different
places.

The errors arrive at our stderr, which in a release build has no console
behind it. So `recorder::libobs::worker_log` points this process's stderr at
`logs/libobs.log` before the worker is spawned, and the child inherits it.
No change to the fork, no IPC change, and, being a file rather than a pipe, no
way to block the worker by failing to drain it. One previous session is
kept as `libobs.1.log`: appending
forever grows unbounded, and truncating outright loses the session that
crashed, which is the one anybody is looking for. A devtools build writes
`libobs-devtools.log` and `libobs-devtools.1.log` instead, for the reason in
"One log file per process" below.

Only in builds with no console (`debug_assertions` is exactly the condition
`main.rs` gates `windows_subsystem` on), because taking stderr away from a
`tauri:dev` terminal would be a downgrade. `NINJA_RECORDER_LIBOBS_LOG=1`
forces it on so the path is exercisable from a dev build.

The info and warnings took longer to find, because the first version of this
assumed the default handler wrote everything to stderr, and a log holding
errors and ffmpeg output looked plausible (#221). They come up the IPC pipe
mixed in with the replies. `ipc-link` reads each line while a command waits
for its reply and hands any line that is not JSON to `log::info!("[rec]: ...")`
in the daemon, which had installed no `log` logger, so every one was dropped:
module loads, "not loaded" warnings, the encoder libobs settled on.
`daemon::log_bridge` is that logger. It writes `[rec]:` lines into the libobs
log through `worker_log`, beside the errors, so libobs's output still reads as
one file. It sends the capture crates' other records to `daemon.log`, and
lets other crates through only at warn and above, so an HTTP or TLS stack's
info lines cannot rotate the session's real errors out. This does not make
`log` the app's logging API: nothing in this crate logs through it, and "Why
not `tracing`" below still holds. The facade is only a way in for crates that
already use it.

Being a pipe, stdout brings back the risk the file avoided. The lines move
only when a command is in flight, and mid-game nothing sends one, so a worker
that logs steadily would fill the pipe and block on its next write, on
whichever libobs thread made it. `IpcLinkMaster::drain_logs` is not the answer: it is not
reachable through `libobs_recorder::Recorder`, and it reads to end of file, so
on a live worker it never returns. Instead `Recorder::collect_output` sends
`IsRecording` every fifth Live Client poll while recording, which reads
whatever is queued on the way to the reply. The answer is used too: a worker
that says it is not recording mid-game gets one warning. The daemon also logs
the encoder it chose and the backend that started each recording itself, so
neither depends on libobs's output arriving.

The costs, stated: the lines land in their own file rather than interleaved
with ours, and they carry no level to filter on, because the formatting is
libobs's and not ours. The dev portal lists the file alongside ours and its
level filter deliberately lets level-less lines through, or selecting it
would show an empty view. Both costs are what a handler *inside* the worker
would fix, and that is a change to the fork, worth making once a real
capture shows it is needed (#69).

### Why not `tracing`

`tracing`, and `log` + `fern`, both do this and more. What was needed was a timestamp, a level, a tag and a file that rotates; `tracing`'s value is spans and structured fields, and nothing in this app has asked for either. This project has kept its dependency tree deliberately small (§1.2), and a date crate would have been a second dependency purely to format a timestamp, so `log.rs` hand-rolls Howard Hinnant's `civil_from_days`, which is the same closed form a date crate would run, and pins it with tests for the leap-year and century rules. Revisit when something genuinely wants spans.

### The three things that deliberately do not go through it

- `launch.rs`'s unsupported-mode message, which runs in `run()` **before** `setup` and so before `log::init`: there is no file yet, and it exits immediately.
- The two lines reporting that logging itself could not start. Saying so through the log would say nothing.
- The `SchemaTooNew` block, which is a wall of actionable prose aimed at a person in a terminal. That one keeps its `eprintln!` *and* gets an `error!` line, so the fact is recorded and the explanation is still readable.

## 14. Updates

CI publishes an NSIS installer for every commit that lands on `main`
([docs/ci-and-releases.md](docs/ci-and-releases.md)), and until now nothing
told an installed build about any of them. `tauri-plugin-updater` closes that,
but the obvious configuration of it is wrong for this app in three separate
ways, and each one is a decision worth writing down.

### It notifies; it does not auto-install

**The NSIS updater exits the app and runs the installer.** That is not a
detail of the implementation, it is what installing on Windows *is*: the
running binary cannot replace itself, so the process ends and a separate
installer takes over. Do that while a game is being captured and the recording
in flight is gone, which is the one outcome this whole application exists to
prevent.

So `update::installable` is a gate, `core::install_update` refuses when it
says no, and a person clicks the button. The three states that refuse are
`WaitingForGame`, `Recording` and `Finalizing`, plus the recorder's own
`is_recording()`. Two sources rather than one, because they disagree for a
moment around a start, and either saying yes is enough to refuse. A needless
refusal costs one more click. A wrong permit costs the game.

`WaitingForGame` is in that list even though nothing is recording yet: the
game is loading and capture starts the moment Live Client Data answers.

The gate is checked twice, in the frontend and again in `core::install_update`,
because the button was rendered at some earlier moment and a game can start
between a glance and a click. `run_update_install` then calls
`Supervisor::finalize_for_shutdown` anyway, the same call, for the same
reason, as `tray::request_quit`.

An "install on quit" variant was considered and dropped. It needs no gate,
because quitting has already stopped everything, but it turns Quit into a
several-minute operation the user did not ask for, at the exact moment they
wanted the app gone.

### Quiet about interrupting, not about telling

These are two different questions and the first draft of this feature
conflated them, which made the panel worse for no reason.

**Interrupting: as little as possible.** Every commit on `main` publishes an
alpha, so on that channel "a newer version exists" is true most days. A toast,
a system notification or a modal on each of them is a thing the user learns to
dismiss without reading, and the one time it matters, they dismiss that too.
So the entire announcement is a dot on the settings button.

Since §15 that argument is specifically about **alpha**. A stable release is a
deliberate act, possibly weeks apart, and the risk there runs the other way:
a dot on a gear is easy to sit beside for a month. Whether stable eventually
deserves something louder is open, and belongs with the channel picker.

Notifications in particular stay out of it. A Windows toast for an update is
an interruption that leaves the app to say something the app could say
better, and it competes for the same channel the *recording* notifications use,
the ones that report a capture failing, which are worth reading.

**Telling: everything it knows.** The panel in Settings → About carries the
version, what changed, and the button. The changelog is not decoration:
installing means restarting mid-session, possibly between games, and that is
the user's call to make. A version number alone is not enough to make it:
"0.9.0 is available" gives nobody a reason to say yes or later.

So the notes ride in `latest.json` (CI writes them from the same `git log` the
release page gets, minus the install caveats, which are written for someone
downloading an installer and are simply wrong in a running app) and are
rendered in the row. **As text nodes, never markup**: the manifest is fetched
over HTTPS but is *not* covered by the update signature; only the installer
it points at is. So everything in it is remote text this app did not write.
Building nodes rather than escaping a string means there is no escaping to get
wrong.

The one thing that does interrupt is the backend *refusing* an install,
because the user pressed a button and is owed an answer. A failed download
reports itself in the row instead, since they are already looking at it.

The check runs 30 s after launch and every six hours after that. Late enough
not to compete with the recorder backend coming up, the database opening or
the first paint; slack enough that it is not re-discovering the same answer
all day.

**And once more whenever Settings is opened**, throttled to one a minute.
Those two are not in tension: the six-hour loop is what feeds the *dot*, and
being slack about it is right. The panel is different: it is only read when
someone deliberately opens it, and that is exactly when it is worth being
right. Without this, an answer computed hours ago is what they read, and a
perfectly correct "Up to date." from before the last release looks
indistinguishable from a broken updater.

**"Not checked yet" is not "cannot check".** These shared one state at first,
and the result was a production build reporting *"Updates are not available in
this build"* for the thirty seconds before its first check landed, the most
alarming possible wording for "hang on". `CheckResult::Pending` is now the
seed and renders as "Checking…"; `Unsupported` is set explicitly, by the one
place that decides this build will never check.

### Windows only

`latest.json` carries a `windows-x86_64` entry and nothing else, which is the
whole story now that Windows is the only platform that builds (§9). A binary
made anywhere else, such as a dev box's `cargo run`, finds no platform entry, and
the About block says updates are not available in this build.

Worth keeping in mind if a second platform is ever added: `.dmg` is not an
updatable bundle format. The updater wants a `.app.tar.gz`, which is a second
bundle target and a second signing path, not a line in the matrix.

### The capture backend has to be shut down first

**An installer cannot overwrite a file another process holds open, and the
capture backend is another process.** libobs runs out-of-process (§2.2) and
comes up as soon as the League client appears, which, for a League recorder,
is most of the time anyone would be using the app. It holds every DLL in the
bundled `libobs/` resource folder open while it lives, so NSIS fails on the
first one it tries to replace:

```
Error opening file for writing:
C:\Users\…\AppData\Local\ninja-recorder\libobs\avcodec-61.dll
[Abort]  [Retry]  [Ignore]
```

That dialog is the *good* outcome. **Ignore** would skip the file and leave a
new `extprocess_recorder.exe` beside an old DLL, which is a version mismatch
that surfaces later as a capture failure with no obvious cause.

NSIS's own "close the running app" check cannot help. It keys off
`mainBinaryName`, and the worker is a different executable it has never heard
of. The same property that lets the production and devtools bundles coexist
(§10) is what makes the worker invisible to it here.

So `daemon::update::install` calls `Recorder::release` after the finalize
and before handing over. `release` is a no-op while recording, which is fine
because the gate has already established that nothing is, and it shuts the
worker down over IPC and waits for it to exit, killing it after three seconds
if it has not. **This was lost once** (#220): the UI's `run_update_install`
did it, and when WS3.6 moved the install into the daemon the release did not
come along. The daemon then left with `process::exit`, which runs no
destructors, so the worker only learned it was orphaned when its pipe closed,
some time after the installer had started copying.

**The installer stops it too**, because an update is not the only way an
installer meets a running worker: someone can run one by hand. Tauri's
`installerHooks` include `src-tauri/nsis/installer-hooks.nsh`, whose
pre-install and pre-uninstall hooks run before the template's own check. They
find the worker **by path**, `$INSTDIR\libobs\extprocess_recorder.exe`, through
the Restart Manager, and terminate what it names. By path because the release
and devtools builds install side by side (§10) and both ship a worker with the
same file name, so stopping by name would end the other build's recording.
The Restart Manager rather than a plugin or a PowerShell one-liner because it
is in Windows, reachable from NSIS's own System plug-in, and answers exactly
"who is running this file"; a quoted PowerShell filter would break on an
install directory with an apostrophe in it, which a per-user install under
`%LOCALAPPDATA%` of an O'Brien has.

When the worker and the app are both running, the hook stops the **app first**
and asks the template's own "close the app?" question to do it. Stopping only
the worker would leave an app that looks up and cannot record, and would
leave it that way if the person then pressed Cancel on the template's prompt.
Asking once, up front, means Cancel stops nothing and OK stops both, and the
template's check that follows finds nothing to ask about.

Two things it does not do:

- **An interactive upgrade that uninstalls first is only covered one release
  later.** Tauri's reinstall page runs the *previously installed* uninstaller
  before the new installer's Install section, so before any hook of the new
  one. An uninstaller from before #220 has no hook, its worker survives, and a
  DLL it had loaded is left behind. The uninstaller this build installs does
  have one, so the next upgrade is covered.
- **Nothing removes a file the new version no longer ships.** Installing over
  an install copies files and never deletes, whether they were locked or not.
  The only way to clear `libobs\` without a generated list of what the new
  build ships would be to delete it before copying, and any abort after that
  point (a Cancel on the template's prompt, a write that fails) leaves the
  installed app with no capture backend. Not done. Today only the devtools
  bundle is ever trimmed, and it never updates itself.

### The devtools bundle must never update itself

A dev bundle that updated itself would download the *production* installer and
replace itself with it. `tauri.devtools.conf.json` renames the product
precisely so the two can coexist (§10), and this would undo that in one click.

So `updates_enabled()` is false under `--features devtools`, and the update
seam is never wired: `get_update_status` reports `Unsupported` and both other
commands refuse. It also means `npm run tauri:dev` never has an updater, which
is what you want locally.

Note that it is a **runtime** `cfg!` and not a `#[cfg]` around the wiring.
Compiling the wiring out under `devtools` would leave `CheckResult`'s variants
and `Ctx`'s two update setters constructed by nothing, which is dead code the
devtools clippy run fails the build over because it runs with `-D warnings`
and without `--all-targets`. This way both configurations compile the same
code and only the behaviour differs.

### Where the code is, and why it is split there

`update.rs` is pure: `decide` and `installable` read no clock, open no socket
and name no `tauri` type. That is the half that can lose a VOD if it is wrong,
so it is the half with the tests. The network half, `run_update_check` and
`run_update_install`, lives in `lib.rs`, which is the only place that can
hold an `AppHandle`.

Between them sits a cell on `core::Ctx` and a single `UpdateRequest` closure,
the same shape as `set_library_changed_notifier` (§12, "One notifier, one
seam") rather than a trait, because a trait here would have to be `async` and
this project has no `async-trait` dependency.

That split buys something concrete beyond tidiness: all three commands are
**synchronous**, and `every_command_round_trips` really does invoke every
command in the dispatch table. With the seam unset, which is what `Ctx::new`
leaves, the update commands answer "not available in this build", so the test
suite never reaches GitHub and `install_update` can never restart the test
binary into an installer.

### The signing key is permanent

Updates are verified with a minisign key pair. The public half is baked into
every installer ever shipped; the private half lives in the
`TAURI_SIGNING_PRIVATE_KEY` repository secret and signs each release.

**Losing the private key strands every install in the field.** They will keep
checking, keep downloading, and reject what they download forever, with no
path forward but a manual reinstall. It is not rotatable after the fact,
because the builds that would need to learn the new key are the builds that
can no longer be updated. Back it up somewhere that is not this repository and
not one laptop.

One-time cost worth stating plainly: **the first release carrying the updater
cannot update anything already installed.** Those builds have no updater in
them. One more manual install, and it self-maintains after that.

### Update signing is not code signing

They are unrelated, and it would be easy to read the `.sig` files on a release
as progress on the other. Update signatures let an installed build verify that
what it downloaded came from us. Code signing is what stops SmartScreen
warning on first run, and there is still no certificate configured; see the
standing caveat block in `ci.yml`'s release notes.


## 15. Release channels and what a version number means

Until this, `version` derived a release from the commit's distance to the
newest tag: `major.(minor + n).0`, published on every push to `main`. It got
builds into hands, which was the point, and it had one genuinely good property
worth keeping. The version was a **pure function of the commit**, so two
pushes landing at once could not claim the same one, and re-running a commit
updated its own release rather than minting a second.

But the number counted commits. `0.184.0` said nothing about what changed, and
there was no defensible moment to call something `1.0.0`, since you would be
picking a commit. Worse, every commit was a full release, so there was no way
to ship something deliberately and no way for a user to ask for only those.

### The number a human moves, and the number CI counts

`package.json`'s version is **what we are building toward**. Only
`npm run release -- next <x.y.z>` changes it, in its own commit.

CI never invents a version. On a push to `main` it appends
`-alpha.<commits since this version was declared>`, so alphas read
`1.0.0-alpha.1`, `1.0.0-alpha.2`, and the base moves only when a person moves
it.

**Anchored on the declaration, not on the newest stable tag.** Those coincide
only when a declaration immediately follows a cut, and the first time they did
not the number was nonsense: 1.0.0 was declared eleven commits after
`v0.188.0`, so its first alpha published as `1.0.0-alpha.11`. The counter is
read as "the Nth alpha of the version being built toward", so that is what it
now counts: `git log -1 -S` finds the commit where the version string entered
`package.json`, which makes the declaration's own build alpha.1. The purity property survives intact: the base comes from a file, the
counter from `git rev-list --count`, and both are functions of the commit.

A stable release is a tag push, and `npm run release -- cut` is the only thing
that should produce one. CI cross-checks the tag against `package.json` and
fails if they disagree, because a tag pushed by hand against a different tree
would publish a release named after a version it does not contain.

**The split is the design.** The script owns the version *decision* and never
invokes a compiler; one that built locally would be back to cross-compiling
libobs (§9). CI owns the build and never invents a version.

### Alphas are prereleases, and that does the channel separation for free

GitHub excludes prereleases from `/releases/latest/download/`, which is the
stable channel's endpoint. Marking alphas as prereleases therefore makes
stable installs ignore them with no filtering of our own.

That matters more than it sounds, because **the channels cannot be separated
by comparison**. The updater's default comparator is plain semver `>`, and
semver says `1.1.0-alpha.1 > 1.0.0` is **true**. A stable install that could
see the alpha manifest at all would be offered alphas by default. So the
separation is by *endpoint*, and there are two manifests:

| Channel | URL |
|---|---|
| stable | `releases/latest/download/latest.json` |
| alpha | `releases/download/alpha/alpha.json` |

`/latest/` cannot serve alpha, for exactly the reason it serves stable so
well. So one permanent prerelease tagged `alpha` holds the alpha manifest and
nothing else, and its single asset is replaced on every build. The installers
stay on their own `-alpha.N` releases; that release only ever points at them.

This also narrows §14. Its "quiet because the cadence is loud" argument is
really about *alpha*, where a release still lands most days. A stable release
is now a deliberate act weeks apart, and the risk there runs the other way: a
dot on a gear is easy to sit beside for a month.

### The guard that will actually fire

Forgetting to declare the next version after cutting a release leaves
`package.json` at a version that is already out, and alphas of it sort
**below** it, since `0.9.0-alpha.1 < 0.9.0`, so every alpha would be invisible to
the updater while looking perfectly fine in the releases list. CI fails the
`version` job in that case, naming the command to run.

### The one-time reset, and what it costs

Adopting this meant declaring a base, and the counter had already run to
`0.184.0`. Declaring `0.9.0` makes the numbers mean something immediately and
leaves `1.0.0` a deliberate act rather than somewhere the scheme walks you
into. But `0.9.0 < 0.184.0`, so **every install already in the field is
stranded**: it will keep checking, keep being told nothing is newer, and never
move. Each needs one manual reinstall.

That was judged acceptable at this size and is not repeatable. Once anyone is
running these builds who cannot simply be told to reinstall, the version can
only go up. `release.mjs` enforces exactly that, since `next` refuses a version
at or below the newest stable tag, so the reset had to be made by editing the
file directly, and the guard stays strict for every version after it.

### The channel picker

A dropdown in Settings → About, an `updateChannel` row in `settings_kv`, and
`UpdaterBuilder::endpoints` chosen per check. **No new commands**: the pref
rides the existing `get_ui_prefs`/`set_ui_pref` pair, so adding it needed no
migration and no dispatch-table row.

Only the *alpha* endpoint is a constant in `update.rs`. Stable's stays in
`tauri.conf.json` and is used as configured, so that URL has one copy rather
than two that drift apart.

**Anything unrecognised reads as stable**, on both sides. A corrupt or
hand-edited value should leave an install on the conservative channel, never
silently opt it into prereleases, and a failed preferences read should not
stop an install checking at all.

Changing the channel re-checks immediately. The alternative is a panel that
goes on describing the channel the user just left, which reads as the setting
having done nothing.

Two consequences worth stating rather than discovering:

**Switching from alpha to stable is a downgrade, and downgrades do not happen.**
Someone on `1.1.0-alpha.3` who switches sees `1.0.0`, is offered nothing, and
sits on an alpha until `1.1.0` ships. The dropdown's hint says so. Fixing it
means a custom `version_comparator` *and* an installer willing to go
backwards, which NSIS has never been asked to do here.

**An alpha release per commit accumulates.** Nothing prunes them yet.

### The installer's shortcut names a binary, and this crate builds two

`main.rs` produces `ninja-recorder`, and `src/bin/gen-contract.rs` produces
`gen-contract`, the emitter CI runs with `--check` to catch contract drift.
Cargo builds both, and Tauri's bundler installs every binary it finds. Which
one the Start Menu entry points at is decided by exactly one field:
`mainBinaryName`. The NSIS template defines `MAINBINARYNAME` from it, and every
`CreateShortcut` in the template targets `$INSTDIR\${MAINBINARYNAME}.exe`.

That field was missing from `tauri.conf.json`, and present in
`tauri.devtools.conf.json`, which had a consequence nobody would guess from
reading either file. The devtools installer's shortcut started the app. The
release installer's shortcut started `gen-contract.exe`, which has no
`windows_subsystem` attribute, so Windows gave it a console. It resolved a repo
root that does not exist on an installed machine, wrote nothing, and exited.

The report it produced was "the app opens a console window for a moment and
then closes", and every part of that was true except the word "app". Nothing
was wrong with the application, which is why there was no crash to find, no log
anywhere, and no `app_data_dir()` at all: the binary that creates it had never
been launched. Roughly a day went into looking for a startup crash that did not
exist.

Two things came out of it. `mainBinaryName` is now set in both configs and
pinned by a test in `launch.rs`, against `CARGO_PKG_NAME` rather than a
literal, so renaming the package cannot leave the config behind. And CI asserts
it against the generated `installer.nsi` on every build, because this is
decided by the bundler after every Rust gate has passed, and no test of ours
runs late enough to see it.

The narrower lesson is worth keeping too: a second `[[bin]]` in a Tauri crate
is not free. The bundler collects every bin target the package produces and
installs them all, so without this field one of them can be chosen, and with it
the others are still there.

So `gen-contract` now carries `required-features = ["contract-gen"]`, a feature
nothing enables but CI and a regeneration. Cargo does not build it, so Tauri
cannot install it, and the install directory holds the application and the
things it needs to run. That is the half of the fix that does not depend on a
config field being right; `mainBinaryName` is the half that still would be, the
day this package grows a second binary that genuinely has to ship.

---

## 16. The capture gate, and what it is allowed to decide

**WS1.5's section. The gate has run, and both P0c stages passed:** Option B
is viable and WS1.6 builds it. The numbers are in
[The measurements](#the-measurements) and the decision under
[The outcome](#the-outcome). The gate is three spikes whose whole purpose is
to produce measurements, and a cell still empty there is a true statement
where a plausible number is not.

What is written before the measurements is the half that did not need
hardware, written before the box ran: what each arm is for, what its result
is allowed to decide, and one premise that turned out to be wrong.

### The three arms

| Arm | Asks | Where |
|---|---|---|
| **P0a** | Can libobs be trimmed to a shippable keep-list? | WS1.1, #5 |
| **P0b** | Can `LibObsRecorder` be driven by a `--daemon` process? | WS1.2, #6 |
| **P0c** | Can we capture and encode without libobs at all? | WS1.3 and WS1.4, #7 and #8 |

P0a and P0b are about the **fallback**: they keep libobs viable as a selectable
second backend for one release (WS1.7). P0c is about **Option B**, the target,
and it is the one WS8 is waiting on, because deleting libobs is what allows the
licence to change at v2.1.

P0c has two stages and they fail independently. Stage 1 is per-application
audio; stage 2 is Windows.Graphics.Capture into a fragmented MP4.

### The premise Q1a was written on is false

Q1a (#67) asked, in advance, whether the licence goal at v2.1 outweighs losing
isolated game audio if stage 1 fails. It assumed the alternative was to keep
libobs and keep the audio.

**Keeping libobs does not keep the audio.** The fork captures per-application
audio with `wasapi_process_output_capture`, which is OBS's process-loopback
source, which is `ActivateAudioInterfaceAsync` with
`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`: the same Windows API
`spikes/p0c-audio` calls. The fork carries no fallback either, and says so in
its own comment: a machine without process loopback "should lose per-app
audio, not all recording".

| If process loopback | libobs isolates | Option B isolates | Licence |
|---|---|---|---|
| works | yes | yes | proceeds, nothing traded |
| fails | no | no | proceeds, nothing saved by staying |

So the licence goal is never opposed by the audio feature, and stage 1's result
cannot decide it either way. That is the same answer whichever way the spike
lands, which is the property the plan wanted from answering Q1a in advance.

**There is no third implementation.** Short of process loopback the only option
is a virtual audio device driver, which needs a signed driver, an installer
that installs one, and the user routing game audio by hand. It is a different
product. Everything else is either injection, which §1.1 forbids and Vanguard
bans, or the session APIs, which control a session's volume and return no PCM.

### Stage 1 can be answered by the shipping build

Because both backends call the same API, `windows-verification.md` §6's first
row answers stage 1 without building a spike at all: record a Practice Tool
game on the **Game** preset and check the track for real samples. Doing that
first makes `p0c-audio` confirm a known answer rather than discover one, which
is the cheaper order and needs no new build.

### What "root PID documented" is asking

#7's exit criterion has a third clause, and it is the design document's first
open question: process loopback captures a **process tree**, so which process
is its root? Audio comes from `League of Legends.exe`, not from the
`LeagueClient*.exe` processes the LCU integration tracks, and a wrong root
produces silence rather than an error. The plan's answer is the game process,
found by its window and `GetWindowThreadProcessId`.

`p0c-audio` does not take that on trust. Before capturing it prints the root,
whether the process name and the game window's owner agree on it, the root's
ancestors and descendants, and where every Discord and League client process
sits relative to it. That turns the clause into a recorded fact about this
machine: whether the game runs under the client's tree, and whether Discord is
outside the game's.

It also captures a control. `PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE`
records everything *except* the game's tree, so Discord should be in that file
and the game should not. Without it, "Discord absent" from the include run
cannot be told apart from "Discord was not playing". The procedure is
[`spikes/p0c-audio/README.md`](spikes/p0c-audio/README.md).

### What the drift row is measuring

#8's "drift under one frame" is the plan's **audio against video** at the end
of a ten-minute sample (§4.5), not video against the wall clock. The first
version of `spikes/p0c-video` measured the second, had no audio at all, and
compared a frame count with elapsed time, which reports the game's frame rate
rather than any drift.

Video goes on a 60 fps grid kept by the performance counter; audio is counted
by the sound card, whose crystal is not that one. WASAPI stamps each packet
with the counter, which is what makes the two comparable. The spike reports
three figures and the row wants all three:

- **Raw**: the card's clock against the counter, uncorrected, with its ppm.
  What a pipeline that timestamped audio by sample count would carry, and so
  the size of the correction WS1.6's resampler has to make.
- **Written**: the misalignment left after the spike slips single samples to
  hold audio on the counter. This is the figure the gate reads.
- **File**: audio end minus video end as ffmpeg decodes the result. Audio is
  padded to the last video tick before finalizing, so an offset here was
  added by the encoder or muxer. Its resolution is one AAC frame, 21 ms,
  which is more than a video frame.

The kill is `TerminateProcess`, what Task Manager's End task calls, so the
killed file is what a crashed daemon would leave. "Playable" is measured, not
looked at: the boxes read directly, a full decode, and the app's own faststart
remux, and then a person opening it. The procedure is
[`spikes/p0c-video/README.md`](spikes/p0c-video/README.md).

### What the spikes are allowed to decide

Naming this in advance is the point of the section, for the same reason Q1a was
answered in advance.

| Outcome | What follows |
|---|---|
| Stage 1 passes | Per-application audio survives into Option B. Nothing else changes. |
| Stage 1 fails | Isolated audio is lost **on both backends**. Presets naming "game audio" have to be renamed, because a preset name that is not true is a bug (§2.5). The licence plan is unaffected. |
| Stage 2 passes | Option B is viable and WS1.6 builds it. |
| Stage 2 fails | Option B is not viable, WS1.7's trimmed libobs becomes the shipping backend rather than the fallback, and WS8 stops. This is the only result that ends the relicensing plan. |

### P0a's keep-list, and where a trimmed backend may go

**The keep-list is read off the directory CI stages, not written from what the
recorder is believed to use.** The first draft was the second kind, and against
the real `libobs_32.0.4` tree it would have removed `srt.dll` and `librist.dll`,
which `avformat` imports and `obs.dll` imports `avformat`, so libobs itself
would not have loaded; `libcurl.dll`, which `win-capture` imports; the D3D11
and WinRT graphics modules; `obs-ffmpeg-mux.exe`, which is the output; and
NVENC, which in that version is its own plugin. Every entry now carries the
import or the call site that justifies it, and Appendix C's step 5 is answered
the same way: `avdevice` and `avfilter` cannot go, because `obs-ffmpeg`
imports the first and the first imports the second.

**A file the list does not recognise stops the trim.** The staged directory is
whatever the fork's highest `libobs_*` folder holds, so a fork bump changes it,
and a keep-list that deletes whatever it does not name would delete a new
dependency without anyone having looked at it. Expected removals are named
too, and a file on neither side is refused rather than removed.

**Until both P0a rows above are filled, a trimmed backend reaches the devtools
installer and nothing else.** It is switched by a manual-run input that only
the devtools matrix entry reads, and the devtools bundle is never published.
The alternative, a repository variable, would have trimmed the next release
the moment somebody set it to try the devtools build. Making the keep-list the
staging step for everything is WS1.7's, after the box has said it records.

### The measurements

Measured on the box on 2026-09-24: Windows 11 build 26200, unelevated, an
RTX 4080 (driver 616.56) at 2560×1440, League patch 26.19. The raw reports
are on the issues: #7 for P0c-1, #8 for P0c-2, #5 for P0a. Two cells are
still empty, and each names the issue that fills it.

| Measurement | Arm | Result |
|---|---|---|
| Trimmed libobs: recording plays, plugin-load log clean | P0a | **Plays**: a full game (ARAM Mayhem, 19:59) plays, seeks, has its markers and every audio stem. **The log is not yet readable**, because libobs's info and warning lines never reach it (#221). What stood in: the trimmed worker loads 23 libobs DLLs to the untrimmed one's 24, and the only one missing is `coreaudio-encoder.dll`, which the keep-list removes on purpose. Re-run the log search once #221 lands |
| Trimmed libobs: bundle size | P0a | **195.4 MB as a release install**, under the plan's 200 MB: 235.5 (release install) − (219.5 − 179.4) (untrimmed and trimmed `libobs\`). CI's trim step: 118 files, 219.4 MB before; 60 files, 179.4 MB after |
| `LibObsRecorder` under `--daemon`: recording finalizes | P0b | Yes. The daemon is the only process that builds the backend (#6), and #130 §3 recorded straight through a UI kill with no restart |
| Daemon-only RAM while recording | P0b | Not measured yet; #6 carries it (`scripts/measure.ps1` during a recording) |
| Process loopback isolates game audio from Discord | P0c-1 | Yes. Include mode: 2,880,000 of 2,880,000 frames over 60 s, 0 discontinuities, peak −16.4 / rms −35.5 dBFS; listened: the game and nothing else |
| Root PID the capture was attached to | P0c-1 | `League of Legends.exe`, found both by name and as the owner of the game window. Its ancestors are `LeagueClient.exe` → `RiotClientServices.exe` → `explorer.exe`, which include mode does not capture; every Discord process is outside the tree |
| Control: Discord audible in the exclude-mode capture | P0c-1 | Yes. Exclude mode: 2,879,040 frames, 0 discontinuities, peak −0.4 / rms −29.0 dBFS; listened: Discord and Spotify, no game |
| WGC frames reach a fragmented MP4 | P0c-2 | Yes: 36,000 frames decoded from 2,000 complete fragments over ten minutes; 59.78 fps delivered, 2.0% of ticks repeated the previous frame |
| Worst drift over ten minutes, in frames | P0c-2 | **0.016 frames written** (QPC clock). Raw device drift −0.2 ppm, and the file's audio ends +0.00 frames from its video |
| A file killed at minute five is playable | P0c-2 | Yes: `TerminateProcess` at 300 s, 999 of 999 fragments complete, 0 decoder errors, 19 frames (0.32 s) lost from the tail, faststart remux ok. Before the remux it has no `mfra`, so a player treats it as live and offers no scrub bar |
| Encoder selected, and what was offered | P0c-2 | NVIDIA H.264 Encoder MFT [VEN_10DE], hardware, vendor matching the adapter. Offered: that one and the Microsoft `H264 Encoder MFT` |
| Software-only encode (#68's second arm): initialises, and holds 60 fps | P0c-2 | Yes: 120 s, 7,200 ticks on the 60 fps grid, worst tick 0.21 frames late, 3.1% repeated, 0 decoder errors. The software MFT reports no friendly name |

**The vendor half of #8's exit criterion cannot be met.** It asks for encoder
detection on two GPU vendors; #68 settled that only NVIDIA and software-only
are available here, so the AMF and oneVPL orderings stay unverified. That is a
recorded gap rather than a pending measurement, and the rows above say what was
offered and whether the software path works, rather than pretending to cover
it. #224 carries the second vendor, for whoever has the hardware.

### The outcome

**Both stages passed, so this is the "Stage 1 passes" and "Stage 2 passes"
row pair of the table above:** per-application audio survives into Option B,
and WS1.6 (#10) builds `recorder/own/` as the default backend. The relicense
at v2.1 stays on course, and the licence itself was settled separately as MIT
(#66); nothing in the gate traded against it.

What the run leaves for WS1.6, none of which reopens the gate:

- **The activation `PROPVARIANT` must not be dropped.** windows-rs 0.62 gives
  it a `Drop` that calls `PropVariantClear`, which frees a `VT_BLOB` pointing
  at the stack. The spike crashed with 0xC0000374 until #218 wrapped it in
  `ManuallyDrop`, and WS1.6's port of that code needs the same.
- **WGC's yellow border has to be turned off**, as libobs turns it off; the
  spike asked for it, and #219 tracks the change.
- **A crashed recording needs the remux before it can be scrubbed.** The killed
  file plays, but with no `mfra` it has no scrub bar until faststart has run.
  Startup recovery (`db::reconcile::recover_unfinished`) only probes the
  duration today and does not remux, so a recovered Option B file would reach
  the library unscrubbable unless recovery learns to remux it.
- **The resampler has little to correct.** Raw drift was −0.2 ppm, at worst
  −0.27 ms over ten minutes, and the spike's single-sample slips held the
  written figure at 0.016 frames.

### The switch, and when it applies

WS1.7's `capture_backend` setting, built ahead of the backend it switches to.
The plan keeps libobs selectable for exactly one release after Option B
ships, so that a recording Option B gets wrong has a fallback a user can pick
without a reinstall, and `RecordingDiagnostics::backend` already says which
backend wrote each file (§4.5 of the plan). The trimmed-libobs half of WS1.7
is not part of this: the trim stays devtools-only until the P0a rows above are
filled.

**It is a `settings_kv` key, `capture_backend = libobs | own`**, spelled as
the plan spells it. No migration: a missing key is the default, the same as
every other key in that table ([data-model.md](docs/data-model.md#what-lives-in-settings_kv)).

**The default is libobs, and WS1.6 flips it to `own`.** Not because libobs is
the preferred answer; the plan's default is Option B. A default the build
cannot construct would refuse every game for everyone who never opened
Settings, and until `recorder/own/` exists that is what `own` would do. The
flip is a one-line change to `CaptureBackend`'s `#[default]`, pinned by a test
so that it cannot happen by accident, and it belongs in WS1.6's own change:
the same change that makes `own` constructible. It moves only the users who
never chose. Someone who picked libobs explicitly has a stored row and keeps it.

**The Settings row is devtools-only until WS1.6, and WS1.6 un-hides it.**
Today the row can offer one backend, with the other disabled beside it, and a
control that changes nothing is not worth a release user's attention. So the
Advanced group, which holds only this row, renders only where the `dev_*`
commands exist: the check the dev portal button already makes
(`hasDevCommands`), rather than a second devtools flag. Everything behind the
row is live in every build: the key, the daemon's choice at startup, the
refusal, and both commands. WS1.6 removes the gate in the same change that
flips the default, which is the change that gives the row something to switch
to. Until then a release build has no way to show the "Nothing will be
recorded" warning, which is acceptable because it has no way to save an
unbuildable choice either: the only writer that bypasses the checks is
`set_ui_pref`, and nothing in a release build calls it with this key.

**A backend that cannot be built is refused, never substituted.** The choice
is `recorder::backend::choose`, a pure function of the setting and what this
build offers, and a chosen backend that cannot be built becomes a
`FailedRecorder` carrying the reason: the refusal path a missing libobs
worker has always taken. It does not fall back to the other backend. The
setting is the user's answer to "which one", and a silent substitution is the
thing that would make a bad recording impossible to attribute.

In practice the refusal is hard to reach, because the setting cannot be
chosen that way. The daemon lists every backend it knows about with a reason
beside the ones it cannot build, the settings row shows the unbuildable one
disabled with that reason, and `set_capture_backend` refuses it again for any
caller that got past the control. What remains is a row written some other
way: a downgrade from a build that had the own backend, or a raw
`set_ui_pref`. The daemon then records nothing, and the settings row (in a
devtools build, until WS1.6) says so in a warning rather than only through a
disabled button.

**A change applies to the next recording, and never to the current one.**
Two answers were available. "At the next daemon start" is simplest, but the
daemon lives in the tray for weeks, so the setting would appear to do nothing
until a reboot. So `set_capture_backend` replaces the backend in place: the
daemon holds one `Arc<Mutex<Box<dyn Recorder>>>` shared with the supervisor,
and the command swaps the box inside it. What makes that safe is the lock and
the gate:

- the swap happens under the recorder lock, which `start` also takes, so the
  check and the replacement cannot be split by a recording beginning;
- it is refused while a game is in progress, by `update::installable`, the
  rule the updater already uses to decide when the live backend may be dropped.
  A game that is loading counts, because capture starts the moment Live Client
  Data answers, and so does a finalize;
- the old backend is `release`d before it is dropped, and the new one is
  `prepare`d if the client is already open, so §2.2's pre-warm survives a
  switch in the client's lobby.

A refusal writes nothing, so the saved value and the live backend cannot
disagree because of one. The daemon also reads the setting once at startup,
which covers the case of a row changed while it was not running.

**Why two commands rather than a pref.** Every other `settings_kv` key is
written by `set_ui_pref`, which writes the row and does nothing else. That is
right for a key the daemon re-reads per use, and wrong for this one: the
daemon has to refuse an unbuildable backend, refuse mid-game, and replace a
live object, and the UI has to know which backends exist before it can draw
the control at all. `get_capture_backend` and `set_capture_backend` carry that,
and `set_capture_backend` returns what the daemon holds afterwards, which the
control renders instead of what it asked for. Both refuse in a process that
does not own the recorder, like `quit_recorder`.

**What only Windows can confirm** is the row in
[windows-verification.md §9](docs/windows-verification.md#9-the-capture-backend-switch-ws17-11):
that switching in the client's lobby leaves one worker process rather than two,
and that the next game records on the backend the row says is in use. The
comparison WS1.7's exit criterion asks for, both backends recording the same
game, waits on WS1.6.

---

## 17. Contract and transport

WS2 made the command and event surface a single declaration in Rust, generated
into TypeScript and checked in CI. This section is the half that carries it:
how a client reaches the daemon once the UI is a separate process.

### Newline-delimited JSON, not a length prefix

Every frame is one `serde_json` value on one line. A length prefix would be
marginally cheaper and considerably harder to debug; this way the protocol can
be read with `cat`, replayed with `echo`, and diffed in a test failure as text.
Nothing here is on a hot path, and the busiest frame is a marker at roughly
1 Hz. JSON cannot contain a raw newline, so the delimiter cannot appear inside
a frame and reading a line is a complete framer.

### Requests carry ids because replies overtake each other

A slow `extract_audio_track` must not head-of-line block a status poll. Commands
are answered on whatever task finishes first, and the client matches a reply to
its request by `id` rather than by arrival order. The daemon never invents an
id; it echoes the one it was given.

Only `lcu_status` is genuinely async. Everything else is blocking work against
SQLite or the filesystem, and runs on `spawn_blocking`, because running it on
the runtime's worker threads would stall every other session behind the slowest
of them and make the ids a promise the transport could not keep.

### Events are a bounded broadcast, and lag is a frame

One broadcast per daemon, one receiver per session, bounded at 512 events. A
client that stops reading must not be able to grow the daemon's memory without
limit, which is exactly what an unbounded channel would let a hung UI do while a
game is being recorded. 512 is more than a whole game's markers and samples, so
a session has to be wedged rather than merely slow to lose anything.

When one falls behind, the oldest frames go and it is told how many, as
`Event::Lagged`. **Not a disconnect:** the client's right move is to re-`hello`
for a fresh snapshot, and it cannot decide that if the socket simply died.

A session subscribes to *topics*, never to individual events, which is what lets
a variant be added to an existing topic without a client change. Subscribing
replaces rather than adds, so narrowing and widening are the same operation and
there is no `unsubscribe` to keep in step.

### A skewed protocol refuses rather than adapts

`hello` carries a version and a mismatch is an error naming both sides. The
updater can replace the daemon under a running UI, so this is a real state
rather than a theoretical one, and a daemon that tried to speak an older dialect
would be guessing at frames it has never seen.

### The server is generic over the stream, and that is load-bearing

Production is a Windows named pipe. The tests drive the same `serve` over a
loopback TCP socket, in milliseconds. That is not a convenience: the alternative
is a protocol whose only exercise is on the Windows box, which is the loop §9 is
organised to stay out of. The transport is the one part of the daemon that can
be tested honestly without Windows, so it is.

**Loopback rather than a Unix socket, which is a correction worth recording.**
The obvious choice was a Unix socket, and it is what the task asked for: it is a
local IPC primitive like the named pipe, and it keeps the protocol in the dev
loop. But CI runs on `windows-latest` and nothing else, so Unix-only tests would
never run there at all, which is the same hole seen from the other side. A
loopback socket runs in both places. One Unix-socket test is kept, gated to
where it compiles, because what it proves is that `serve` is genuinely generic,
and that is the claim the named pipe rests on.

This was found by CI rather than by reasoning: the first version of those tests
named `tokio::net::unix` unconditionally and compiled perfectly on the dev box.
Cross-checking with `cargo check --target x86_64-pc-windows-msvc` does not work
either, because `ring` needs a C toolchain for the target, which is the same
reason §9 refuses to cross-compile the build. For anything that differs by
platform, CI is the only check.

What still needs Windows is the Win32 message pump and the daemon actually
recording a game, which are WS3.3 and WS3.8. The endpoint's name and the
single-instance check landed with the daemon itself, and the sections below say
what they turned out to be.

### The endpoint is the single-instance lock, and there is no separate mutex

The implementation plan sketches a named mutex held alongside the pipe. What
landed is one lock rather than two, and the lock is the endpoint.

The thing worth protecting is the address. Two daemons are a problem precisely
because they would fight over one pipe, one database writer and one capture
device, and the pipe is the first of those to be contended. A mutex held while
the pipe failed to bind, or a pipe bound while the mutex was somehow free, are
both states where "is a daemon running" has two answers depending on which lock
you ask. Binding the address answers it once.

Windows gets that from `first_pipe_instance`, which fails with
`ERROR_ACCESS_DENIED` when another process already has a server on the name.
Unix gets it from `bind`, plus a probe: a socket file outlives the process that
created it, so `EADDRINUSE` on its own cannot tell a running daemon from a
crashed one's leftovers. Connecting separates them. Someone answers, or nobody
does and the file is stale and ours to remove. A daemon that could not tell
those apart would refuse to start after a single crash, forever, with nothing to
show for it but silence.

"Already running" is `Ok(None)` rather than an error, and the process exits 0.
A second launch must never signal the first to quit, because the first might be
recording, and a login start that found a daemon already up is a correct
outcome, not a failure to report.

### The name is scoped by build identity

`\\.\pipe\ninja-recorder.com.ninjarecorder.app.<build>`, where the build is
`release` or `devtools`. Two daemons, one from each build, must not bind the
same name: whichever started first would silently own the other's clients,
which means a dev portal driving the release daemon's recorder, or the reverse.

The name carries the *release* identifier in both builds, even though the
devtools build has had its own identifier since #222. The name is the
single-instance lock, and moving it would let a devtools daemon of the old
name and one of the new run side by side across an upgrade, which is the
two-daemons-one-game failure again.

### The pipe's ACL is explicit, not inherited

A named pipe created with no security attributes gets the creating process's
default DACL, which on a normal account grants that user and `SYSTEM` full
control. That is nearly what is wanted, and "nearly" is doing real work here:
the default comes from the token, can be widened by policy, differs for a
service account, and is written down nowhere a reader of this code would find
it.

So the daemon builds one: `D:P(A;;GA;;;<user>)(A;;GA;;;SY)(A;;GA;;;BA)`. A
protected DACL, so nothing is inherited and the list is the whole list, allowing
this user, Local System and the built-in administrators group. No `Everyone` and
no `Authenticated Users`, because another account signed in to the same machine
is exactly who this keeps out. System and Administrators are in rather than out
because both can take ownership of anything on the machine regardless, so
excluding them would buy nothing and would stop an administrator diagnosing a
stuck daemon.

It matters because the daemon is not a passive thing to connect to. Whoever
opens that pipe can start a recording, delete recordings, and run everything in
the command table.

**SDDL rather than hand-built ACLs.** `InitializeAcl` plus `AddAccessAllowedAce`
plus `SetSecurityDescriptorDacl` is four allocations and three chances to get a
length wrong, in `unsafe`, for what SDDL says in one line. The string is also
checkable against the live pipe with one PowerShell command, which is worth a
lot for something only verifiable on Windows
([docs/windows-verification.md](docs/windows-verification.md) §5.0.5).

**A failure degrades rather than refuses.** If the SID lookup or the descriptor
fails, the daemon logs it and creates the pipe with default attributes, which is
what it did before this existed. Refusing to start because it could not look up
its own SID would be the worse outcome.

### Paths without an `AppHandle`

The daemon builds no `tauri::App`, so it cannot ask one where anything is. It
resolves the same paths itself, following Tauri's rules rather than inventing
its own: `app_data_dir()` is `dirs::data_dir()` joined with the identifier, and
bundled resources sit beside the executable. `dirs` is the crate Tauri itself
uses, at the version it uses, and was already in the tree through it, so this is
the same function producing the same answer rather than a second implementation
of one rule.

The identifier is the one string both processes have to spell identically, and
it is now written in two places: `tauri.conf.json` (and, for the devtools
build, `tauri.devtools.conf.json`), which Tauri reads, and `daemon::IDENTIFIER`,
which the daemon reads. The UI resolves its data paths through the same
`Paths::resolve`, so within one build the two processes cannot disagree; the
configs still matter for what only Tauri decides, the asset-protocol scope
first. A test parses the first and
asserts the second, because getting this wrong would not crash anything. The
daemon would open a different database in a different folder and record
flawlessly into a library the UI has never heard of, which is the kind of bug
that is found weeks later by a user with no recordings.

### One log file per process

`log.rs` used to write one `ninja-recorder.log`. Two processes sharing it would
be two independent sinks appending to one file and, worse, each rotating the
other's file out from under it, since a rename is not something the other
process can be told about. So the stem names the role: `ui.log` and
`daemon.log`, both under `app_data_dir()/logs/`, which is what the ownership
table said all along. The dev portal lists its own files and picks the daemon's
up through the same directory scan that already finds `libobs.log`.

**And one per build** (#202). The devtools build then kept the release identifier
so its portal read the real library, which put both builds' logs in the same
directory, and with both installed that is the same problem again one level up:
two daemons appending to one `daemon.log`, lines interleaved with nothing to say
whose they are, each rotating the other's file away. It cost a verification
pass real time reconstructing which daemon recorded which half of a game from
the `listening on` lines. So a devtools build writes `daemon-devtools.log` and
`ui-devtools.log`, the way its pipe name and Run value already carry the build,
and a release build keeps the names every shipped version has used. Each
process's first lines also name its version, build and pid, because a file name
separates builds but not a reinstall, an update or a restart of the same one.
The build suffix was the whole of that change: the data directory, the database
and the recordings folder stayed shared on purpose, until #222 showed what that
cost (below). With the folders split the suffix is redundant, and it stays:
a file copied out of its folder still says which build wrote it.

The libobs worker's file follows the same rule, one fix later. Nothing in
`log.rs` writes it, but each daemon rotates it by hand when its capture worker
first starts, so a release daemon starting capture pushed a running devtools
daemon's live `libobs.log` to `libobs.1.log` and deleted the one before it, and
the other way round. A devtools build now writes `libobs-devtools.log`; the
names come from `log::libobs_file_names`, which sits outside the Windows-only
recorder so the test pinning them runs everywhere.

### Each build has its own data folder

The devtools build's identifier is `com.ninjarecorder.app.devtools` (#222), set
in `tauri.devtools.conf.json` and mirrored by `daemon::IDENTIFIER` under
`--features devtools`. Everything under the app data folder is per build: the
library, the recordings, fixtures, the Data Dragon cache and the logs.

**Why.** Until then both builds used `com.ninjarecorder.app`, so the dev portal
would read the real library. Logs and the pipe had already been split per
build, but with both running the two daemons still shared everything else, and
a verification pass (#202, #212) found what that costs. Both daemons saw the
same game start from the same client, derived the same file name from its
start time, and recorded into one path. One remux failed on a locked file and
the other on a file that had just been replaced; a full decode found 23 H.264
errors in one game and 5,024 AAC errors in the other. One `recordings` row was
missing, and both daemons ran the resume sweep over the same rows. Each build
recorded one corrupt file per game where two good ones were expected.

**Alternatives rejected.**

- *Put the build in the file name* (`recording-<ms>-devtools.mp4`) and scope the
  resume sweep to rows its own build created. That separates the files and
  nothing else: one database would still take two writers from two processes
  that each believe they own it (§3.1's ownership table), both would run the
  startup reconcile over one folder and import each other's files, retention
  would delete from a list the other was still recording into, and every new
  per-recording writer would have to remember the build. The folder split
  separates all of it in one place, and matches how logs and the pipe already
  split.
- *Refuse to record when the other build's daemon is running.* It would keep
  the files apart, but side-by-side testing is exactly what the devtools build
  is for, and it would make recording depend on another process's state.

**The cost, accepted.** The portal no longer sees the release build's library.
A devtools install starts empty, including one upgraded from a build that
shared the folder; its data comes from the Seed panel, games it records itself,
or recordings copied into its own folder and picked up by the startup
reconcile ([docs/dev-portal.md](docs/dev-portal.md#it-has-its-own-library)).
Inspecting a release library now means copying it, which is also the safer way
to point a tool with raw SQL and a DB wipe at it.

**What moves with the identifier.** Tauri keys more than `app_data_dir()` off
it: the WebView2 profile (`%LOCALAPPDATA%\<identifier>`), the AppUserModelID the
installer gives the shortcut and `daemon::notify` attributes toasts to, and the
uninstaller's "delete application data" option. All of those are better split:
uninstalling the devtools build with that box ticked used to delete the release
build's library. The install directory, uninstall entry and Run value were
already keyed off `productName` and do not move. Updates do not apply, since a
devtools build never updates itself (§14).

**The recordings folder is not a setting**, so the two builds cannot be pointed
at one folder by configuration. If it ever becomes one, a folder shared between
builds is the #222 failure again, and the setting needs to refuse it or say so.

### Startup and shutdown order

The log is opened first, before anything that can fail. That is a correction:
it used to come *after* the bind, so that a second daemon finding the endpoint
owned would touch nothing at all.

The cost of that ordering was found the hard way. A daemon that died before the
bind left no trace anywhere, and on Windows `main.rs`'s stderr goes nowhere in a
release build, so the only symptom was a window that flashed and closed. An hour
went into diagnosing that from a machine that cannot run the binary, and the
answer was never in a log because there was no log.

The property the old ordering protected is kept anyway. Rotation happens on
*write*, past 5 MiB, and the quiet path writes nothing: it opens the file, finds
the endpoint owned, and exits. The "logging to ..." line is emitted after the
bind succeeds rather than at `init`, which is what keeps the second daemon
silent, and which was itself found by measuring rather than by reasoning about
it.

The endpoint is still bound before the database, because binding it is the
single-instance check.

Shutdown runs the other way: stop accepting, publish `DaemonShuttingDown` so a
connected UI can say why it is about to lose its connection instead of showing a
dead pipe, then finalize whatever recording is in flight. That last step is the
one worth the wait. Killing a daemon mid-game leaves a fragmented MP4 with no
row, recoverable only by the next startup's reconcile and stripped of its
markers, so a clean stop finalizes first and exits second.

### The UI opens its log before the builder, for the same reason

The daemon's ordering above was corrected first, and the UI was left as it was:
`log::init` ran as the opening statement of Tauri's `setup` hook. That reads
like the earliest point there is, and it is not. Four plugins initialise before
it, the generated context loads before it, and the whole of
`tauri::Builder::build` runs before it, the windowing runtime included. Every
one of those reports a failure that reached
`.expect("error while building tauri application")`, which aborts with a
message on a stderr that a `windows_subsystem = "windows"` build sends nowhere.

The symptom was the same one, reported from the field a second time: an app
that flashed and closed, with `app_data_dir()` not existing at all. The missing
directory is not incidental, it is the diagnosis. `log::init` creates it, so
its absence proves the process died before the first line of `setup`, which is
exactly the window the UI had no logging for.

So `run` opens the log itself, before the builder, through
`daemon::Paths::resolve` rather than `app.path()`: there is no app yet, and
resolving those paths without one is what that function exists for. `build()`'s
failure is no longer an `expect` either. It logs the reason and exits 3, beside
`daemon::run`'s 2 and the library's 1, so the three fatal starts stay
distinguishable to a caller that can see nothing but an exit code.

What this cannot cover is a process that dies in the loader, before `main` runs
at all. That leaves no log by construction, whoever opens it. The difference is
that the silence is now itself a result: a launch that still writes nothing has
ruled out everything after the loader, which is the half of the search space
this could not previously separate.

### A setting the daemon acts on gets its own command

`set_ui_pref` writes a `settings_kv` row and nothing else, which is enough for
every key the daemon re-reads when it needs it. `capture_backend` is the first
key where writing the row is not the change: the daemon has to validate it
against what this build can construct, refuse it mid-game, and replace a live
`Recorder`. So it has a `get_capture_backend`/`set_capture_backend` pair
(WS1.7), and the setter returns the daemon's state afterwards rather than an
acknowledgement, the shape `set_autostart` already has. The next setting that
has to take effect in the daemon rather than be read by it should follow that
shape, not `set_ui_pref`'s. The reasoning is §16's "The switch, and when it
applies".

---

## 19. The Svelte migration: what WS4.1 decided

WS4 replaces the frontend view by view rather than at once, so the decisions
below are all about the same thing: making two frontends coexist in one window
without either becoming the other's problem.

### The root mounts beside the views, not around them

The obvious shape for a framework migration is to put the new root at the top
and let it render the old markup underneath. It is also the shape that makes
every later step a merge conflict: the vanilla views and the Svelte tree would
both have an opinion about layout, and `index.html` would have to be rewritten
on the first commit instead of the last.

So `#svelte-root` is a sibling, the last child of `.container`, empty in the
markup and empty at runtime. An empty div in a block container takes no space,
which is what lets the seam land without the existing views moving a pixel.
When WS4.3 moves the library across, the component renders in the same place,
at the same width, as the section it replaces.

### `router.ts` owns the join

Not `main.ts`, and not a new module. The router already owns the only question
both halves have to agree on, which is which view is showing, and it was
written in the first place because two files were toggling each other's
`hidden` attributes. A Svelte root that managed its own visibility would
reintroduce exactly that, with a compiler in front of it.

### Mount once, and do not tie it to view changes

`mount` and `unmount` destroy component state. Driving them from `showView`
would look tidy and would throw away a migrated view's scroll position, its
filter selections and its in-flight requests every time the user glanced at
Settings. Visibility stays what it has always been: the `hidden` attribute on
the host.

`unmountApp` exists anyway, and nothing in the app calls it. Mounting that
cannot be undone is mounting that cannot be tested, and "an empty `App.svelte`
mounts and unmounts without affecting the vanilla views" is the whole of what
WS4.1 claims. The test suite is the caller.

### The tokens moved, and nothing else did

Every custom property left `styles.css` for `src/lib/styles/tokens.css`
unchanged: same values, same selectors, same order, verified by diffing the
declaration sets before and after. Components need a token source that outlives
the stylesheet WS4.6 deletes, and that was reason enough to move them; it was
not reason to also revise them. A commit that moved and rewrote them at once
would have no readable visual diff, and the next person to change a colour
would be reading two changes at once.

`styles.css` imports the file on its first line rather than `index.html`
loading it as a second `<link>`, and no component imports it either. Both
alternatives put the tokens after first paint, which is the theme flash the
inline boot script in `index.html` exists to prevent.

### Two type-checkers, on purpose

`tsc` does not look inside `.svelte` files. That is why `svelte-check` had to
land in the same task as the first component rather than in WS5.6 afterwards:
a gate that narrows silently is worse than one that was never there, because it
goes on reporting success while covering less each week.

It could not simply be added, though. `svelte-check` peer-requires
`typescript` at `^5 || ^6`, and this tree had moved to 7.x, the native port.
Downgrading would have given back the whole-project check's speed, and pinning
`svelte-check` to an older Svelte was not a real option either. So both are
installed: `typescript` at 6.x for `svelte-check`'s language service, and the
7.x native checker under the `@typescript/native` alias for `npm run
typecheck`.

The cost of that is a genuine trap. Both packages ship a binary called `tsc`,
and which one `node_modules/.bin/tsc` resolves to is decided by install order,
so `npx tsc --noEmit` runs whichever won a race. Every script and every CI step
now names the checker it means by path, and `npx tsc` should not appear in this
repository again.

`check:svelte` also does not pass `--tsgo`, which would run the Svelte gate on
the native checker too. It is about a second faster and writes a shadow
TypeScript project into `.svelte-check/` that is not pruned when a component is
deleted, so the gate goes on failing over a file that is no longer in the tree,
citing a path that does not exist. A second on a run measured in minutes does
not buy that.

### 19.1 The dev portal was reworked, not retired

Plan §9, Q6 asked whether the portal should survive WS4 at all, and the answer
was to bring it across rather than let it rot behind the app it exists to
debug. It was the last hand-built markup in the repository: eleven panels, a
hash router, and a module of markup builders with a hand-applied escape at
every interpolation site.

**It is the one part of the migration that removed a workaround rather than a
file.** A panel was an object with `mount(root, ctx)`, called again on every
navigation and on every `refresh()`. Panels bound delegated handlers to the
element they were handed, and `unmount` was not given a reference to it, so
there was no way to unbind them: mounting onto the same element stacked one
handler per mount. Two live handlers turn a single click into two toggles, a
no-op that renders once on the way through, and the Log panel's tag chips lit
up and reverted inside one frame because of it. Handlers left behind by *other*
panels were worse, since hooks like `[data-reload]` were not unique across the
set. The fix was to swap `#dev-main` for a shallow clone of itself before every
mount. Svelte destroys a component's handlers with the component, so the clone
and the paragraph explaining it are both gone.

The `onHealth` hook went for a related reason. A panel could opt into the 1 Hz
health tick to patch one element in place, because a full redraw would have
wiped whatever was typed into a textarea. Form values are component state now,
so the state strip reads the shared poll and the textarea beside it is
untouched.

**What did not come across is `refresh()`.** Most of what it did was re-run a
`draw()` a panel could not trigger itself, which is what state does for free.
What is left is a generation counter: `r` and a `library-changed` event force
the current panel to be destroyed and recreated, which re-runs its load from
scratch. That is the half of `refresh()` that was never about rendering.

The panels' pure decisions moved out into `src/lib/dev/` on the way, and are
covered by tests for the first time: the argument coercion whose rule is that
an omitted optional must be absent rather than null, the level filter that must
never let the last level be turned off, the cell parser that falls back to text
when an object literal will not parse, and the seed presets, whose whole value
is that each one exercises something that otherwise needs a real game on
Windows.

#### The stylesheet moved; it did not scatter

`src/dev/styles.css` is `src/lib/styles/dev.css`, still one global sheet
loaded by a plain `<link>` in `dev.html`. This is the same call §4.8 made for
the app's own stylesheet, for the same reason: Svelte scopes a component's
`<style>` block to that component's own template, so a rule that matches an
element a child renders stops applying, silently, and nothing in this repo
renders a pixel in CI. The portal has no visual tests and no Windows
verification of its own. Moving rules into components is safe work, a few at a
time, checked against a running window; doing it blind in the same commit that
rewrote every panel would have been a large invisible-failure surface for no
gain.
