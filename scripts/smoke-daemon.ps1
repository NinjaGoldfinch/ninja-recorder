<#
.SYNOPSIS
    Starts the daemon on Windows, proves it answers over its named pipe, and
    prints what it logged.

.DESCRIPTION
    Everything in the `test` job above this is a claim about types, units and
    framing. None of it starts the binary. This does: it runs `--daemon` on a
    real Windows box, waits for the pipe to appear, completes the handshake,
    runs a command, checks who is allowed to connect, and confirms that a second
    daemon leaves quietly.

    It exists because a week of work on the daemon could only be verified by
    someone with a Windows machine in front of them, and because the first real
    report from one was "a console window appears and instantly closes" with no
    log to explain it. A CI job that starts the thing would have answered that
    in four minutes.

    Runnable by hand, too, which is half the point:

        pwsh scripts/smoke-daemon.ps1

.PARAMETER Exe
    The binary to start. Defaults to the debug build, which is what CI has
    already compiled by this point.

.PARAMETER Build
    Which build `Exe` is: `devtools` (the default, because that is what CI
    compiles here) or `release`. It decides the pipe name and the log file,
    both of which carry the build: `daemon-devtools.log` or `daemon.log`.

.PARAMETER PipeName
    The endpoint to expect, without the `\\.\pipe\` prefix. Defaults to the
    one `Build` names.

.NOTES
    Deliberately not a `cargo test`. The protocol tests already drive `serve`
    over a real pipe in-process; what is missing from them is the *process*:
    `main.rs`, `Paths::resolve`, the single-instance check, the log file, and
    the security descriptor. Those only exist when something is launched.
#>
[CmdletBinding()]
param(
    [string]$Exe = "src-tauri/target/debug/ninja-recorder.exe",
    [ValidateSet('devtools', 'release')]
    [string]$Build = 'devtools',
    [string]$PipeName = "ninja-recorder.com.ninjarecorder.app.$Build",
    [int]$StartTimeoutSeconds = 30
)

$ErrorActionPreference = 'Stop'
$failures = @()

function Note($message) { Write-Host "  $message" }
function Fail($message) { $script:failures += $message; Write-Host "  FAIL: $message" }

$logDir = Join-Path $env:APPDATA "com.ninjarecorder.app\logs"
# Per build since #202, because both builds share this directory.
$logName = if ($Build -eq 'devtools') { "daemon-devtools.log" } else { "daemon.log" }
$logFile = Join-Path $logDir $logName
$stdout = Join-Path $env:TEMP "daemon-stdout.txt"
$stderr = Join-Path $env:TEMP "daemon-stderr.txt"

# A log from an earlier run would make "what did it say" ambiguous.
if (Test-Path $logFile) { Remove-Item $logFile -Force }

if (-not (Test-Path $Exe)) { throw "no binary at $Exe" }
Write-Host "Starting $Exe --daemon"

# `-NoNewWindow` with both streams redirected, because a debug build is
# console-subsystem and `main.rs` reports a refusal to start on stderr. That is
# the message that goes nowhere in a release build, which is exactly why a
# failure here was invisible in the field.
$daemon = Start-Process -FilePath $Exe -ArgumentList '--daemon' -PassThru -NoNewWindow `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr

