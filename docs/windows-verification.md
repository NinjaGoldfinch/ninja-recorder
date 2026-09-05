# Windows verification checklist

The capture backend compiles, bundles and records, but has **never run
against a real Vanguard-protected game**. This is the checklist that closes
that gap. It must be run on real Windows hardware with a real League client
and Vanguard active — nothing here is executable from macOS
([DEVELOPMENT.md §1.1, §9](../DEVELOPMENT.md#11-riot-vanguard-the-constraint-that-shapes-everything)).

Fill in the results inline as each step is done; this file is the record of
the run, not just the plan. When everything below passes, update
[DEVELOPMENT.md §2.2 and §3.4](../DEVELOPMENT.md#22-the-recorder-trait)'s
"not verified" notes and the status paragraph in the
[README](../README.md).

---

## What this catches that the dev loop can't

```mermaid
flowchart LR
    A["macOS dev loop<br/><small>stub recorder,<br/>fixtures, dev portal</small>"] --> B["Everything above<br/>the Recorder trait"]
    C["Windows cargo run"] --> D["Capture code paths,<br/>encoder selection"]
    E["This checklist<br/><small>installed build, real game</small>"] --> F["Vanguard tolerance<br/>Resource targets<br/>Installer + resource resolution<br/>Real gameflow timing"]
    style E fill:#ede7f6,stroke:#5e35b1
    style F fill:#ede7f6,stroke:#5e35b1
```

## 0. Prerequisites

- [ ] Latest `main` has a green CI run on `windows-latest`
- [ ] League of Legends installed and up to date on the Windows box
- [ ] **No dev toolchain involved in the app under test** — no `cargo run`, no
      `npm run tauri dev` for this pass. The dev loop already covers
      everything short of a real installer and real Vanguard; this pass exists
      specifically to catch what that loop cannot.

## 1. Install from a CI artifact

- [ ] Download `ninja-recorder-windows-latest-<sha>` from the latest `main` CI
      run — or take the installer off that run's Release instead, if
      this pass is also meant to validate what a release actually ships
- [ ] Run the NSIS installer on a clean-ish user account (not the profile used
      for `cargo run` testing, if avoidable — the goal is to catch anything
      leftover dev state hides)
- [ ] Launch the installed app from the Start Menu / desktop shortcut, not
      from a terminal

Record: installer filename, version, SmartScreen prompt behaviour (expected —
unsigned build).

## 2. Full loop: client detected → recording → markers → VOD in library

Use Practice Tool first: 30-second launch, on-demand kills and objectives.
Never iterate against real queued games.

- [ ] League client launch is detected (lockfile discovery)
- [ ] Gameflow phase transitions drive the state machine into `Recording` when
      a Practice Tool game starts
- [ ] Recording file appears and grows during the game
- [ ] Markers are captured (kills, objectives) and time-aligned
- [ ] On game end the VOD and its markers land in the library — SQLite row,
      visible in the review UI
- [ ] Playback works in the review UI and markers seek correctly

Record: any step that didn't fire, or fired late or wrong.

## 3. Vanguard-protected game

- [ ] The Practice Tool run above completed with Vanguard active and no flags
      or warnings from Vanguard or Riot
- [ ] Repeat the full loop once during a **live queued game**, not just
      Practice Tool — this confirms behaviour under real matchmaking timing
      (champ select, dodges) per the documented state machine edge cases in
      [recording-pipeline.md](recording-pipeline.md#2-the-state-machine)

Record: queue type, and anything that differed from Practice Tool.

## 4. Capture resilience

For each, confirm the recording continues or recovers cleanly and the final
VOD is playable:

- [ ] Alt-tab out of League and back mid-game
- [ ] In-game resolution change mid-recording
- [ ] Mid-game reconnect — disconnect the client (brief network drop or manual
      client kill), then reconnect; exercises the `Reconnect` path

Record: pass/fail per case, and what the output VOD looked like for any
failure (gap, corruption, truncation).

## 5. Resource measurements

First real measurement against the targets in
[DEVELOPMENT.md §1.2](../DEVELOPMENT.md#12-lightweight-is-a-tracked-requirement).

| Metric | Target | Measured | How |
|---|---|---|---|
| Installed size | ≤ 200 MB | | `Get-ChildItem -Recurse \| Measure-Object -Property Length -Sum` on the install folder |
| Idle RAM | ≤ 100 MB | | Task Manager / `Get-Process` working set, app idle, no League running |
| Recording overhead | Hardware encoder only, no x264 | | Confirm encoder choice in app logs during §2/§3 |
| Idle CPU | ~0% | | Task Manager, app idle with the client closed |

## 6. Open questions specific to the capture backend

These are the things nobody has been able to answer by reading the code:

- [ ] Does `window_capture` forced to WGC (`method=2`) actually produce frames
      for League's borderless and windowed modes?
- [ ] Does the faststart remux on stop actually run against a real capture?
- [ ] Does the bundled resource path (`target/libobs` → next to the installed
      `.exe`) resolve correctly in an installed build, and does dev mode need
      the staging step to also copy into `target/debug/libobs`?
- [ ] Are encoder priority, the window-size retry timing and the
      `AudioSource::SYSTEM` choice sensible against real hardware? They are
      first-cut defaults, not tuned.
- [ ] Does gameflow report a distinct phase while spectating? If it reports
      `InProgress`, spectated games are currently recorded, which the design
      says they should not be.
- [ ] Does the app recover state after machine sleep/wake mid-session?

## Outcome

- [ ] All boxes above checked
- [ ] Any failures filed as follow-up issues, linked here
- [ ] "Not verified" notes in DEVELOPMENT.md §2.2 / §3.4 and the README status
      paragraph updated to reflect what is now actually verified
