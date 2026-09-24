<#
.SYNOPSIS
    Starts the UI on Windows and proves it reaches its own startup, finds a
    daemon, and talks to it.

.DESCRIPTION
    The companion to `smoke-daemon.ps1`, and the half that answers a different
    question. That one asks whether the recorder runs; this asks whether the
    window process gets far enough to open its log, start a daemon if none is
    listening, and complete a handshake over the pipe.

    It exists because of a report this could not have been diagnosed without:
    "a console window appears and instantly closes", with `%APPDATA%\
    com.ninjarecorder.app` not existing at all, which means the UI died before
    `log::init` and left nothing behind. A process that exits before it can
    write a log is exactly what CI should be catching, because CI can read an
    exit code and a redirected stderr that a windowed build throws away.

    Runnable by hand:

        pwsh scripts/smoke-ui.ps1

.PARAMETER Exe
    The binary to start, with no arguments, which is what `Launch::Ui` means.

.PARAMETER Build
    Which build `Exe` is: `devtools` (the default, because that is what CI
    compiles) or `release`. Each build has its own data folder, and its log
    files carry the build too: a devtools build writes `ui-devtools.log` and
    `daemon-devtools.log` under `%APPDATA%\com.ninjarecorder.app.devtools\logs`,
    a release build `ui.log` and `daemon.log` under
    `%APPDATA%\com.ninjarecorder.app\logs`.

.NOTES
    A headless runner is not a desktop. What this can assert is that the
    process starts, reaches `setup`, and connects; what it cannot assert is
    that anything is drawn. The window, the tray and the views stay in
    docs/windows-verification.md.
#>
[CmdletBinding()]
param(
    [string]$Exe = "src-tauri/target/debug/ninja-recorder.exe",
    [ValidateSet('devtools', 'release')]
    [string]$Build = 'devtools',
    [int]$TimeoutSeconds = 45
)

$ErrorActionPreference = 'Stop'
$failures = @()
function Note($m) { Write-Host "  $m" }
function Fail($m) { $script:failures += $m; Write-Host "  FAIL: $m" }

# Each build has its own data folder since #222 (daemon::IDENTIFIER), and
# the log names still carry the build too (#202).
$dataDir = if ($Build -eq 'devtools') { "com.ninjarecorder.app.devtools" } else { "com.ninjarecorder.app" }
$logDir = Join-Path $env:APPDATA "$dataDir\logs"
$suffix = if ($Build -eq 'devtools') { "-devtools" } else { "" }
$uiLogName = "ui$suffix.log"
$daemonLogName = "daemon$suffix.log"
$uiLog = Join-Path $logDir $uiLogName
$daemonLog = Join-Path $logDir $daemonLogName
$stdout = Join-Path $env:TEMP "ui-stdout.txt"
$stderr = Join-Path $env:TEMP "ui-stderr.txt"

# Both logs, because the UI is expected to start a daemon and that daemon's log
# is half the evidence when it does not.
foreach ($f in @($uiLog, $daemonLog)) { if (Test-Path $f) { Remove-Item $f -Force } }
Get-Process ninja-recorder -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue

if (-not (Test-Path $Exe)) { throw "no binary at $Exe" }
Write-Host "Starting $Exe (no arguments: a normal UI start)"

