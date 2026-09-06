# Frontend

Vanilla TypeScript, no framework, no build-time templating beyond Vite. The
markup lives in `index.html`; the modules under `src/` wire behaviour onto it.

The organising principle is **state ownership, not widgets**. Each module owns
exactly one piece of mutable state and is the only place that writes it.

---

## Module graph

```mermaid
flowchart TB
    MAIN["main.ts<br/><small>composition root — owns nothing</small>"]
    ROUTER["router.ts<br/><small>owns: which view is showing</small>"]
    THEME["theme.ts<br/><small>owns: html[data-theme]</small>"]
    PREFS["prefs.ts<br/><small>owns: the preference cache</small>"]
    STATUS["status.ts<br/><small>owns: the poll timer</small>"]
    LIB["library.ts<br/><small>owns: the row set + filters</small>"]
    REVIEW["review.ts<br/><small>owns: the player + timeline</small>"]
    SETTINGS["settings.ts<br/><small>owns: the settings form</small>"]
    TOAST["toast.ts<br/><small>owns: the transient message</small>"]
    BRIDGE["bridge.ts<br/><small>invoke + asset URLs</small>"]
    DOM["dom.ts<br/><small>el, escapeHtml, escapeAttr</small>"]
    FMT["format.ts<br/><small>pure formatters</small>"]
    TYPES["types.ts<br/><small>mirrors the Rust serde structs</small>"]

    MAIN --> ROUTER
    MAIN --> THEME
    MAIN --> PREFS
    MAIN --> STATUS
    MAIN --> LIB
    MAIN --> REVIEW
    MAIN --> SETTINGS
    MAIN --> TOAST
    STATUS --> LIB
    SETTINGS --> LIB
    SETTINGS --> THEME
    LIB --> REVIEW
    LIB --> BRIDGE
    REVIEW --> BRIDGE
    SETTINGS --> BRIDGE
    STATUS --> BRIDGE
    PREFS --> BRIDGE
    LIB --> FMT
    REVIEW --> FMT
    LIB --> DOM
    REVIEW --> DOM
    BRIDGE --> TYPES
    style MAIN fill:#ede7f6,stroke:#5e35b1
    style BRIDGE fill:#e3f2fd,stroke:#1565c0
```

`types.ts` sits apart deliberately: putting each shape beside its first
consumer would make `bridge` → `review` → `bridge` a cycle.

## Views

Three top-level sections in one document, toggled by `router.ts`. Before it
existed, each view flipped its own and its sibling's `hidden` attribute from
two files that knew nothing about each other.

```mermaid
stateDiagram-v2
    [*] --> library
    library --> review: click a VOD card
    review --> library: back
    library --> settings: settings button
    review --> settings: settings button
    settings --> library: close (always returns to library)
```

## Backend communication

Two directions, deliberately asymmetric.

```mermaid
flowchart LR
    subgraph FE["Frontend"]
        S["status.ts"]
        M["main.ts"]
        L["library.ts"]
    end
    subgraph BE["Rust"]
        CMD["Tauri commands"]
        SUP["Supervisor"]
    end
    S -->|"pull: lcu_status + game_state_status<br/>setTimeout chain, interval scales with state"| CMD
    L -->|"pull: list_recordings, get_disk_usage, …"| CMD
    SUP -->|"push: library-changed event"| M
    M --> L
```

**Pull for live state.** The header's summoner/phase/recording readout comes
from a `setTimeout` chain, not `setInterval` — `lcu_status` reads a lockfile
and makes two HTTPS round trips, and a slow tick under `setInterval` would
stack calls on top of each other. The interval scales with game state, and
stretches to 10 s while the window is hidden.

Because that delay is only chosen when the *next* timer is armed, a
`visibilitychange` listener re-polls immediately when the window comes back —
otherwise the header could show up to 10 s of stale state while the in-flight
timer ran out. The 60 s safety refresh is skipped entirely while hidden: it
rebuilds the whole grid with `innerHTML`, and `library-changed` already covers
real changes. Skipping it leaves its timestamp stale on purpose, so the first
poll after the window returns catches up at once.

**Push for the library.** `library-changed` is the one backend→frontend event:
the supervisor emits it after a finalize, and `set_retention_policy` after a
deletion. Polling `list_recordings` instead would rebuild the grid every few
seconds and fight scroll and focus.

### Command surface

