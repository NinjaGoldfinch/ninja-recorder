# p0c-audio — running P0c stage 1 (#7)

The procedure for WS1.3's exit criterion: **game audio present, Discord absent,
root PID documented.** Why the spike exists, and what its result is allowed to
decide, is [DEVELOPMENT.md §16](../../DEVELOPMENT.md#16-the-capture-gate-and-what-it-is-allowed-to-decide);
this file is only how to run it and what to bring back.

## Before running it

- **Q1a (#67) is the question #7 says to answer first, and it is closed.** Its
  premise was false: libobs calls the same Windows API, so the result cannot
  trade the licence against the feature either way (§16).
- **#66 (which licence at v2.1) is still open.** #78's body says it must be
  answered before the spike runs; #7 names Q1a instead, and #66 itself says it
  blocks nothing before WS8. Decide which of those stands **before** treating a
  run as the gate result, so the result is not litigated afterwards. Running it
  as a rehearsal is fine either way.
- **The cheaper check comes first.** `windows-verification.md` §6's first
  multi-track row asks the same question of the shipping build: a Game-preset
  recording with real samples in its game track says process loopback works
  against League on this machine. Doing that first makes this run confirm a
  known answer.

## What you need

- The Windows box, with rustup and the MSVC build tools (the app build already
  needs both). `spikes/rust-toolchain.toml` pins the compiler; rustup fetches
  it on first use.
- **League in a game.** Practice Tool is fine. `League of Legends.exe` exists
  only from loading screen to end of game; at the client there is nothing to
  capture. Game sound on, not muted in the Volume Mixer.
- **Discord open and audible through the same speakers or headset**, for the
  whole run. Any of: a voice channel with someone talking, a Soundboard sound
  played repeatedly, or a stream being watched in a voice channel. What matters
  is that Discord's own process is playing it.
- Nothing else making noise, if you can help it (browser tabs, music). If
  something is, write down what.
- **Not elevated.** Run from an ordinary PowerShell, because the daemon that
  will do this for real is not elevated. The spike prints whether it was.

## The runs

From the repository root, in an ordinary PowerShell:

```powershell
cd spikes\p0c-audio
cargo build --release
$spike = ".\target\release\p0c-audio.exe"

# 1. The report: Windows build, elevation, and the process tree. No capture.
& $spike --list 2>&1 | Tee-Object list.txt

# 2. The #7 run: the game's process tree, and nothing else. 60 s.
#    Keep the game making noise (walk, cast, attack a dummy) and Discord playing.
& $spike 2>&1 | Tee-Object include.txt

# 3. The control: everything EXCEPT the game's tree. 60 s, same conditions.
& $spike --mode exclude 2>&1 | Tee-Object exclude.txt
```

Each capture writes `p0c-audio-<mode>-<pid>.wav` into the current
directory, and prints one line per second while it runs. `2>&1` keeps an
error in the saved file, which is where it is most useful.

If run 1 says no `League of Legends.exe` is running, or that more than one is,
or warns that the game window belongs to a different PID, that is itself a
finding about the root: write it down, then pass `--pid <n>` explicitly with the
PID the report names as the window's owner.

## Reading the output

**The report (run 1)** answers "root PID documented". The design document's
open question is which process the tree is rooted at: the game
(`League of Legends.exe`) or the client the LCU tracks (`LeagueClient*.exe`).
Check:

- `root` is `League of Legends.exe`, and `resolved by` says the game window is
  owned by the same PID.
- `ancestors` shows what launched the game. If `LeagueClient.exe` is listed
  there, the game is inside the client's tree; if not, targeting the client
  could never have worked.
- `of interest` lists every Discord process as **outside the tree**. If any is
  inside it, include mode would capture Discord by design and the run cannot
  show isolation.

**The per-second table** says whether anything arrived: `peak dBFS` above -60
is signal, `-inf` is digital silence, `(no packets)` means the engine sent
nothing that second. The `== result ==` block closes with one of:

| Verdict | Means |
|---|---|
| `AUDIO CAPTURED` | Something was captured. Only listening says what. |
| `SILENCE CAPTURED` | The stream exists and the target's audio is not reaching it. In include mode, the wrong root produces exactly this. |
| `NOTHING CAPTURED` | No frames at all. Check the game was audible. |
| `activation was refused` | E_ACCESSDENIED is a finding, not a bug: Windows would not let this process capture that target. |

**Then listen to both WAVs.** They are 32-bit float, 48 kHz stereo; Audacity,
VLC and Windows Media Player all open them, and Audacity also shows the
waveform. The numbers cannot tell game from Discord.

| include WAV | exclude WAV | Reading |
|---|---|---|
| game, no Discord | Discord, no game | **Pass.** Isolation works against a Vanguard-protected game. |
| game **and** Discord | anything | **Fail.** Not isolating, unless the report put Discord inside the tree. |
| silent or nothing | game and Discord | **Fail.** Wrong root, or Vanguard in the way. Check the root before concluding which. |
| game, no Discord | **no Discord either** | **Inconclusive.** Discord was not audible; fix that and run again. |

The exclude run is what makes "Discord absent" a finding rather than an
assumption: it proves Discord was playing during the same session.

## What to bring back

Paste into #7:

1. `list.txt`, `include.txt` and `exclude.txt` in full.
2. The listening verdict for each WAV: game yes/no, Discord yes/no.
3. What Discord was playing, what else was making noise, and the League patch.

**Do not attach the WAVs to the issue.** The exclude file holds whatever
Discord was playing, which may be other people's voices, and the repository is
public.

The two P0c-1 rows in DEVELOPMENT.md §16's measurement table, plus the control
row, are where the result is written down; that is WS1.5 (#9). Leave them
empty until a run has produced them.

## Windows 10 floor test (#237)

**The question:** does process loopback work on Windows 10 22H2, build 19045?
Microsoft documents it from build 20348, which no Windows 10 release reaches;
OBS registers the same capture from 19041 ("process filtering seems to work
earlier"). **The own backend's floor is already 19041**
(`recorder::own::select::MIN_BUILD`): #237's plan was to lower it only if this
run passed, and the owner has since lowered it without the run (#291),
leaving a bug report to confirm it. [DEVELOPMENT.md §2.4](../../DEVELOPMENT.md#24-encoding-defaults)
has the decision.

**So this is optional confirmation, not a gate.** It is still the quickest way
to know rather than wait for a report, and a result either way is worth
pasting into #237. A fail would not stop Windows 10 recording: the own backend
records without game audio and says so in the app, naming the failing call.

**This spike has no build gate of its own.** It reads the build and prints it,
warns below 19041, and tries the capture anyway, so it runs unchanged on
Windows 10. That is the point: a refusal has to come from Windows, not from us.

### What you need

- A **Windows 10 22H2** machine: `winver` says *Version 22H2 (OS Build
  19045.xxxx)*. Not Windows 11, and not an older Windows 10.
- Everything in [What you need](#what-you-need) above: rustup and the MSVC
  build tools, League **in a Practice Tool game** with sound on, **Discord open
  and audible** for the whole run, nothing else making noise, **not elevated**.

### The runs

The same three runs as #7, saved under their own names:

```powershell
cd spikes\p0c-audio
cargo build --release
$spike = ".\target\release\p0c-audio.exe"

& $spike --list 2>&1 | Tee-Object list-19045.txt             # the report
& $spike 2>&1 | Tee-Object include-19045.txt                 # the game's tree only, 60 s
& $spike --mode exclude 2>&1 | Tee-Object exclude-19045.txt  # everything but the game, 60 s
```

Check the first line of `list-19045.txt` reads `windows 10.0 build 19045`,
and that `root` is `League of Legends.exe` owned by the game window, as
[Reading the output](#reading-the-output) describes. Then listen to both WAVs.

### What passing means

**Exactly the #7 result, on 19045:**

- **include**: `AUDIO CAPTURED`, and listening, **the game and nothing else**;
- **exclude**: `AUDIO CAPTURED`, and listening, **Discord and no game**.

Anything else fails the floor test, and the output says which way:

| What you see | Means |
|---|---|
| `ActivateAudioInterfaceAsync failed`, `activation was refused`, or `the activation never completed` | This Windows does not offer process loopback to this process. The floor should go back up. |
| `IAudioClient::Initialize failed` | The activation exists but the stream cannot be set up. The floor should go back up. |
| include has Discord in it | No isolation on this build (check the report did not put Discord inside the tree). |
| include is `SILENCE CAPTURED` with the right root | The stream exists and the game's audio does not reach it. |
| exclude has no Discord | Inconclusive: Discord was not audible. Fix that and run again. |

### What to paste into #237

1. `list-19045.txt`, `include-19045.txt` and `exclude-19045.txt` in full.
2. The listening verdict for each WAV: game yes/no, Discord yes/no.
3. `winver`'s full build (19045.xxxx), the League patch, and what Discord was
   playing.

As for #7, **do not attach the WAVs**.

A pass confirms the floor as it stands: cite the run in the §2.4 bullet and
in `MIN_BUILD`'s comment, which say it is untested on Windows 10 until then.
A fail means the floor goes back up to 20348 (`the_os_floor_is_build_19041`
and the §2.4 bullet), and what Windows 10 gets instead becomes its own issue.

### The own backend itself, on Windows 10

The spike proves the API. On 19045 the own backend no longer needs any
override: select it in Settings and follow §11.2. Below the floor (Windows 10
1903 or 1909, builds 18362 and 18363), a **devtools** build can still be told
to ignore it: `NINJA_OWN_IGNORE_OS_FLOOR=1` in the daemon's environment. A
release build never reads it, and a devtools build logs a warning every time
it lifts the refusal and again at every recording start.

```powershell
# Quit the app from the tray first, so no daemon is left running. The daemon
# is the process that reads the variable, and one started at login or by the
# window does not have it.
$env:NINJA_OWN_IGNORE_OS_FLOOR = "1"
& "$env:LOCALAPPDATA\ninja-recorder-dev\ninja-recorder-dev.exe" --daemon
```

Then open the app from the Start Menu as usual (it connects to that daemon),
set Settings → Advanced → Capture backend to **Own** and the audio preset to
**Game**, and follow
[windows-verification.md §11.2](../../docs/windows-verification.md#112-game-audio-by-process-loopback-237).
`daemon.log` shows `NINJA_OWN_IGNORE_OS_FLOOR=1: offering the own backend
BELOW ITS OS FLOOR` when the row is offered, and `RECORDING BELOW THE OS FLOOR`
at each start. Paste those with the §11.2 lines into #237. Unset the variable
(close that PowerShell) and restart the app when you are done.