$ui = Start-Process -FilePath $Exe -PassThru -NoNewWindow `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr

function Show-Everything {
    Write-Host "--- $uiLogName ---"; Get-Content $uiLog -EA SilentlyContinue
    Write-Host "--- $daemonLogName ---"; Get-Content $daemonLog -EA SilentlyContinue
    Write-Host "--- stderr ---";     Get-Content $stderr -EA SilentlyContinue
    Write-Host "--- stdout ---";     Get-Content $stdout -EA SilentlyContinue
}

# --- did it reach its own startup at all? ----------------------------------
#
# `log::init` is the first thing `setup` does, so the log file appearing is the
# earliest observable proof that the process got past building the Tauri app.
# A UI that dies before this is the reported failure, and the exit code below
# is the whole of what the field could not tell us.
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$reached = $false
while ((Get-Date) -lt $deadline) {
    if (Test-Path $uiLog) { $reached = $true; break }
    if ($ui.HasExited) { break }
    Start-Sleep -Milliseconds 250
}

if ($ui.HasExited -and -not $reached) {
    Write-Host "The UI exited with code $($ui.ExitCode) before writing a log."
    Show-Everything
    Get-Process ninja-recorder -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    throw "the UI process died before reaching setup"
}
if (-not $reached) {
    Show-Everything
    Get-Process ninja-recorder -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    throw "no $uiLogName after $TimeoutSeconds seconds, and the process is still running"
}
Note "the UI reached setup and opened $uiLogName"

# --- did it find a daemon? -------------------------------------------------
#
# Nothing was listening when this started, so the UI has to start one itself
# (`daemon::spawn::connect_or_start`) and complete a handshake over the pipe.
# `ui::link` logs the connection's health, so "Connected" in the log is the
# whole of WS3.4's path proving itself: window process, spawn, pipe, hello.
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$connected = $false
while ((Get-Date) -lt $deadline) {
    if ((Get-Content $uiLog -EA SilentlyContinue) -match 'daemon connection: Connected') {
        $connected = $true
        break
    }
    if ($ui.HasExited) { break }
    Start-Sleep -Milliseconds 500
}

if ($connected) {
    Note "it started a daemon and connected to it over the pipe"
} else {
    Fail "the UI never reported a connected daemon"
}

if ($ui.HasExited) { Fail "the UI exited with code $($ui.ExitCode) while we watched" }

# --- the daemon going away, and coming back (WS3.8) ------------------------
#
# The daemon can vanish for ordinary reasons: the updater replaces it, someone
# quits it from the tray, it crashes. The UI is disposable but a recording is
# not, so what the window must do is notice, say so, and recover — never sit
# there showing a library that has quietly stopped answering.
#
# Killed rather than asked to stop, because that is the case worth testing: a
# clean shutdown says goodbye on the wire first, and a crash says nothing at
# all.
if (-not $connected) {
    Note "skipping the recovery check: there was no connection to lose"
} else {
    $daemon = Get-Process ninja-recorder -EA SilentlyContinue |
        Where-Object { $_.Id -ne $ui.Id }
    if (-not $daemon) {
        Fail "no daemon process to kill; the UI reported connected without one"
    } else {
        $mark = (Get-Content $uiLog -Raw).Length
        $daemon | Stop-Process -Force
        Note "killed the daemon the UI was talking to"

        # Two things have to happen, in order: the UI has to *notice*, and it
        # has to come back. Noticing is what the strip in the window is for;
        # coming back is `connect_or_start` starting a new one.
        $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
        $noticed = $false
        $recovered = $false
        while ((Get-Date) -lt $deadline) {
            $since = (Get-Content $uiLog -Raw -EA SilentlyContinue)
            if ($since.Length -gt $mark) {
                $tail = $since.Substring($mark)
                if ($tail -match 'daemon connection: Reconnecting') { $noticed = $true }
                if ($tail -match 'daemon connection: Connected') { $recovered = $true; break }
            }
            if ($ui.HasExited) { break }
            Start-Sleep -Milliseconds 500
        }

        if ($noticed) { Note "the UI noticed it was gone" } else { Fail "the UI never reported losing the daemon" }
        if ($recovered) { Note "and started another, and reconnected" } else { Fail "the UI never reconnected" }
        if ($ui.HasExited) { Fail "the UI died when the daemon did; it is supposed to be the disposable one" }
    }
}

Show-Everything
Get-Process ninja-recorder -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue

if ($failures.Count -gt 0) {
    Write-Host ""
    throw "$($failures.Count) UI smoke check(s) failed: $($failures -join '; ')"
}
Write-Host ""
Write-Host "The UI starts, finds a daemon, and talks to it on Windows."
