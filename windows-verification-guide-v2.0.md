# v2.0 Windows verification: step-by-step guide

Written 2026-09-23 against `main` @ `1ce6602` (v2.0.0-alpha.88).

This follows the section order of the #130 plan, which the checklist sheet
mirrors. The sheet itself is not readable from here, so row wording may differ
slightly; the section numbers match #130.

## Where things stand

As of the 2026-09-22 session, **26 of 28 rows pass** (`docs/windows-verification.md` §5.0.6).
Two rows are open:

| Open row | Section below |
|---|---|
| `daemon.log` holds the finalize, with no `WARN [notify]` lines | §1 |
| The row, markers, advantage graph, card and game id survive a killed daemon | §4 |

Since that session, five fixes have landed that **no one has run on hardware yet**.
All of them are in alpha.87 and alpha.88:

- #186: keep the advantage curve when the daemon is killed
- #191: keep the library card when the daemon is killed
- #192: the backfill skips a recording that is still in flight
- #194: keep the game identity when the daemon is killed
- #195: `--hidden` removed; old Run entries heal themselves (new §5 row)

So the minimum to finish is §1's log check, all of §4, and §5's `--hidden` row.
Everything else is a re-run, useful if you're signing off v2.0 as a whole (#47).

### Progress, 2026-09-23 (alpha.87)

