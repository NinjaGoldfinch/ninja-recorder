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

.NOTES
    A headless runner is not a desktop. What this can assert is that the
    process starts, reaches `setup`, and connects; what it cannot assert is
    that anything is drawn. The window, the tray and the views stay in
    docs/windows-verification.md.
#>
[CmdletBinding()]
param(
    [string]$Exe = "src-tauri/target/debug/ninja-recorder.exe",
    [int]$TimeoutSeconds = 45
)

$ErrorActionPreference = 'Stop'
$failures = @()
function Note($m) { Write-Host "  $m" }
function Fail($m) { $script:failures += $m; Write-Host "  FAIL: $m" }

$logDir = Join-Path $env:APPDATA "com.ninjarecorder.app\logs"
$uiLog = Join-Path $logDir "ui.log"
$daemonLog = Join-Path $logDir "daemon.log"
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
    Write-Host "--- ui.log ---";     Get-Content $uiLog -EA SilentlyContinue
    Write-Host "--- daemon.log ---"; Get-Content $daemonLog -EA SilentlyContinue
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
    throw "no ui.log after $TimeoutSeconds seconds, and the process is still running"
}
Note "the UI reached setup and opened ui.log"

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

Show-Everything
Get-Process ninja-recorder -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue

if ($failures.Count -gt 0) {
    Write-Host ""
    throw "$($failures.Count) UI smoke check(s) failed: $($failures -join '; ')"
}
Write-Host ""
Write-Host "The UI starts, finds a daemon, and talks to it on Windows."
