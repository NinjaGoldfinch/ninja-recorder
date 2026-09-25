# Measurement method

How install size, memory and CPU are measured for this project, so that a
number in [windows-verification.md](windows-verification.md) means the same
thing every time it is taken. CPU is §4, added for the own backend's exit run
(#243).

This is WS0 task 0.1. It exists because the target it serves,
[DEVELOPMENT.md §1.2](../DEVELOPMENT.md)'s 100 MB idle-RAM ceiling, was
written without a method, and a target with no method is unfalsifiable. WS0.2
takes the v1 baseline with it; WS7.1 repeats it on the v2.0.0 release
candidate, with [`scripts/measure.ps1`](../scripts/measure.ps1) so that the two
runs are the same run.

---

## 1. Memory

### 1.1 Private Bytes is the headline

**Private Bytes** (private committed memory) is the figure this project
quotes. It excludes pages shared with other processes, so it is the honest
answer to "what does running this cost the machine". A number that counted
shared copies of `ntdll` would flatter every process on Windows equally and
tell nobody anything.

**Working Set is recorded alongside it**, because Working Set is what Task
Manager's "Memory" column shows, and a user comparing our claim against their
own machine is going to compare it against that. Recording both is what stops
the two numbers being mistaken for each other.

They are not interchangeable and the gap between them is not fixed: Working Set
can be *smaller* than Private Bytes when Windows has trimmed pages under
pressure, and much larger when shared DLLs dominate. Quote one, record both.

In PowerShell:

| Figure | Counter |
|---|---|
| Private Bytes | `Get-Counter "\Process(<name>)\Private Bytes"`, or `(Get-Process <name>).PrivateMemorySize64` |
| Working Set | `(Get-Process <name>).WorkingSet64` |

`Get-Process`'s `PrivateMemorySize64` is the private *commit* charge and is
what `measure.ps1` samples. It needs no performance-counter permissions and
is stable across the instance-name collisions that `\Process(name#1)\` paths
run into when two processes share a binary name, which is exactly the v2
daemon-plus-UI case.

### 1.2 Sample over time, report min / median / max

A single reading catches whatever the allocator happened to be doing. Sample at
a fixed interval for a fixed duration (`measure.ps1` defaults to 1 Hz for 60
seconds) and report the three numbers. A resting process should show a flat
line; if it does not, the spread is the finding and a single figure would have
hidden it.

### 1.3 The three states

Memory is measured in three states, and they are three different numbers:

| State | What is running | Why it is measured |
|---|---|---|
| **Window closed** | The app is in the tray, no window, no League client | v1's tray-resident resting state |
| **Window open** | The main window is showing | What the webview costs |
| **League client open** | The League client is running, so the capture backend is warm | The backend is only warm while the client is up ([DEVELOPMENT.md §2.2](../DEVELOPMENT.md)), so this is the third and highest number |

In v2 these become four, because the process model changes:

| v2 state | What is running |
|---|---|
| **Daemon only** | `ninja-recorder.exe --daemon`, no UI, no League client |
| **Daemon + UI** | Both processes, window showing |
| **Daemon + League client** | Daemon with the capture backend warm |
| **Daemon + UI + League client** | All of it |

### 1.4 The daemon-only state is the one the ceiling describes

**C3's 100 MB is a statement about the resting state, and in v2 the resting
state is the daemon alone.** That is what runs at login, and what runs for the
twenty-three hours a day nobody has the window open. It is the figure to gate
on.

**The two-process total is recorded separately, and is not gated.** The cost of
having the UI open is real and belongs in the table, because hiding it inside a
single headline would be the same mistake as quoting Working Set as Private
Bytes. But a webview that exists only while someone is looking at it is not
what the ceiling is about. Record it; do not let it move the pass/fail.

### 1.5 What v1's 9 MB actually was

[windows-verification.md](windows-verification.md) §5 records **9 MB idle RAM**
for v1 0.8.0. That figure is:

- **Working Set**, not Private Bytes,
- with the **window closed**,
- from **Task Manager / `Get-Process`**, as a single reading rather than a
  sample.

The design document says idle RAM was never measured. It was, but as the one of
the six possible numbers that flatters the app most. It is not wrong and it
is not a like-for-like baseline for the v2 target, which is why WS0.2 retakes
it under this method before anything changes.

---

## 2. Install size

Measured on the installed folder, not the installer:

```powershell
Get-ChildItem -Recurse | Measure-Object -Property Length -Sum
```

This is the project's own existing method and the reason the 248 MB figure is
comparable across versions. The installer's own size is recorded next to it as
a second figure (v1's NSIS `.exe` is 64 MB, roughly a quarter of what it unpacks
to) because it is what a user downloads and is not what the target is
about.

The target is **200 MB** ([DEVELOPMENT.md §1.2](../DEVELOPMENT.md)). v1 misses
it at 248 MB, of which roughly 200 MB is libobs plus ffmpeg. That miss is
recorded rather than rounded; see §8 of the implementation plan, which requires
the same of v2.0.0: under 200 MB, or the miss documented with the number.

---

## 3. Recording the result

Every run lands as a row in [windows-verification.md](windows-verification.md)
§5, in the table this document's method fills in. `measure.ps1` emits that row
directly so the transcription step cannot introduce a typo:

```powershell
.\scripts\measure.ps1 -ProcessName ninja-recorder -Label "v1 0.8.0, window closed"
```

State what was measured, not just the number: the version, the state from §1.3,
and whether the figure is a daemon-only or a two-process total. A row that says
"9 MB" and nothing else is how the v1 baseline ended up needing this document.

---

## 4. CPU

What the recorder's own processes cost the processor, idle and while
recording, so the own backend can be compared with libobs, and its software
encoder with its hardware one. `measure.ps1 -Cpu` takes it.

### 4.1 What is measured

**CPU time consumed by each process**: every thread, user and kernel time
together, which is `Process.TotalProcessorTime`. It is read at every interval
and the differences are divided by the wall time between the reads, measured
with one stopwatch:

| Figure | Formula |
|---|---|
| **% of the machine** | CPU ms ÷ wall ms ÷ logical processors × 100 |
| **% of one core** | CPU ms ÷ wall ms × 100 (above 100 when more than one thread is busy) |

% of the machine is the headline: a share of the whole machine is the scale
Task Manager's per-process CPU column uses, so it is the one a user will
compare against. (They will not match exactly: Task Manager on recent Windows
11 also scales by clock frequency, "processor utility", which CPU time does
not.) % of one core is
recorded next to it because it does not depend on how many cores the box has,
so it is the figure that carries to another machine. The logical processor
count is in the row, so either can be recovered from the other.

Each is reported two ways:

- **the mean**, over the whole run: total CPU time over total wall time, not an
  average of the intervals, so an interval that ran long counts for what it
  lasted;
- **the peak interval**: the busiest single interval. A process that idles at
  0.1% and spends one second in ten at a full core has a low mean and a peak
  that says so.

**Why not `Get-Counter '\Process(*)\% Processor Time'`.** Its instances are
named by image, so the daemon, the UI and the own backend's worker, all
`ninja-recorder.exe`, are `ninja-recorder`, `ninja-recorder#1` and
`ninja-recorder#2`, and which is which is not stable: the suffixes are
reassigned when one of them exits. That is the collision §1.1 avoids for
memory. `TotalProcessorTime` is read by process id, so it has no such
ambiguity, and it needs no performance-counter permissions.

**Precision.** The CPU time Windows reports for a process can move in steps
as coarse as the clock tick, commonly 15.6 ms. Over a 60 s run that is negligible in the mean. In
a single one-second interval it is not, so treat a small peak as a bound, not a
reading.

### 4.2 Which processes

`-Role` finds each process by image and command line, the way
`-ArgumentFilter` does for memory, and measures them side by side in one run:

| Role | Process | Present |
|---|---|---|
| `daemon` | `<ProcessName> --daemon` | always |
| `ui` | `<ProcessName>` with neither flag | while the window is open |
| `worker` | `<ProcessName> --capture-worker` | own backend, while League runs (#241) |
| `libobs` | `extprocess_recorder` | libobs backend, while League runs |

A role with nothing running is skipped with a line saying so, because that is
the true state: own has no worker while the client is closed. With more than
one role matched, a **total** row follows, whose peak is the busiest interval
of the sum rather than the sum of each role's peak. Without `-Role`, `-Cpu`
measures the one `-ProcessName`/`-ArgumentFilter` target, summed, as the
memory run does.

**Not counted:** WebView2's own processes (`msedgewebview2.exe`), which render
the window, and work the capture causes in other processes (`dwm.exe` for
Windows.Graphics.Capture, `audiodg.exe` for audio). The UI and the WebView2
cost are the same whichever backend records, so they do not affect the
comparison; the other two are a known limit of the method, and a difference
there would show as game frame rate, not in these rows.

### 4.3 How to take it

```powershell
.\scripts\measure.ps1 -Cpu -ProcessName ninja-recorder-dev `
    -Role daemon,ui,worker,libobs -Label 'own (hardware) - recording'
```

It samples for `-Seconds` (default 60) at every `-IntervalMs` (default 1000),
the same as the memory run, then prints one row per role that matched and the
total:

```text
| own (hardware) - recording | ninja-recorder-dev --daemon | <mean>% | <peak>% | <mean>% | <peak>% | <logical CPUs> | 60 @ 1 Hz |
| own (hardware) - recording | ninja-recorder-dev --capture-worker | … |
| own (hardware) - recording | total: ninja-recorder-dev --daemon + ninja-recorder-dev --capture-worker | … |
```

The rows go under this header:

| State | Process | CPU, machine (mean) | Machine (peak interval) | CPU, one core (mean) | One core (peak interval) | Logical CPUs | Intervals |
|---|---|---|---|---|---|---|---|

A process that exits during the run voids it, as for memory: nothing is
printed. `.\scripts\measure.ps1 -SelfTest` checks the arithmetic against fixed
samples, and CI runs it in Windows PowerShell 5.1.

### 4.4 Holding everything else constant

A CPU figure is only comparable with one taken under the same load. For every
run in a comparison:

- **Practice Tool**, the same champion, standing at **the same spot on the
  map** (the fountain is easiest to return to), camera locked, nothing
  happening. A live game's load cannot be repeated.
- **The same game settings**: resolution, window mode, graphics quality and
  frame cap. The capture cost follows the frame size and rate.
- **The same recorder settings**: the same build (devtools or release), the
  same audio preset, and so the same sources opened.
- **No other load**: browsers, launchers and updaters closed; Discord in the
  same state for every run, or closed if the preset does not need it. On
  mains power, the same power plan.
- **Settled first.** Start sampling once the recording has run for a minute,
  past the start-up work (the window search, the encoder coming up).

The states, each on both backends:

| State | What is running | Roles |
|---|---|---|
| **Idle** | Daemon only, no League client | `daemon` |
| **Client open** | The client in the lobby, the backend warm | `daemon`, `worker` / `libobs` |
| **Recording** | A Practice Tool game, recording | `daemon`, `worker` / `libobs` |

### 4.5 Comparing backends fairly

- **Compare the totals**: own's `daemon + worker` against libobs's
  `daemon + libobs`. Each backend does its capture in a different process, so
  a per-process comparison is between different work.
- **Back to back, alternating.** Run own, libobs, own, libobs in one sitting,
  switching in Settings → Advanced between games, rather than all of one then
  all of the other: drift in what else the machine is doing then falls on both.
  At least three runs each; record every row, not an average of them. The
  spread between runs of the same backend is the noise floor, and a
  difference between backends smaller than it is not a difference.
- **The software encoder** is a third arm, not a variant of the first: own on a
  devtools daemon started with `NINJA_OWN_FORCE_SOFTWARE_ENCODER=1`, which
  takes [DEVELOPMENT.md §2.4](../DEVELOPMENT.md#24-encoding-defaults)'s
  fallback on a machine that has a hardware encoder. Its label should say
  `own (software)`, and its `diagnostics_json.backend` should confirm it.
- **Label the run fully**: backend, encoder, state, and the build, for
  example `v2.0.0-rc1 devtools, own (hardware), recording`.
