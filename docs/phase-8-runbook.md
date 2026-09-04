# Phase 8 runbook: integration test on real hardware

Tracks [issue #8](https://github.com/NinjaGoldfinch/ninja-recorder/issues/8). Must be run on the
Windows box with a real League client and Vanguard active — nothing here is executable from macOS
(DEVELOPMENT.md §1.1, §9). Fill in the results inline as each step is done; this file is the record
of the run, not just the plan.

## 0. Prerequisites

- [ ] Latest `main` has a green CI run on `windows-latest` (check the Actions tab)
- [ ] League of Legends installed and up to date on the Windows box
- [ ] Nothing from the dev toolchain used for the app under test itself — no `cargo run` /
      `npm run tauri dev` for this pass. The dev loop already covers everything short of a real
      installer + real Vanguard; this pass exists specifically to catch what that loop can't.

## 1. Install from CI artifact, no dev tooling involved

- [ ] Download the `ninja-recorder-windows-latest-<sha>` artifact from the latest `main` CI run
      (or build a `v*.*.*` tag and grab the draft Release installer instead, if this pass is also
      meant to validate `release.yml`)
- [ ] Run the NSIS installer on a clean-ish user account (not the same profile used for `cargo run`
      testing, if avoidable — the goal is to catch anything the dev loop's leftover state hides)
- [ ] Launch the installed app directly (Start Menu / desktop shortcut) — not from a terminal

Record: installer filename, version, any SmartScreen prompt behavior (expected, unsigned build).

## 2. Full loop: client detected → game starts → recording → markers → VOD in library

Use Practice Tool first (fast iteration, on-demand kills/objectives — DEVELOPMENT.md §3.3).

- [ ] League client launch is detected (lockfile discovery)
- [ ] Gameflow phase transitions drive the state machine into `Recording` when a Practice Tool
      game starts
- [ ] Recording file appears and grows during the game
- [ ] Markers are captured (kills/objectives) and time-aligned
- [ ] On game end, the VOD + markers land in the library (SQLite row, visible in the review UI)
- [ ] Playback in the review UI works, markers seek correctly

Record: any step that didn't fire, or fired late/wrong.

## 3. Vanguard-protected game verification

- [ ] Practice Tool run above completed with Vanguard active and no flags/warnings from Vanguard
      or Riot
- [ ] Repeat the full loop (§2) once during a **live queued game** (not just Practice Tool) —
      confirms behavior holds under real matchmaking timing (champ select, dodges, etc. per the
      state machine's documented edge cases in DEVELOPMENT.md §3.4)

Record: which queue type, whether anything differed from Practice Tool behavior.

## 4. Capture resilience

For each, confirm the recording continues (or recovers cleanly) and the final VOD is playable:

- [ ] Alt-tab out of League and back mid-game
- [ ] In-game resolution change mid-recording
- [ ] Mid-game reconnect (disconnect the client, e.g. brief network drop or manual client kill,
      then reconnect — exercises the state machine's `Reconnect` path from DEVELOPMENT.md §3.4)

Record: pass/fail per case, and what the output VOD looked like for any failure (gap, corruption,
truncation).

## 5. Resource measurements vs targets (DEVELOPMENT.md §1.2)

| Metric | Target | Measured | Tool |
|---|---|---|---|
| Installed size | ≤ 200 MB | | Installed folder size (`Get-ChildItem -Recurse \| Measure-Object -Property Length -Sum`) |
| Idle RAM | ≤ 100 MB | | Task Manager / `Get-Process` working set, app idle with no League running |
| Recording overhead | Hardware encoder only, no x264 | | Confirm encoder choice via app logs during §2/§3 (NVENC/AMD/QSV — DEVELOPMENT.md §2.4) |
| Idle CPU | ~0% | | Task Manager, app idle with League client closed |

Record actual numbers in the table above once measured — this is the first real measurement
against these targets (DEVELOPMENT.md §1.2 says "revisit once measured").

## Outcome

- [ ] All boxes above checked
- [ ] Any failures filed as follow-up issues, linked here
- [ ] Update DEVELOPMENT.md §3.4 / §9's "verification pending Phase 8" notes to reflect what's now
      actually verified
- [ ] Close issue #8
