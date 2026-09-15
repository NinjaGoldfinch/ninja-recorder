# Measurement method

How install size and memory are measured for this project, so that a number in
[windows-verification.md](windows-verification.md) §5 means the same thing
every time it is taken.

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