| Section | Status |
|---|---|
| 1. No window open | Pass (the toast is confirmed; C17 is the log row) |
| 2. Tray | Pass, every row |
| 3. UI killed mid-game | Pass |
| 4. Daemon killed mid-game | Done: every row passes except "second recording's markers are its own" ❌ #199 |
| 5. Start on login | Pass, every step (1–11) |
| 6. Dev portal | Pass (every panel renders; Log's active file is `daemon.log`) |
| 7. Update | Pass: the in-app install to alpha.89, the app coming back by itself (first ever), one install entry, the dev build offering nothing. network off reports the failure in words |
| 8. Second account | Pass (access denied to the pipe) |

Issues filed from this pass:
- [#198](https://github.com/NinjaGoldfinch/ninja-recorder/issues/198): a recording started mid-game puts its own markers at 0:00 (Practice Tool only so far)
- [#199](https://github.com/NinjaGoldfinch/ninja-recorder/issues/199): after a daemon kill, the next recording inherits the killed recording's markers (the §4 fail)
- [#200](https://github.com/NinjaGoldfinch/ninja-recorder/issues/200): a recovered recording loses its items, spells and runes until the backfill runs (permanent for Practice Tool)
- [#201](https://github.com/NinjaGoldfinch/ninja-recorder/issues/201): the resume sweep re-patches Practice Tool rows on every client connection
- [#202](https://github.com/NinjaGoldfinch/ninja-recorder/issues/202): the release and devtools daemons write the same `daemon.log`
- [#203](https://github.com/NinjaGoldfinch/ninja-recorder/issues/203): Viego ending a custom game mid-possession is saved as the possessed champion

alpha.89 (docs-only, #197) is now the latest alpha, so the §7 update goes from alpha.87 to alpha.89.

---

## Part A: Which builds you need

| Build | What it's for | Where it comes from | Needs the repo? |
|---|---|---|---|
| **alpha.87** (release) | Every section except 6. Install it first. | GitHub Release `v2.0.0-alpha.87` | No |
| **alpha.88** (release) | §7, the update target | **Don't install it by hand.** The app updates itself to it. | No |
| **Devtools** build (`ninja-recorder-dev`) | §6 dev portal, plus the "offers no update" row in §7 | CI workflow artifact, **expires 2026-09-29 20:58 UTC** | No (GitHub sign-in needed) |
| Local source build | **Not used for this pass** | See A.5 | Yes |

**Why start on alpha.87 rather than alpha.88.** The only difference between them is
the update URL (#196, the repository rename). Everything under test is identical.
Starting on .87 gives you a real update to .88 at the end with no downgrading.
It also tests something extra: alpha.87 still polls the old `ninja-recorder-v2` URL,
so the update only works if GitHub's rename redirect works. I checked that
redirect on 2026-09-23 and it serves alpha.88's manifest.

### A.0 Prerequisites on the Windows box

1. Windows 10 or 11, signed in to the account you normally play on.
   **Don't use Administrator elevation for the app.**
2. League of Legends installed and patched. Vanguard active, and the machine
   rebooted since the last Vanguard update.
3. WebView2 runtime. It's built into Windows 11. On Windows 10, check
   *Apps & features → Microsoft Edge WebView2 Runtime*; if it's missing, the
   installer bootstraps it.
4. **Two local Windows accounts** for §8. To create a second one, use
   *Settings → Accounts → Other users → Add account → "I don't have this person's
   sign-in information" → "Add a user without a Microsoft account"*.
   Or, from an elevated PowerShell:
   ```powershell
   net user nrtest 'SomePassword1!' /add
   ```
5. **No dev toolchain in the loop**: no `cargo run`, no `npm run tauri dev`
   (verification doc §0).
6. **Leave Defender live, with no exclusion.** The 2026-09-22 pass showed the app
   does not trip it (#181). **Don't write the Run key from PowerShell and then
   launch the exe via WMI** (`Win32_Process::Create`). That combination looks
   like malware and got the binary quarantined once. Setting the value with
   `Set-ItemProperty` and then signing out is fine; §5 does exactly that.
7. Optional: in Task Manager's **Details** tab, right-click a column header
   → *Select columns* → tick **Command line**. That tells the daemon
   (`--daemon`) apart from the UI at a glance.

### A.1 Clean slate (do this before installing)

You want a "fresh install" without losing your library.

1. Quit the app: tray icon → **Quit**. Then confirm nothing is left:
   ```powershell
   Get-Process ninja-recorder, ninja-recorder-dev -ErrorAction SilentlyContinue
   ```
2. Uninstall any existing `ninja-recorder` and `ninja-recorder-dev` from
   *Settings → Apps → Installed apps*. **Leave "delete app data" unticked.**
   The library lives in `%APPDATA%\com.ninjarecorder.app\`.
3. Remove stale start-on-login values. The uninstaller doesn't touch them (by design):
   ```powershell
   $run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
   Remove-ItemProperty $run -Name 'ninja-recorder','ninja-recorder-dev' -ErrorAction SilentlyContinue
   Get-ItemProperty $run   # neither name should appear
   ```
4. Optional: move the old logs aside so this session's logs start empty.
   ```powershell
   $logs = "$env:APPDATA\com.ninjarecorder.app\logs"
   if (Test-Path $logs) { Rename-Item $logs "logs.before-2026-09-23" }
   ```

### A.2 Install alpha.87 (release build, no repo needed)

1. Open <https://github.com/NinjaGoldfinch/ninja-recorder/releases/tag/v2.0.0-alpha.87>.
2. Under **Assets**, download `ninja-recorder_2.0.0-alpha.87_x64-setup.exe`
   (about 65 MB). Ignore the `.sig` file and `alpha.json`.
   Or, with the GitHub CLI:
   ```powershell
   gh release download v2.0.0-alpha.87 -R NinjaGoldfinch/ninja-recorder -p '*x64-setup.exe'
   ```
3. Run it. **SmartScreen will warn**, because the build isn't code-signed (#183).
   Click *More info → Run anyway*. Note what the prompt said; the verification
   doc wants it recorded.
4. Go through the NSIS installer. It installs per-user, with no UAC prompt expected.
5. **Before opening the app**, run the §5 "fresh install registers nothing" check:
   ```powershell
   Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' |
     Select-Object -ExpandProperty 'ninja-recorder' -ErrorAction SilentlyContinue
   ```
   It must print nothing.
6. Launch from the **Start Menu shortcut**, not a terminal. Notifications depend on
   the shortcut's AUMID, so a terminal launch gives false failures.
7. *Settings → About*: confirm the version reads **2.0.0-alpha.87**.
8. *Settings → About → Update channel* → **Alpha**. It re-checks straight away and
   should offer alpha.88. **Don't install it yet**; that happens in §7.
9. Note the install path for later:
   ```powershell
   (Get-Process ninja-recorder | Select-Object -First 1).Path
   ```

### A.3 Install the devtools build (CI artifact, no repo needed)

Needed only for §6 and one §7 row. It installs **beside** the release build as
`ninja-recorder-dev`, with its own Start Menu entry, process name, pipe and
Run value. It reads and writes the **same library**.

**In a browser** (you need to be signed in to GitHub; artifacts aren't public):

1. Open <https://github.com/NinjaGoldfinch/ninja-recorder/actions/runs/35782262090>
   (the CI run for `1ce6602`, the alpha.88 commit).
2. Scroll to **Artifacts** and download
   `ninja-recorder-devtools-windows-latest-1ce660232b7ad59843fc362e33d645f51f2cd597`
   (about 98 MB zip).
3. Extract it. Inside is an NSIS installer, `ninja-recorder-dev_2.0.0-alpha.88_x64-setup.exe`
   or similar. It's larger because it's built uncompressed.

**With the GitHub CLI** instead:
```powershell
gh run download 35782262090 -R NinjaGoldfinch/ninja-recorder `
  -n ninja-recorder-devtools-windows-latest-1ce660232b7ad59843fc362e33d645f51f2cd597 -D .\nr-dev
```

Then:

4. Run the installer. You'll get the same SmartScreen prompt.
   **Quit the release app from its tray first** if the installer complains about
   a running app. It shouldn't, because the process names differ.
5. Launch **ninja-recorder-dev** from the Start Menu.

**If the artifact has expired (after 2026-09-29)**, you need a fresh CI run on `main`.
That requires write access to the repo:
```bash
gh workflow run ci.yml -R NinjaGoldfinch/ninja-recorder --ref main
```
Wait for the run to go green (roughly 20–40 min; the `build` job is the slow one),
then repeat from step 1 with the new run's ID from
`gh run list -R NinjaGoldfinch/ninja-recorder --workflow ci.yml --limit 1`.

### A.4 alpha.88: through the app, never by hand

This is §7. Leave alpha.87 installed until then.

### A.5 Building from the repo: don't, for this pass

The verification doc (§0) rules out a dev toolchain for this pass on purpose.
A local build also **can't record**. The libobs capture runtime and ffmpeg are
staged into `src-tauri/target/libobs/` by CI steps that aren't scripted for
local use (`docs/ci-and-releases.md` → Build). A `cargo run` or `npm run tauri:dev`
without that staging runs, but capture fails. If you need a build of some other
branch, run CI on it (`gh workflow run ci.yml --ref <branch>`) and install its
artifact as in A.3. The artifact without `devtools` in its name is the release flavour.

---

## Part B: Tools you'll use throughout

Paste these into a PowerShell window at the start of the session.

```powershell
# Show both processes with their role
function nr-ps {
  Get-CimInstance Win32_Process -Filter "Name like 'ninja-recorder%'" |
    Select-Object ProcessId, Name,
      @{n='Role';e={ if ($_.CommandLine -match '--daemon') {'DAEMON'} else {'UI'} }},
      CommandLine | Format-Table -AutoSize
}

# Kill only the daemon, or only the UI (a hard kill, the same as End task).
# These only touch the RELEASE build (ninja-recorder.exe). nr-ps shows both builds.
function nr-kill-daemon { Get-CimInstance Win32_Process -Filter "Name='ninja-recorder.exe'" |
  ? CommandLine -match '--daemon' | % { Stop-Process -Id $_.ProcessId -Force } }
function nr-kill-ui { Get-CimInstance Win32_Process -Filter "Name='ninja-recorder.exe'" |
  ? CommandLine -notmatch '--daemon' | % { Stop-Process -Id $_.ProcessId -Force } }

# Logs
$nrlogs = "$env:APPDATA\com.ninjarecorder.app\logs"
function nr-tail($f='daemon.log') { Get-Content "$nrlogs\$f" -Wait -Tail 30 }
function nr-warn-notify { Select-String -Path "$nrlogs\daemon.log" -Pattern 'WARN \[notify\]' }

# Start-on-login value
function nr-run { Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' |
  Select-Object 'ninja-recorder','ninja-recorder-dev' }

# Anything that crashed without logging
function nr-events { Get-WinEvent -LogName Application -MaxEvents 50 |
  ? { $_.Message -match 'ninja-recorder|extprocess_recorder' } | Format-List TimeCreated, Id, Message }
```

Use Practice Tool for anything that doesn't specifically need a real game. It
launches in about 30 s and lets you trigger kills and objectives.

---

## Part C: The checklist, section by section

Legend: ✅ **Pass (2026-09-23)** / ❌ **FAIL (2026-09-23)** = verified in this pass on
alpha.87 · ✅ alone = passed in an earlier session, not re-run this pass · ⬜ = **open,
must be done** · 🆕 = a fix has landed since the last run, so it's effectively new.

### 1. A game with no window open (#114, #124)

**Setup:** alpha.87 running. Settings → Notifications: master switch on and
"Recording saved" ticked. Also tick "Recording started" if you want to check it.

1. Open the app from the Start Menu once, so a daemon is running. Then close
   the window, and **also end the UI process** (a closed window leaves the UI
   process alive):
   ```powershell
   nr-kill-ui; nr-ps   # exactly one row, Role = DAEMON
   ```
2. Start a Practice Tool game. In another PowerShell window, run `nr-tail` and
   watch it start recording.
3. ✅ **Pass (2026-09-23)** The recording happens with no UI process: `nr-ps` still shows only the DAEMON row.
4. Play a couple of minutes, get a kill or two, then end the game.
5. ✅ **Pass (2026-09-23)** A **"Recording saved"** toast appears, with the file name and marker count.
6. ✅ **Pass (2026-09-23)** Open the app from the Start Menu. The new row is in the library
   (Ahri, Practice Tool, 1:15).
7. ✅ **Pass (2026-09-23)** **The log row** (sheet row C17). One daemon
   (running since 22:40:20 UTC) identified game `711143318` at 22:42:31 and saved it
   as recording 22 at 22:43:51 with no restart in between; zero `WARN [notify]` lines.
   Run:
   ```powershell
   Select-String -Path "$nrlogs\daemon.log" -Pattern 'libobs output|match-summary|recording \d+' | Select-Object -Last 15
   nr-warn-notify     # must print nothing
   ```
   A normal save writes **no** line containing "finalized" (only the shutdown and
   pre-update paths do). The evidence is the lines that follow a save:
   `match-summary: patched recording N from game G` and `wrote N gold points for
   recording N`. For Practice Tool the League client still identifies the game, and
   the row is usually completed by `resumed recording N from game G` instead.
   Pass: `daemon.log` shows this recording being saved, and there are
   **zero** `WARN [notify]` lines. That proves the toast came from the daemon's
   normal path rather than a fallback. Attach the relevant log lines to the sheet.

### 2. The tray (#116): ✅ **Pass (2026-09-23)**, every row

1. ✅ **Pass (2026-09-23)** Test **Open** and **Settings** in three window states, each via tray right-click:
   - window **closed**: a window appears (Settings lands on the Settings view)
   - window **open**: the existing window is focused, and `nr-ps` still shows one UI row
   - window **minimised**: it's restored
2. ✅ **Pass (2026-09-23)** Left-click the tray icon: the window opens and **no menu** appears.
3. ✅ **Pass (2026-09-23)** **Quit mid-recording** (needs a game running). The modal appeared; No kept
   recording; Yes logged `finalized an in-flight recording before exiting` then `stopped`,
   and the VOD plays:
   - Tray → Quit → a modal names the recording. Answer **No**: the recording
     continues and `nr-ps` still shows the daemon.
   - Tray → Quit → **Yes**: both processes exit. Relaunch; the VOD is in the
     library and plays.
4. ✅ **Pass (2026-09-23)** During a recording, right-click the tray about 10 times. The menu must open
   **instantly** every time. Any pause is a fail; note roughly how long.
5. ✅ **Pass (2026-09-23)** **Quit while idle**: no prompt, the icon disappears, and `nr-ps` prints nothing.

Observed: reopening the app while a game is still running starts a **second
recording** of the same game, so one game gives two rows. That's expected after a quit.

### 3. Killing the UI mid-game (#122): ✅ **Pass (2026-09-23)** (WS3.4 exit criterion)

The timer continued, the recording carried on, the 0:49 Viego VOD plays fully, and the
release daemon logged no restart during the game. Its second Viego row came from
quitting a devtools instance mid-game, not from the UI kill.

1. With the app open, start a game and wait for Recording in the header.
   Note the elapsed timer.
2. Run `nr-kill-ui` (or Task Manager → end the non-`--daemon` process).
3. The game keeps recording: `nr-ps` shows the daemon, and `daemon.log` has no stop line.
4. Relaunch from the Start Menu. Within one reconnect the header shows
   **Recording**, and the elapsed time **continues**. If it restarts from 0:00, that's a fail.
5. End the game. The VOD is complete and plays end to end.

### 4. Killing the daemon mid-game (§4.1): done 2026-09-23, one ❌ (#199)

This covers #150, #186, #191, #192 and #194. **You need two games in a row**: the
second one checks for inherited markers. A ranked game is not required. Practice
Tool works, but the **backfill row needs a game the LCU knows about** (match
history), so for that game use a real queue (Normal, ARAM or Ranked) if you can.
Practice Tool games may not appear in match history.

**Game 1: the kill**

1. App open (the window in view), alpha.87. Start a game, and wait for Recording
   plus at least **2–3 events** (kills, a dragon, a tower) and **2+ minutes**
   so the advantage graph has samples. Write down what happened and roughly when.
2. Kill the daemon: `nr-kill-daemon` (or Task Manager → end the `--daemon` process).
3. ✅ **Pass (2026-09-23)** A strip under the app bar says the recorder isn't running, and it **stays**
   rather than disappearing like a toast.
4. ✅ **Pass (2026-09-23)** The UI starts a new daemon by itself, and the strip clears. The log shows a
   new release daemon at 22:58:21, and `startup recovery: finished 1 interrupted recording(s)`.
5. ✅ **Pass (2026-09-23)** The row was hidden while the daemon was down. **The unfinished row isn't a library entry while the daemon is down.** The
   window restarts the daemon within seconds, so there's very little time to see
   this. If you want a clean look, repeat the kill with the UI already ended
   (`nr-kill-ui` first, then `nr-kill-daemon`). Nothing is running, so nothing
   recovers. Then relaunch and watch the library populate.
   Record which approach you used.
6. ✅ **Pass (2026-09-23)** **The row appears after the daemon restarts** (Viego, 2:30, 1/1/0, 10 cs,
   no outcome), with:
   - a duration read from the file
   - **the card filled in**: champion, KDA, queue and game mode as of the kill
   - outcome blank, because the game hadn't ended. `role` and `patch` are blank too (expected).
7. ✅ The partial file plays in the review player.
8. ✅ **Pass (2026-09-23)** **Markers up to the kill** (the kill cluster at game 1:02, and the deaths): open the recording. There are marker glyphs on the
   timeline, at times matching your notes from step 1. **This row failed on alpha.49.**
9. ✅ **Pass (2026-09-23)** **The advantage graph is drawn up to the kill.** Glyphs with a blank graph is a fail (#186).
   Check **CS diff** or **Kill diff**. Gold diff comes from the post-game match record, which
   Practice Tool never gets. The CS diff line is drawn to the end of the recovered 1:48 recording.
10. Let the game finish, or leave it. Note whether the restarted daemon began
    **another** recording of the same game; the doc doesn't say what to expect,
    so just record what happened.
    *Observed: yes, the new daemon starts a second recording of the same game.*
11. ✅ **Pass (2026-09-23)** (custom game, queue 3100) **Backfill completes the row** (#192, #194). The role,
    patch, outcome **and the scoreboard (items, spells)** filled in: after the game has ended,
    *Settings → **Fill in*** (in the storage group, beside the recordings folder). The report appears under the button.
    The killed recording's `role`, `patch` and **outcome** fill in. It carried the
    game id, so the match is found directly rather than by clock. Attach the report text.

**Game 2: nothing inherited**

12. ❌ **FAIL (2026-09-23)**: [#199](https://github.com/NinjaGoldfinch/ninja-recorder/issues/199). The
    post-restart recording listed every pre-kill event at video 0:00 (reproduced twice). In
    Practice Tool, its own markers were also at 0:00 ([#198](https://github.com/NinjaGoldfinch/ninja-recorder/issues/198)),
    which didn't reproduce in a custom game.
    Record a **second game to completion** normally. Its markers must be **its
    own**. Any glyph at a time that matches game 1's events, or at a time where
    nothing happened, is a regression. Screenshot both timelines.

**Version skew row (if it's on the sheet):** it can't be exercised right now.
Every available build has `PROTOCOL = 1` (`src-tauri/src/daemon/rpc.rs`), so no
UI/daemon pair actually disagrees. Mark it *not run: no builds with differing
protocols*, rather than pass.

### 5. Start on login (§5.0.2)

**Setup:** alpha.87 only. Do this with the devtools build **not running**.

1. ✅ **Pass (2026-09-23)** Fresh install registers nothing: this was step 5 in A.2. Record that
   result here. Also open Settings and find the **Start on login** row (in the background/tray
   group, next to the close-button options): the checkbox is **off**.
2. ✅ **Pass (2026-09-23)** Tick **Start on login**. `nr-run` shows the full path to `ninja-recorder.exe`
   followed by `--daemon`.
3. ✅ **Pass (2026-09-23)** Untick it: the value is gone. Tick it again.
4. ✅ **Pass (2026-09-23)** Quit from the tray, relaunch, reopen Settings: the checkbox is still ticked.
5. ✅ **Pass (2026-09-23)** **Sign out and back in.** One DAEMON row, and a count of 0. A Practice Tool game
   recorded with no window open. Then:
   ```powershell
   nr-ps   # exactly one row, DAEMON
   @(Get-CimInstance Win32_Process -Filter "Name='ninja-recorder.exe'" | ? CommandLine -notmatch '--daemon').Count   # 0
   ```
   The tray icon is present. **Don't** judge by counting `msedgewebview2.exe`,
   because other apps host it too. Start a Practice Tool game without opening the
   window and confirm it records.
6. ✅ **Pass (2026-09-23)** Open the app from the tray: two processes, still **one** tray icon, and the game is in the library.
7. ✅ **Pass (2026-09-23)** 🆕 **An old `--hidden` entry heals itself** (#195). The first login opened a window;
   the value was rewritten to `--daemon` (log: `[autostart] login entry rewritten with the current
   arguments`, 23:23:20 UTC); the next login gave one DAEMON process and no window:
   ```powershell
   $exe = (Get-Process ninja-recorder | Select-Object -First 1).Path
   Set-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' `
     -Name 'ninja-recorder' -Value "`"$exe`" --hidden"
   nr-run
   ```
   (Only set the value. **Don't launch the exe from PowerShell**; see A.0 point 6.)
   - Sign out and in. **A window opens.** That's expected once.
   - Without touching Settings, run `nr-run`. The value must now end in **`--daemon`**.
   - Sign out and in again. No window this time; `nr-ps` shows only the daemon.
   Pass means the window appeared **exactly once**.
8. ✅ **Pass (2026-09-23)** **The entry deleted from outside the app.** Settings reads off after the value was removed. With the app running, delete the
   value. Task Manager's Startup tab can only enable or disable, not delete, so use:
   ```powershell
   Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'ninja-recorder'
   ```
   Then reopen Settings: the checkbox must read **off**. Tick it again afterwards.
9. ✅ **Pass (2026-09-23)** Disabled in Windows' Startup apps, then the app closed and reopened:
   its checkbox read **off**. Re-ticking it in the app switched Windows' toggle back to **On**.
   **The doc's expectation ("still on") is wrong**, and the real behaviour is better:
   `auto-launch` 0.5.0 reads and writes
   `HKCU\...\Explorer\StartupApproved\Run` (`is_enabled` = Run value present **and** not
   disabled there; `enable()` resets it to enabled). `docs/windows-verification.md` §5.0.2 needs
   correcting.
   Tick it again, then in *Task Manager → Startup apps* **Disable** the entry.
   Settings is expected to still say **on**, because Windows stores "disabled"
   outside the Run key. Record what it says. Then untick and re-tick in Settings,
   and confirm Task Manager shows it **Enabled** again.
10. ✅ **Pass (2026-09-23)** After the reinstall, the value still reads `...\ninja-recorder.exe --daemon` and `Test-Path` is True.
    Reinstall alpha.87 over itself with autostart on (run the same setup.exe).
    `nr-run` still points at an exe that exists (`Test-Path` on the path).
11. ✅ **Pass (2026-09-23)** Two separate values (`...\ninja-recorder-dev\ninja-recorder-dev.exe --daemon` beside the
    release one); unticking in the dev build removed only `ninja-recorder-dev`. Side by side: install the devtools build (A.3), tick Start on login in
    **both**. `nr-run` shows two separate values, `ninja-recorder` and
    `ninja-recorder-dev`. Untick one; the other stays ticked.
12. Optional, destructive (do it last, or skip): uninstall with autostart on,
    then sign out and in. A stale value is expected, and **no error dialog**
    should appear.

### 6. The dev portal (#123): ✅ (WS3.7 exit criterion), needs the devtools build

1. Install the devtools build (A.3) and launch **ninja-recorder-dev**.
2. Click **Dev portal** in the main window. The button only exists in this build.
3. ✅ **Pass (2026-09-23)** Every panel renders; Seed, Simulate and Retention work; Database holds today's rows;
   Commands has no drift banner; Recorder's `is_recording` returns false; Log's active file is
   **`daemon.log`**. (Because of #202 the file name alone doesn't say which daemon; see the
   `listening on` check.)
   Work through every panel: Overview, Database, Simulate, Library, Retention,
   Log and so on. Each should describe the **daemon**. The decisive check:
   **Log panel → active file = `daemon.log`**, not `ui.log`.
4. Confirm its pipe is separate:
   ```powershell
   [System.IO.Directory]::GetFiles("\\.\pipe\") -match "ninja-recorder"
   ```
   With both builds running, you should see both a `...app.release` and a `...devtools` pipe.

### 7. An update, end to end (#129; the relaunch is fixed in #153)

**2026-09-23:** alpha.87 → **alpha.89** (docs-only, #197; it replaced .88 as the newest alpha) via the in-app **Install** button, through the old `ninja-recorder-v2` URL redirect.

**Do this last.** It replaces the binary. You'll be going from alpha.87 to alpha.88.

1. Check both manifests resolve:
   ```powershell
   (irm https://github.com/NinjaGoldfinch/ninja-recorder/releases/download/alpha/alpha.json).platforms.'windows-x86_64'.url
   ```
   It should point at `.../download/v2.0.0-alpha.88/...setup.exe` (a **tagged** path).
   The stable manifest (`releases/latest/download/latest.json`) currently
   **returns 404**, because there's no non-prerelease v2 release yet. That's
   expected until v2.0.0 is cut; record it as such.
2. On alpha.87 with channel = Alpha: about 30 s after launch there's a dot on
   the settings button, and *Settings → About* names **2.0.0-alpha.88**.
   No toast or dialog should appear. **Check now** gives the same answer.
3. ✅ **The gate**: start a game. During *Game starting…*, *Recording* and
   *Saving…*, **Install** is disabled with a reason. After the game ends it
   re-enables **without** reopening Settings.
4. ✅ **Pass (2026-09-23)** Outside a game, click **Install**. The app exits and the installer runs silently.
5. ✅ **Pass (2026-09-23)** **First pass ever.** **The app comes back by itself** (#145 / #153): the window reappeared on its own, and About reads 2.0.0-alpha.89, up to date. a **window** reappears, and
   a daemon starts behind it (`nr-ps` shows two rows). *Settings → About* reads
   **2.0.0-alpha.88** and offers nothing. **Never passed before**, so time how
   long it took. If nothing comes back within about a minute, that's a fail;
   capture `nr-events` and both logs.
6. ✅ **Pass (2026-09-23)** There's one entry in *Apps & features*, one Start Menu shortcut, and one
   install directory.
7. ✅ Settings survived (start on login, close action, audio preset, retention),
   and the library still plays.
8. ✅ **Pass (2026-09-23)** ("Updates are not available in this build.") **Devtools build offers nothing**: in ninja-recorder-dev,
   *Settings → About* reads **"not available in this build"**.
9. ✅ **Pass (2026-09-23)** With the network off, About read "Could not check for updates: error sending request for url
   (https://github.com/NinjaGoldfinch/ninja-recorder/releases/download/alpha/alpha.json)", and the app carried on working. Unplug the network (or turn Wi-Fi off) and click **Check now**. The failure is
   reported in words and the app carries on (recording still works).
10. ✅ Tamper check: passed in an earlier session, and needs a scratch release.
    Skip it unless you're re-doing it deliberately.

### 8. A second Windows account (#115): ✅ **Pass (2026-09-23)**. The second account's connect to the pipe was refused (access denied). The ACL lists exactly three Allow entries: `NT AUTHORITY\SYSTEM`, `BUILTIN\Administrators` and `SAMS-PC\NinjaGoldfinch`; there's no Everyone and no Authenticated Users.

1. As your main user, with the daemon running, confirm the ACL:
   ```powershell
   $p = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'ninja-recorder.com.ninjarecorder.app.release')
   $p.Connect(2000)
   $p.GetAccessControl().Access | Format-Table IdentityReference, AccessControlType
   $p.Dispose()
   ```
   Expect exactly three rows: you, `NT AUTHORITY\SYSTEM` and `BUILTIN\Administrators`.
   `Everyone` and `Authenticated Users` must not appear.
2. *Start → your avatar → Switch user*, and sign in as the second account
   (A.0 step 4). **Don't sign out** of the first account.
3. In the second account's PowerShell, run the first three lines above.
   `Connect` must throw **UnauthorizedAccessException / access denied**. A
   timeout means the pipe wasn't found, which is inconclusive; check that the
   first user's daemon is still running.

---

## Part D: Recording results

- For each row: pass, or **what happened instead**.
- For every fail, attach **both** `daemon.log` and `ui.log` from
  `%APPDATA%\com.ninjarecorder.app\logs\`. A symptom in one process usually has
  its cause in the other. Include `nr-events` output if anything exited silently.
- Note the build (`alpha.87` / `alpha.88` / devtools `1ce6602`) against every row.
- When done, update the sheet on #130, and the ticks and dated notes in
  `docs/windows-verification.md` (§4.1, §5.0.2, §5.0.3, §5.0.4, §5.0.6), plus the
  status in #197.
