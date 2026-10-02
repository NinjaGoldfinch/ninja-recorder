# v2.0 verification pass: the checklist (#47)

The run sheet for WS7.2 on **2.0.0-alpha.182** (`df361f0`, the tip of `main`),
with WS7.1's measurements (#46) folded in. It is ordered so each block leaves
the machine ready for the next. Results go back into
`docs/windows-verification.md` and onto the issues afterwards, the same way
as last time. This file is not for merging.

**What it covers.** #47's scope (§1 to §5 and the §4 daemon cases) **on the own
backend**, which the earlier §2/§3 passes predate. It also re-runs every
landed fix that has never run on hardware: #293 (#317), #295 (#316), #296
(#331), #297/#313 (#319) and #298 (#318). Last come the leftover rows any box
can reach. Everything that already passed on a recent build is left out.

**Not on this box, so not here:** Windows 10 and below-19041 rows, a second GPU
vendor (#224), a lost GPU device, anything on the stable channel (no stable v2
exists yet), and protocol skew (every build still has `PROTOCOL = 1`).

**Every row:** Pass / Fail / Skipped, plus what was seen. **Paste log lines
exactly**: no rounding and no summarising. Where a row says *paste*, the line is
the result.

---

## C0. Before the session

- [ ] **C0.1** Builds, both from the same commit:
  - Release: `ninja-recorder_2.0.0-alpha.182_x64-setup.exe` from the
    v2.0.0-alpha.182 release page.
  - Devtools: artifact `ninja-recorder-devtools-windows-latest-df361f0…` from
    CI run 36699016646. **It expires on 2026-10-07**, so download it now.
- [ ] **C0.2** Record: Windows build, GPU and driver version, League patch,
  monitor resolution and League's window mode.
- [ ] **C0.3** No dev toolchain anywhere in the pass. Defender is live, with no
  exclusion for the app.
- [ ] **C0.4** A newer alpha is needed for C11. The next merge to `main` (#384,
  the docs PR) publishes alpha.183, so nothing needs building.

## C1. The measurement script (#59). Do this first

#46's numbers depend on it, and its success path has never run.

- [ ] **C1.1** With the app open (daemon + UI), run
  `.\scripts\measure.ps1 -Label 'filter test' -ArgumentFilter '--daemon' -Seconds 10`.
  It measures **one** process, and that pid is the one Task Manager's Details
  tab (with the Command line column) shows with `--daemon`.
- [ ] **C1.2** `-ArgumentFilter '--no-such-flag'` finds nothing, says so, and
  exits 1.

## C2. Install and launch (§1, §5.0)

- [ ] **C2.1** Install the release build, ideally on a clean-ish account (§1).
  Record the installer filename, the SmartScreen behaviour, and the version
  in Settings → About.
- [ ] **C2.2** Start it from the Start Menu. The window opens at **1340x850**,
  will not shrink below **960x640**, and the taskbar names it `ninja-recorder`.
  For the drawn title bar (#376): drag moves the window, double-click
  maximises, and minimise / maximise / close all work.
- [ ] **C2.3** Quit from the tray, then run
  `& "$env:LOCALAPPDATA\ninja-recorder\ninja-recorder.exe" C:\nothing.mp4`:
  it starts normally (§5.0, unknown arguments).
- [ ] **C2.4** With the app running:
  `$p = Start-Process "$env:LOCALAPPDATA\ninja-recorder\ninja-recorder.exe" -ArgumentList '--daemon' -PassThru -Wait; $p.ExitCode`
  prints **0**, and the first daemon's `daemon.log` has nothing new
  (§5.0.5).
- [ ] **C2.5** Settings → Advanced reads "Automatic: Own, the default.", and
  "In use now" reads `own (idle)` with no client open. If a stored backend
  from earlier passes shows instead, note it and pick Own.

## C3. Resources (#46, §5, §5.2): clean state, before any game

Use the release install throughout. Each run is 60 s at 1 Hz, and every row the
script prints is the result. Set `$inst = "$env:LOCALAPPDATA\ninja-recorder"`
first.

- [ ] **C3.1 Daemon only** (window closed, *no UI process*, no League client):
  `.\scripts\measure.ps1 -Label 'alpha.182 - daemon only' -ArgumentFilter '--daemon' -InstallPath $inst`.
  **This is the row gated against C3.** The install column is §5's size
  figure, against the 200 MB budget.
- [ ] **C3.2 Daemon + UI**: the same with `-Label 'alpha.182 - daemon + UI'`,
  then once more **without** `-ArgumentFilter`, for the two-process total.
- [ ] **C3.3 Daemon + League client** (window closed, client in the lobby, so
  the worker is warm): a `--daemon` row, a `--capture-worker` row, and a
  total with no filter.
- [ ] **C3.4 All three**: the total, no filter.
- [ ] **C3.5 Idle CPU** (§5, "~0%"), in C3.1's state:
  `.\scripts\measure.ps1 -Cpu -Role daemon -Label 'alpha.182 - idle'`.
- [ ] **C3.6** *(Optional, §11.8's open CPU rows.)* Own against libobs,
  alternated in one sitting with three runs each, by measurement.md §4.4–4.5:
  idle with the client open, and recording in Practice Tool at the fountain.

## C4. The full loop on own (§2, §3), and the recent UI

- [ ] **C4.1 Practice Tool, Game preset, with no UI process** (end the
  non-`--daemon` `ninja-recorder.exe` in Task Manager first). It starts
  recording by itself, and "Recording saved" arrives with the file name and
  marker count. *Paste* `daemon.log`'s
  `recording started: backend own (ready: …)` line and the `own: recording` /
  `own: stopped` / `own: remux` lines.
- [ ] **C4.2** Open the app. The card is in the library with its role badge
  (#377). It plays, the markers sit on their events, clicking a marker seeks,
  and **Space then plays** (#379). The review page fits the window (#375), and
  the title bar's client pill (#376) shows the client's state.
- [ ] **C4.3 A live queued game** (§3), full length. Vanguard raises no flag.
  The card fills in **role and queue on its own within a minute** of the end,
  with no refresh (§6, the deferred patch), and `daemon.log` has no
  winner-disagreement warning. **Write down anything that behaved differently
  from Practice Tool.** §3 has had an empty paragraph waiting for this since
  2026-09-17.
- [ ] **C4.4** On that file:
  `ffprobe -v error -show_entries stream=index,codec_type,start_time,duration <file>`.
  The video and every audio stream end within one frame (16.7 ms) of each
  other. *Paste* the output and the `[trim]` line.
- [ ] **C4.5** *(Devtools, optional, §6.)* Straight after that game, run
  `dev_lcu_get` on `/lol-end-of-game/v1/eog-stats-block` and save the
  response. It has never been seen, and the parser is modelled on the spec.

## C5. Resilience on own (§4), with #313, #318, #316 and #297

Game + mic preset on "Windows default", using the headset as both mic and
output. One Practice Tool game per row is fine, or several rows in one game.

- [ ] **C5.1** Alt-tab out for 10 s and back, twice. Minimise for 10 s. The
  file has no gap, and the minimised stretch holds the last frame.
- [ ] **C5.2** Change the resolution to another aspect and back, then to a
  smaller size with the same aspect. The first is pillarboxed in black; the
  second fills the frame.
- [ ] **C5.3 Reconnect (#313 on #319).** End `League of Legends.exe` and
  press Reconnect. The result is one file: black from the kill, then the game
  again. The game source's stop line should read **about 0 ppm, a few slips at
  most, `gaps` 1–2**, not thousands of slips. *Paste* the `own: stopped`
  line.
- [ ] **C5.4 Mic off for ~10 s, then on (#318).** `worker.log` has
  `the microphone audio device went away (…)` once, then
  `microphone came back (<device>); captured again after N s of silence`. On
  "Windows default", expect `came back (<webcam>)` first and then
  `microphone audio moved to the new default device: …`. The voice is back in
  sync. The card reads "Recorded without part of the microphone audio (the
  microphone was disconnected)", its reason reads
  `silent from m:ss to m:ss: … captured again when it came back`, and the strip
  does not ask for a report (§11.9, #331). *Paste* the lines.
- [ ] **C5.5 Mic left off** (another game). The stop line says
  `device lost 1 times (… s silent, the last never came back)`, the reason
  says `silent from m:ss on: … did not come back`, and the track layout is
  unchanged.
- [ ] **C5.6 Desktop preset (#316, #318).** At the first packet:
  `desktop audio clock qpc: the first packet's QPC stamp is real, ~7.5 ms after it was taken`,
  with no device-time warning. At stop: `clock qpc: raw drift … ppm` for the
  desktop. Then switch the headset off and on: `desktop audio moved to the new default device`,
  and the desktop track follows it back. *Paste* the lines.
- [ ] **C5.7 Game audio running short (#297 on #319).** Use Game + mic +
  Discord in a call, and switch the headset (the default output) off mid-game.
  Look for `jumps=N` with `raw` near 0 ppm on the summary line, the
  per-source `… unflagged jumps in the stamps (x ms)`, and a written residual
  worst within about 5 ms (it used to reach −26 ms). A clean recording's
  summary line has **no** `jumps=`. *Paste* both.

## C6. Capture failures in the app (§11.9 on #331): release, then devtools

- [ ] **C6.1 Discord closed**, Game + mic + Discord preset. There is no strip,
  no "without" toast and no line on the row. `worker.log` has
  `no Discord.exe audio` at **INFO**, and the start line says
  `Discord.exe=left out (…)`, not `failed`.
- [ ] **C6.2 Mic access off** (Settings → Privacy & security → Microphone →
  *Let desktop apps access your microphone*), Game + mic. The toast reads
  "Recording saved without microphone audio", says "Windows is blocking
  microphone access" and how to turn it back on, and shows **no report request
  and no HRESULT**. The strip says the same with Dismiss; the row says
  "Recorded without microphone audio", with the reason in its tooltip; the
  review page shows the full line. The log still names the call and
  `0x80070005`. *Paste* the toast text and the log line. Turn access back on.
- [ ] **C6.3 Worker killed mid-game** (End task on `--capture-worker`, a
  minute in). The toast reads "Recording ended early … the capture worker
  stopped unexpectedly at <time>" and does ask for a report. The pid and exit
  code are only in `daemon.log` and `diagnostics_json`, and the log says
  `exited with code 1` **once**. Within about a second a new worker is up and
  a second recording carries on with the same game (#300). Both play.
- [ ] **C6.4** Repeat C6.2 on the devtools build. The text is word for word the
  same.

## C7. Daemon and worker lifecycle (§4.1, §11.6, #293)

- [ ] **C7.1 Daemon killed mid-game** (Game + mic + Discord, a few minutes in,
  so there are markers). The strip appears and stays, then the UI starts a
  daemon and the strip clears. The log has
  `startup recovery: finished 1 interrupted recording(s)` and
  `remuxed recovered … 4 audio track(s)`. The recovered card has champion,
  KDA, CS, items, spells and runes, its markers and curve up to the kill, and
  a track menu with all four stems (#312), and it scrubs. **The second
  recording's markers are its own**, at the right times.
- [ ] **C7.2 A clean stop releases the worker (#317).** With the client open
  and no game, quit from the tray. `worker.log` has
  `capture worker exiting (Released)`. `daemon.log` has
  `own backend: capture worker pid N exited cleanly (code 0)` **before**
  `[daemon] stopped`, and no `did not release within`. *Paste* the lines.
- [ ] **C7.3 The client killed mid-game** (§11.6). With a Practice Tool game
  running, end `LeagueClient.exe` and `LeagueClientUx.exe`, leave the game up,
  then end the game. The recording plays, and the worker's `exited cleanly`
  line comes **after** the recording's finish line. *Paste* everything from
  the kill to the exit.

## C8. The forced software encoder (§11.7, §11.8 on #331): devtools

Quit the app from the tray, then in PowerShell:
`$env:NINJA_OWN_FORCE_SOFTWARE_ENCODER = "1"; & "$env:LOCALAPPDATA\ninja-recorder-dev\ninja-recorder-dev.exe" --daemon`.
Open the app from the Start Menu, and record a Practice Tool game on the Game
preset.

- [ ] **C8.1** `own backend: software H.264 encoding with …: forced by NINJA_OWN_FORCE_SOFTWARE_ENCODER (devtools)`
  is logged **once** per decision, not twice in the same millisecond.
  `nvidia-smi encodersessions` shows no session for the worker.
- [ ] **C8.2** Settings → Advanced's notice says
  "…because forced by NINJA_OWN_FORCE_SOFTWARE_ENCODER (devtools)", the same
  reason as "In use now", **not** "no usable hardware encoder".
- [ ] **C8.3** The recording plays and scrubs. Close that PowerShell and
  restart the app afterwards.

## C9. The backend switch (§9's open rows)

- [ ] **C9.1 Refused mid-game.** In a game on Own, click **libobs**. It is
  refused with "can't be changed while a recording is in progress", the
  recording carries on and finalizes on Own, and the row still shows Own.
- [ ] **C9.2 Switching counts processes.** Back in the lobby, switch to
  libobs: `daemon.log` gains `(capture_backend = libobs, changed in Settings)`,
  and Task Manager shows **one** `extprocess_recorder.exe` and **no**
  `--capture-worker`. The next game's `diagnostics_json.backend` (devtools
  portal → Library) names libobs. Switch back to Own: no
  `extprocess_recorder.exe` is left running.
- [ ] **C9.3 An unbuildable saved libobs.** Choose libobs, quit from the tray,
  and rename `libobs\extprocess_recorder.exe` in the install folder. Then
  start the app. The backend line reads
  `unavailable (the libobs worker is not beside the executable) (capture_backend = libobs)`,
  the row shows libobs disabled with "Nothing will be recorded", and a game
  produces **no** recording. Choosing Own then records again without a restart.
  Put the file name back afterwards.

## C10. Tray and notifications (§5.0.1, §5.0.3)

- [ ] **C10.1** Close the window the first time: the "still running in the
  tray" notice appears. Close it again: no notice. Settings → Notifications →
  Reset brings it back once.
- [ ] **C10.2** With "Recording started" enabled and no window open, starting
  a game shows it.
- [ ] **C10.3** Turn the master switch off: nothing is shown, the one-time
  notice included, and the other three checkboxes grey out.
- [ ] **C10.4** Clicking a toast does nothing: no crash and no stolen focus.
- [ ] **C10.5** With both builds installed, their toasts are named apart
  (`ninja-recorder` vs `ninja-recorder-dev`).
- [ ] **C10.6** End `explorer.exe` in Task Manager and start it again: the tray
  icon comes back.
- [ ] **C10.7** Tray → Settings opens on the Settings view, both with a window
  already open and with none.
- [ ] **C10.8** Search `daemon.log` for `WARN [notify]`. Any hit sits beside a
  recording that carried on anyway.

## C11. Updates and installing (§5.0.4, §10): once alpha.183 is published

- [ ] **C11.1**
  `curl -L https://github.com/NinjaGoldfinch/ninja-recorder/releases/download/alpha/alpha.json`
  names alpha.183 with a **tagged** asset URL. The stable `latest.json` is
  expected to 404.
- [ ] **C11.2** Settings → About → switch the channel to Stable and back: each
  switch re-checks at once, and the row describes the channel just chosen.
- [ ] **C11.3** Relaunch alpha.182. About 30 s later, a dot appears on the
  settings button and About names alpha.183. There is **no** toast and no
  dialog. "Check now" gives the same answer.
- [ ] **C11.4 Install with the client open** (§10). `daemon.log` has
  `[update] handing over to …` and no `could not release the capture backend`.
  The app comes back on alpha.183, and your settings and library survive.
- [ ] **C11.5 Uninstall before installing** (§10's last row). Run a newer
  installer by hand and keep "Uninstall before installing". `libobs\` is back
  and complete afterwards, and a game records. *(Needs an installer newer
  than what is installed. If alpha.183 is already in place, any later alpha
  will do.)*

## C12. Side by side and library leftovers (§7.8, §6)

- [ ] **C12.1** Release and devtools both running, one Practice Tool game. Each
  build's recording plays in its own review player, with markers (§7.8's open
  row).
- [ ] **C12.2** Drop three video files the app did not record into the
  recordings folder and press Rescan. Each card shows a real length, not the
  placeholder (§6, the duration probe).
- [ ] **C12.3** *(Optional.)* Spectate a game. Note whether the app records
  it; by design it should not.
- [ ] **C12.4** *(Optional.)* Sleep the machine with the client open, wake it,
  and play a game: it records.

## C13. Last: uninstall with autostart on (§5.0.2)

- [ ] **C13.1** Turn Start on login on, then uninstall. The `Run` value is
  left pointing at a removed exe, which is expected. Sign out and back in: **no
  error dialog**. Reinstall afterwards if the box is staying in use.

---

## Rows this pass closes, if they pass

| Issue | Closed by |
|---|---|
| #59 | C1 |
| #46 | C3.1 to C3.5 (the v1 baseline in #3 is still blocked; record v2's rows anyway) |
| #293 | C7.2 |
| #295 | C5.6 |
| #296 | C6.1 to C6.4, C8.1, C8.2 |
| #297, #313 | C5.7, C5.3 |
| #47 | the whole sheet, signed off |