$deadline = (Get-Date).AddSeconds($StartTimeoutSeconds)
$bound = $false
while ((Get-Date) -lt $deadline) {
    if ($daemon.HasExited) { break }
    if ([System.IO.Directory]::GetFiles("\\.\pipe\") -contains "\\.\pipe\$PipeName") {
        $bound = $true
        break
    }
    Start-Sleep -Milliseconds 250
}

if ($daemon.HasExited) {
    Write-Host "The daemon exited with code $($daemon.ExitCode) before binding anything."
    Write-Host "--- stderr ---"; Get-Content $stderr -EA SilentlyContinue
    Write-Host "--- stdout ---"; Get-Content $stdout -EA SilentlyContinue
    Write-Host "--- $logName ---"; Get-Content $logFile -EA SilentlyContinue
    throw "the daemon did not stay running"
}
if (-not $bound) {
    Stop-Process -Id $daemon.Id -Force -EA SilentlyContinue
    Write-Host "--- $logName ---"; Get-Content $logFile -EA SilentlyContinue
    throw "no pipe named $PipeName appeared within $StartTimeoutSeconds seconds"
}
Note "the pipe is bound: \\.\pipe\$PipeName"

try {
    # --- the handshake, over a real named pipe -----------------------------
    $pipe = New-Object System.IO.Pipes.NamedPipeClientStream '.', $PipeName, ([System.IO.Pipes.PipeDirection]::InOut)
    $pipe.Connect(10000)
    $reader = New-Object System.IO.StreamReader $pipe
    $writer = New-Object System.IO.StreamWriter $pipe
    $writer.AutoFlush = $true

    $writer.WriteLine('{"method":"hello","id":1,"protocol":1}')
    $hello = $reader.ReadLine()
    if ($hello -match '"type":"hello"') { Note "hello answered" } else { Fail "hello answered with: $hello" }
    if ($hello -match '"snapshot"') { Note "the snapshot rode along with it" } else { Fail "no snapshot in the hello reply" }

    # --- a command, so dispatch is exercised and not just framing ----------
    $writer.WriteLine('{"method":"invoke","id":2,"command":"game_state_status","args":{}}')
    $status = $reader.ReadLine()
    if ($status -match '"type":"ok"') { Note "a command ran in the daemon" } else { Fail "game_state_status answered with: $status" }

    # --- who is allowed to connect (issue #115) ----------------------------
    #
    # The security descriptor is the part of the daemon that had never run
    # anywhere. Reading it back is the only check that does not need a second
    # Windows account.
    $acl = $null
    try { $acl = $pipe.GetAccessControl() }
    catch { try { $acl = [System.IO.Pipes.PipeStreamAcl]::GetAccessControl($pipe) } catch { $acl = $null } }

    if ($null -eq $acl) {
        Note "could not read the pipe's ACL from this PowerShell; skipped rather than failed"
    } else {
        $who = $acl.Access | ForEach-Object { $_.IdentityReference.Value }
        Note "pipe ACL: $($who -join ', ')"
        foreach ($bad in @('Everyone', 'NT AUTHORITY\Authenticated Users', 'BUILTIN\Users')) {
            if ($who -contains $bad) { Fail "the pipe grants '$bad'; it must not" }
        }
        if (-not ($who | Where-Object { $_ -eq "$env:USERDOMAIN\$env:USERNAME" -or $_ -match [regex]::Escape($env:USERNAME) })) {
            Fail "the pipe does not grant the user that created it ($env:USERNAME)"
        }
    }

    $pipe.Dispose()
} catch {
    Fail "talking to the daemon threw: $_"
}

# --- the tray got an icon --------------------------------------------------
#
# A tray with no icon is an invisible click target, and the tray is the only way
# to reach a daemon that has no window. The daemon says so rather than failing,
# because recording matters more than being reachable, which means nothing would
# ever notice unless something read the log. This does.
#
# The first run of this script is what turned that from a worry into a fact: a
# `cargo build` binary has no embedded resource and no `icons/` beside it, so
# every lookup failed.
$log = Get-Content $logFile -EA SilentlyContinue
if ($log -match 'the tray will be invisible') {
    Fail "no icon could be loaded; the tray would be invisible"
} elseif ($log -match 'tray icon up') {
    Note "the tray came up with an icon"
} else {
    Note "no tray line in the log at all; this build may not have one"
}

# --- the log says which binary wrote it (#202) ------------------------------
if ($log -match [regex]::Escape("($Build build), pid $($daemon.Id)")) {
    Note "the log names the build and the pid"
} else {
    Fail "no line in $logName names the $Build build and pid $($daemon.Id)"
}

# --- a second daemon must leave quietly, and say nothing --------------------
#
# Compared by *reading* the file rather than by `(Get-Item).Length`. Windows
# serves a directory entry's size from cached metadata that can lag behind a
# file another process still holds open, so the stat version reported bytes that
# were written before it was ever called: it failed on one run and passed on
# another with identical code, which is worse than either.
function Read-Log { if (Test-Path $logFile) { Get-Content $logFile -Raw } else { "" } }

$before = Read-Log
$second = Start-Process -FilePath $Exe -ArgumentList '--daemon' -PassThru -NoNewWindow -Wait
if ($second.ExitCode -eq 0) { Note "a second daemon exited 0" } else { Fail "a second daemon exited $($second.ExitCode); expected 0" }
$after = Read-Log
if ($after -eq $before) {
    Note "and wrote nothing to the log"
} else {
    $added = $after.Substring([Math]::Min($before.Length, $after.Length))
    Fail "a second daemon added to the log: $($added.Trim())"
}

Stop-Process -Id $daemon.Id -Force -EA SilentlyContinue

Write-Host "--- $logName ---"
Get-Content $logFile -EA SilentlyContinue
Write-Host "--- stderr ---"
Get-Content $stderr -EA SilentlyContinue

if ($failures.Count -gt 0) {
    Write-Host ""
    throw "$($failures.Count) smoke check(s) failed: $($failures -join '; ')"
}
Write-Host ""
Write-Host "The daemon starts, serves, and stops on Windows."
