# p0c-video — running P0c stage 2 (#8)

The procedure for WS1.4's exit criterion: **drift under one frame over a
ten-minute sample, a file killed at minute five is playable, and encoder
detection.** Why the spike exists, and what its result is allowed to decide, is
[DEVELOPMENT.md §16](../../DEVELOPMENT.md#16-the-capture-gate-and-what-it-is-allowed-to-decide);
this file is only how to run it and what to bring back.

**It has never been built on Windows or run.** It type-checks and passes
clippy for `x86_64-pc-windows-msvc` from Linux, which does not link, and its
timing, box-walking and ffmpeg-parsing code is unit-tested on any host
(`cargo test`). A compile, link or runtime error on the box is a finding: paste
it into #8 as it is.

## What one run measures

| §16 row | Where it comes from |
|---|---|
| WGC frames reach a fragmented MP4 | The clean run's file check: `moov` with `mvex`, complete `moof`+`mdat` fragments, and ffmpeg decoding real frames out of the video track. |
| Worst drift over ten minutes, in frames | The clean run. Video is written on a 60 fps grid kept by the performance counter; audio comes from the default output in loopback, counted by the sound card's own clock. Three numbers: the **raw** device drift (what an uncorrected pipeline would carry), the drift **written** after the spike slips samples to keep audio on the video's clock, and the **file**'s audio end minus video end as ffmpeg decodes it. |
| A file killed at minute five is playable | The `--kill-after 300` run: the capture is a child process that calls `TerminateProcess` on itself at 300 s, which is what Task Manager's End task does. The parent then checks the file: boxes, a full decode, and the app's own faststart remux. |
| Encoder selected, and what was offered | Every run: the H.264 encoders Media Foundation offers, the one the sink writer actually loaded, and whether its PCI vendor matches the GPU. |
| Software-only encode (#68's second arm) | The `--encoder software` run. |

**Only one hardware vendor can be checked here.** #68 settled that the box has
NVIDIA and nothing else, so "vendor detection on two GPUs" is met, at most, as
NVENC plus software. AMF and Quick Sync stay unrun; say so in #8 rather than
leaving it implied.

## Before running it

- **Stage 1 (#7) comes first in the plan, but this does not depend on it.** It
  captures the whole default output, not one process, so it runs whatever
  `p0c-audio` found.
- **Quit ninja-recorder** (tray icon, Quit), so its daemon is not recording the
  same game with libobs while this runs. Two captures and two encoders on one
  GPU would make every timing number ambiguous. Keep it *installed*: the spike
  decodes with its bundled `ffmpeg.exe`.

## What you need

- The Windows box, with rustup and the MSVC build tools (the app build already
  needs both). `spikes/rust-toolchain.toml` pins the compiler.
- **ffmpeg.** Found in this order: `--ffmpeg <path>`, then the installed app's
  copy at `%LOCALAPPDATA%\ninja-recorder\libobs\ffmpeg.exe`, then `ffmpeg` on
  `PATH`. The report prints which one it used. Without any of them the file
  check still reads the MP4's structure but cannot say whether it decodes.
- **League in a game for about 25 minutes.** Practice Tool is fine. The game
  window (`RiotWindowClass`) exists only from loading screen to end of game,
  and it is the default capture target.
- **Borderless or Windowed**, not Fullscreen, and **not minimised** during a
  run. Alt-tabbing to the PowerShell window is fine. Write down which mode and
  resolution. WGC gets nothing from a minimised window, and may get black or
  nothing from exclusive fullscreen; the report's brightness line says which.
- **Game sound on**, and audible through the default output device. Anything
  else playing (Discord, music) ends up in the file too.
- **Not elevated.** An ordinary PowerShell, because the daemon is not elevated.

## The runs

From the repository root, in an ordinary PowerShell:

```powershell
cd spikes\p0c-video
cargo build --release
$spike = ".\target\release\p0c-video.exe"

# 1. The machine: GPUs with vendor IDs, every H.264 encoder MF offers,
#    monitors, windows, and the two default audio endpoints. No capture.
#    Run it in game, so it also shows the game window it will capture.
& $spike --list 2>&1 | Tee-Object list.txt

# 2. The drift run: ten minutes, hardware encoder, finalized cleanly, then the
#    file check. Keep playing (walk, cast, fight a dummy) the whole time.
& $spike --out clean.mp4 2>&1 | Tee-Object clean.txt

# 3. The kill run: the same capture as a child, terminated at 300 s with no
#    finalize, then the file check on what it left.
& $spike --out killed.mp4 --kill-after 300 2>&1 | Tee-Object killed.txt

# 4. #68's second arm: Microsoft's software H.264 encoder, two minutes.
& $spike --encoder software --seconds 120 --out software.mp4 2>&1 | Tee-Object software.txt
```

Run 2 takes ten minutes plus a minute or two for ffmpeg to decode the result;
run 3, five minutes plus the check. The status line every ten seconds is the
live view: ticks written, frames WGC delivered, how many ticks repeated a
frame, the worst lateness, and the audio drift so far.

If run 2 refuses with **"no hardware H.264 encoder"**, that is the plan's
"refuse on none" doing its job and is a finding: keep the output, then check
`--adapter` against `--list` (a laptop may list the iGPU first).

### Killing it by hand instead

`--kill-after` makes the kill land at the same instant every time. To do it
the way the issue describes as well:

```powershell
& $spike --out manual.mp4 2>&1 | Tee-Object manual.txt
# When the status line passes 300s: Task Manager, Details tab,
# p0c-video.exe, End task. (Or, from a second PowerShell:
# Stop-Process -Name p0c-video -Force, which is the same call.)
& $spike --verify manual.mp4 2>&1 | Tee-Object manual-verify.txt
```

`--verify` reads `manual.mp4.progress`, which the run appends to every second,
so it can still say how much of what was fed to the encoder never reached the
disk.

### Optional runs, if there is time

```powershell
# The raw drift in the file, uncorrected: audio timestamped by sample count.
& $spike --audio-clock device --out device-clock.mp4 2>&1 | Tee-Object device-clock.txt
# A USB headset's microphone, whose clock is the likeliest to disagree.
& $spike --audio mic --out mic.mp4 2>&1 | Tee-Object mic.txt
```

## Reading the output

**The header** says what was used: Windows build, the GPU, what was offered,
what was loaded (`loaded`) and the transform chain the sink writer built. A
`WARNING` that the loaded encoder carries no hardware id means either a silent
software fallback or an MFT that does not report its attributes; compare the
name with `--list`. The last line, `border`, says whether WGC's yellow border
was turned off (#219): `off` where Windows supports it (build 20348 and later),
`on` with the reason where it does not, and the answer to the `Borderless`
access request either way. The border is drawn on screen only and never
reaches the file.

**The `== result ==` block** (clean runs only):

| Line | Means |
|---|---|
| `WGC` | Frames WGC delivered and their rate. League renders at its own frame rate, and WGC delivers only changed frames, so this is often not 60. |
| `grid` | Ticks written at 60 fps, and how many repeated the previous frame because nothing new had arrived. Repeats are the design, not a fault. |
| `video timing` | The worst tick written late, in frames, and how old the frame shown at a tick was. Both should stay well under one frame. |
| `content` | Brightness sampled from the captured texture. Near zero means WGC delivered black: check the window mode. |
| `raw drift` | The sound card's clock against the performance counter, at the end and at worst, plus the rate in ppm. What a pipeline that trusted the sample count would put in the file. |
| `correction` | How many single samples were dropped or repeated to hold audio on the video's clock, and any holes or overlaps. |
| `in the file` | The A/V misalignment actually written, at the end and at worst. |

**The file check** (every run):

| Line | Means |
|---|---|
| `fragmented` | `yes` if the `moov` carries `mvex`. `NO` means the sink wrote an ordinary MP4, and a kill would leave nothing. |
| `fragments` | `moof` count, complete fragments, and whether the finalize wrote `mfra`. A killed file has no `mfra`, which is expected. |
| `tail` | The box the kill cut short, if any. |
| `video decoded` / `keyframes` | What ffmpeg got out of the video track, and how often a keyframe came. Fragments close on keyframes, so this bounds what a kill costs. |
| `tail lost` | Frames fed to the encoder but not in the file (kill runs): the real cost of a crash. |
| `A/V end offset` | Audio end minus video end as decoded. The run pads audio to the last video tick before finalizing, so an offset here was added by the encoder or muxer. One AAC frame is 1024 samples (21 ms at 48 kHz, more than a video frame), so read it to that resolution. |
| `faststart remux` | The app's own finalize (`remux.rs`): stream copy to an ordinary MP4. If it works on a killed file, the daemon can recover one the way it finishes a clean one. |
| `verdict` | `PLAYABLE`, `PLAYABLE, WITH DECODER ERRORS`, `NOT PLAYABLE: <why>`, or `UNDECIDED` when no ffmpeg was found. |

**The `== DEVELOPMENT.md §16, P0c-2 rows ==` block** at the end of runs 2 and
3 is the same numbers phrased as the table's rows.

**Then open `clean.mp4` and `killed.mp4` in a player** (VLC, or Films & TV) and
say whether each plays from the start, seeks, and shows the game with sound.
The numbers cannot tell you that the picture is the game.

### What passes

| Clause | Pass |
|---|---|
| Drift | The written figure is under one frame, and the file's end offset agrees with it to within one AAC frame. If the raw figure is under one frame too, correction was not even needed for ten minutes; scale its ppm to a 40-minute game before concluding it never is. |
| Killed file | `verdict PLAYABLE` (or with errors confined to the cut) on `killed.mp4`, the remux succeeds, and it plays in a player. |
| Encoder | NVENC loaded with a vendor matching the GPU in run 2, and the software encoder initialises in run 4. Whether software holds 60 fps at this resolution is its own finding: see its `video timing` line. |

## What to bring back

Paste into #8:

1. `list.txt`, `clean.txt`, `killed.txt` and `software.txt` in full, plus any
   optional runs.
2. League's window mode and resolution, the patch, and the GPU driver version.
3. Whether each MP4 played, seeked, and showed the game with sound.

**Do not attach the MP4s.** They are large, and system loopback records
whatever the machine played, which may include other people's voices from a
call. The repository is public.

The P0c-2 rows in DEVELOPMENT.md §16's measurement table are where the result
is written down; that is WS1.5 (#9). Leave them empty until a run has
produced them.

## What it does not prove

It is a spike, not `recorder/own/`, and each shortcut below is one WS1.6 has
to replace. None of them can make a pass look better than the real pipeline
would, which is why they are acceptable here.

- **Colour conversion** is the video processor the sink writer inserts, not
  the plan's own BGRA-to-NV12 shader. The `chain` line shows it.
- **One audio source.** The plan's resampler places mic, process loopback and
  system audio on one clock; this puts one endpoint on the video's clock by
  slipping single samples. It shows whether the drift is correctable and how
  large the correction is, not what a resampler sounds like.
- **One hardware vendor**, per #68.
- **Ten minutes**, not a full game. The ppm figure is what extrapolates.