The names and arguments below are the IPC contract and have not changed, but
how they reach Rust has. `bridge.ts` sends all but three of them through a
single `rpc` command — `invoke("rpc", { command, args })` — which `core`'s
dispatch table routes by name
([DEVELOPMENT.md §12](../DEVELOPMENT.md#12-process-model-a-recorder-daemon-and-a-ui-that-can-leave)).
Callers are unaffected: `call()` takes the same name and the same args object,
and forwards the args untouched.

The exceptions are in `DIRECT_COMMANDS` in `bridge.ts`:
`open_recordings_folder` and `dev_open_portal` drive the desktop shell, so they
stay in the UI process; `dev_registered_commands` must stay direct because
`devportal.ts` detects the portal's existence by watching that call *reject* in
a shipped build.

| Command | Returns | Used by |
|---|---|---|
| `list_recordings` | `Vec<RecordingRow>` | library grid |
| `rescan_recordings` | `ReconcileReport` | library toolbar → rescan |
| `get_recording_markers` | `Vec<MarkerRow>` | review timeline |
| `get_recording_samples` | `Vec<SampleRow>` | advantage curve |
| `get_disk_usage` | `DiskUsage` | library stats bar |
| `get_retention_policy` / `set_retention_policy` | policy / `EnforcementReport` | settings → storage |
| `preview_retention_policy` | dry-run deletion list | settings, while editing |
| `set_pinned` | — | library 📌 |
| `delete_recording` | — | library card |
| `get_recordings_dir` / `open_recordings_folder` | path / — | settings |
| `get_ui_prefs` / `set_ui_pref` | `HashMap<String,String>` / — | `prefs.ts` |
| `get_audio_preset` / `set_audio_preset` | `AudioPreset` / — | settings → audio |
| `list_audio_inputs` | `Vec<AudioInputDevice>` | settings → microphone picker |
| `extract_audio_track` | path to a cached sidecar | review player, stem selection |
| `lcu_status` | `LcuStatus` | header strip |
| `game_state_status` | `SupervisorStatus` | header strip, About block |
| `start_recording` / `stop_recording` / `is_recording` | — | registered but unreferenced by the main UI; the dev portal's Recorder panel drives them |

## Routing and the tray

`router.ts` owns which view is showing. `initRouting` adds two entry points the
tray needs: a `#settings` URL fragment read once at startup, for a window the
tray has just created, and a `navigate` event for a window that already exists.
A `#review` fragment is ignored — the review view with no recording loaded is
not a state worth restoring into.

## Theming

`data-theme` on `<html>` is written by JS and only ever holds `"light"` or
`"dark"` — there is no `prefers-color-scheme` query in the stylesheet.
Resolving the OS preference once, in one place, keeps a single dark block
instead of two and makes an explicit "Light" on a dark OS win by construction
rather than by CSS specificity.

The cost: "System" no longer follows the OS for free. `theme.ts` listens on
the matchMedia `change` event to put that back — **removing that listener is a
silent regression with no test to catch it.**

```mermaid
flowchart LR
    A["settings_kv (SQLite)<br/><small>source of truth</small>"] --> B["prefs.ts cache"]
    A -.mirror.-> C["localStorage"]
    C --> D["inline boot script in index.html<br/><small>picks a theme synchronously,<br/>before first paint</small>"]
    B --> E["theme.ts → html[data-theme]"]
    F["matchMedia change"] --> E
    D --> E
```

`localStorage` exists for exactly one reason: the boot script has to choose a
theme before first paint and IPC resolves too late. SQLite stays the source of
truth and wins any disagreement.

## Review player

- A plain `<video>` element. H.264/AAC MP4 decodes natively in the webview, so
  seeking, playback rate and frame-stepping come for free.
- Video loads through Tauri's asset protocol (`convertFileSrc`), scoped in
  `tauri.conf.json` to `$APPDATA/recordings/*` and
  `$APPDATA/recordings/audio-tracks/*` — this needs the `protocol-asset` Cargo
  feature, not just the config entry. The second entry is not redundant:
  Tauri's scope matcher won't let `*` cross a `/`.
- Frame-step is a ±1/30 s nudge, not true frame-accurate seeking: no
  per-recording frame rate is probed anywhere. Good enough for review, not for
  precision editing.
- Markers closer together than the timeline can resolve (common around a
  teamfight) collapse into one cluster glyph; `MARKER_PRIORITY` decides which
  icon the cluster shows.
- **Audio stems.** Track 0 is the combined mix and plays from the `<video>`
  itself, so most recordings need nothing here and the picker stays hidden
  (fewer than two tracks, or an unknown layout). Selecting any other track
  calls `extract_audio_track`, then plays the returned sidecar through a
  hidden `<audio>` synced against the muted video — WebView2 offers no way to
  switch tracks within one element
  ([DEVELOPMENT.md §2.5](../DEVELOPMENT.md#25-multi-track-audio)).
- Because of that, **volume and mute are held as state, not read off the video
  element** (`userVolume` / `userMuted` → `applyAudioOutput`). The video is
  muted whenever a stem is playing, and controls that read `video.muted` would
  render a muted player over audible sound. The `volumechange` listener was
  removed for the same reason — it would re-enter on the programmatic mute.

- **Playback stops while the window is hidden.** An open VOD otherwise keeps
  decoding video and playing its stem `<audio>` behind a minimised window,
  which is the largest thing the app can burn while it is out of the way.
  A `visibilitychange` listener pauses it and resumes only what it paused
  (`pausedByHide`), so a video the user had already paused stays paused.
  Pausing cascades through the existing `play`/`pause` handlers, so the rAF
  playhead loop stops with it — and `resumeStem` hard-resyncs the stem on the
  way back, so it cannot return drifted.

The library grid's filters, sort and stats bar all operate client-side over
the already-fetched row set. That is fine at solo-user library sizes and would
need real pagination if that stops being true.

## Escaping

`reconcile` imports any video file the user drops into the recordings folder,
so a displayed recording name is **not necessarily ours**. `escapeHtml` is for
text nodes and does not handle quotes; `escapeAttr` is the one for attribute
values. Using the wrong one is an injection bug with a plausible trigger.
