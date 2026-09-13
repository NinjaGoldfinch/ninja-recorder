<#
.NOTES
    Most of this can be exercised without a Windows box. PowerShell 7 runs on
    Linux and macOS, and `Get-Process`'s PrivateMemorySize64 and WorkingSet64
    are both populated there, so the sampling loop, the aggregation, the
    install-size branch, the error paths and the emitted row can all be run
    for real:

        pwsh -File ./scripts/measure.ps1 -ProcessName <something-running> `
             -Label 'smoke test' -Seconds 4 -IntervalMs 500

    What cannot: `-ArgumentFilter`, which needs Win32_Process via CIM. Off
    Windows it finds nothing and the script says so.

    Deliberately ASCII-only, including the markdown row it emits. Windows
    PowerShell 5.1 reads a .ps1 as the ANSI code page unless the file carries a
    UTF-8 BOM, so a stray en dash in here is a mojibake bug waiting for the one
    machine that runs the older shell -- which is the machine this is for.

.SYNOPSIS
    Samples memory and install size, and emits the markdown row for
    docs/windows-verification.md section 5.

.DESCRIPTION
    WS0 task 0.3. The point is repeatability: WS0.2 takes the v1 baseline with
    this script and WS7.1 takes the v2.0.0 figure with the same script, so the
    two numbers are comparable by construction rather than by everyone
    remembering to do it the same way.

    The method is docs/measurement.md. In short: Private Bytes is the headline
    because it excludes shared pages, Working Set is recorded alongside it
    because that is what Task Manager shows, and both are sampled over time and
    reported as min/median/max rather than read once.

.PARAMETER ProcessName
    Process name without .exe. Defaults to ninja-recorder. The devtools bundle
    is ninja-recorder-dev.

.PARAMETER ArgumentFilter
    Only count processes whose command line contains this string. This is what
    separates the v2 daemon from the UI: both are ninja-recorder.exe and only
    argv tells them apart.

        -ArgumentFilter '--daemon'

    Reading another process's command line needs the CIM Win32_Process class,
    which needs an elevated shell for processes owned by other users. Same user,
    same session -- which is the case here -- needs no elevation.

.PARAMETER Seconds
    Sampling duration. Default 60.

.PARAMETER IntervalMs
    Sampling interval. Default 1000, i.e. 1 Hz.

.PARAMETER InstallPath
    Folder to size with Get-ChildItem -Recurse. Omit to skip the size half; a
    memory-only run is the common case when only a code change is being
    checked.

.PARAMETER Label
    What this run is, for the emitted row: the version and the state from
    docs/measurement.md section 1.3. Required, deliberately -- a row that says only
    "9 MB" is how the v1 baseline ended up needing a document to explain it.

.EXAMPLE
    .\scripts\measure.ps1 -Label 'v1 0.8.0 - window closed' `
        -InstallPath "$env:LOCALAPPDATA\ninja-recorder"

.EXAMPLE
    .\scripts\measure.ps1 -Label 'v2.0.0 - daemon only' -ArgumentFilter '--daemon'
#>
[CmdletBinding()]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute(
    'PSAvoidUsingWriteHost', '',
    Justification = 'The progress and summary lines are for a human at a console and are not data. The one thing that *is* data -- the markdown row -- goes to Write-Output, so it can still be piped or captured.')]
param(
    [string] $ProcessName = 'ninja-recorder',
    [string] $ArgumentFilter,
    [int]    $Seconds = 60,
    [int]    $IntervalMs = 1000,
    [string] $InstallPath,
    [Parameter(Mandatory = $true)]
    [string] $Label
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Format-Mb {
    param([double] $Bytes)
    # One decimal place. The figures this reports run from single-digit MB to a
    # few hundred, and a second decimal would imply a precision that sampling a
    # live allocator does not have.
    '{0:N1}' -f ($Bytes / 1MB)
}

function Get-Median {
    param([double[]] $Values)
    if ($Values.Count -eq 0) { return 0 }
    # The `@()` is load-bearing. `Sort-Object` returns a bare [double] rather
    # than an array when handed exactly one element, and under
    # `Set-StrictMode -Version Latest` reading `.Count` off that is a
    # terminating error -- so a one-sample run used to do all the sampling and
    # then die on "The property 'Count' cannot be found on this object".
    # Reachable with `-Seconds 1`, or on any machine slow enough that the loop
    # gets around once.
    $sorted = @($Values | Sort-Object)
    $mid = [int][math]::Floor($sorted.Count / 2)
    # Median rather than mean: one GC pause or one page-in should move the
    # answer by nothing, and with a mean it moves it by a little every time.
    if ($sorted.Count % 2 -eq 1) { return $sorted[$mid] }
    return ($sorted[$mid - 1] + $sorted[$mid]) / 2
}

# Which PIDs to count.
#
# Resolved once, before sampling, rather than per sample: a process that exits
# mid-run should make the run fail loudly, not silently halve the figure. A
# process that *starts* mid-run is likewise not part of what was asked for.
function Resolve-TargetProcess {
    $procs = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)
    if ($procs.Count -eq 0) {
        throw "No process named '$ProcessName' is running. Start it, put it in the state you are measuring, then run this again."
    }

    if (-not $ArgumentFilter) { return $procs }

    $matched = @()
    foreach ($p in $procs) {
        $cim = Get-CimInstance Win32_Process -Filter "ProcessId = $($p.Id)" -ErrorAction SilentlyContinue
        if ($null -ne $cim -and $cim.CommandLine -and $cim.CommandLine.Contains($ArgumentFilter)) {
            $matched += $p
        }
    }
    if ($matched.Count -eq 0) {
        throw "Found $($procs.Count) '$ProcessName' process(es), none with '$ArgumentFilter' in its command line. If the command line came back empty, run this shell as the same user that started the process."
    }
    return $matched
}

# `@()` again, and for the sharper version of the same reason: `return`
# unrolls a one-element array into a scalar, so a filter that matched exactly
# one process would hand back a bare [Process] and `$targets.Count` below
# would throw under StrictMode. One match is not the edge case -- it is the
# headline case, since `-ArgumentFilter '--daemon'` is meant to find exactly
# one daemon.
$targets = @(Resolve-TargetProcess)
Write-Host "Measuring $($targets.Count) process(es): $($targets.Id -join ', ')"
Write-Host "Sampling for ${Seconds}s at every ${IntervalMs}ms. Leave the machine in the state you are measuring."

$privateSamples = New-Object System.Collections.Generic.List[double]
$workingSamples = New-Object System.Collections.Generic.List[double]

$deadline = (Get-Date).AddSeconds($Seconds)
$sampleCount = 0
while ((Get-Date) -lt $deadline) {
    $private = 0.0
    $working = 0.0
    foreach ($t in $targets) {
        # Re-read rather than reusing the cached object: Get-Process snapshots
        # its counters at creation, so sampling $t.PrivateMemorySize64 in a loop
        # would return the same value every time.
        $live = Get-Process -Id $t.Id -ErrorAction SilentlyContinue
        if ($null -eq $live) {
            throw "Process $($t.Id) exited during sampling. The run is void; nothing is reported."
        }
        # PrivateMemorySize64 is the private commit charge -- the Private Bytes
        # counter, without needing performance-counter access and without the
        # `name#1` instance-name collision that two processes sharing a binary
        # name cause. See docs/measurement.md section 1.1.
        $private += $live.PrivateMemorySize64
        $working += $live.WorkingSet64
    }
    $privateSamples.Add($private)
    $workingSamples.Add($working)
    $sampleCount++
    Start-Sleep -Milliseconds $IntervalMs
}

if ($sampleCount -eq 0) {
    throw "No samples taken. Check -Seconds and -IntervalMs."
}

$pMin = Format-Mb ($privateSamples | Measure-Object -Minimum).Minimum
$pMed = Format-Mb (Get-Median $privateSamples.ToArray())
$pMax = Format-Mb ($privateSamples | Measure-Object -Maximum).Maximum
$wMin = Format-Mb ($workingSamples | Measure-Object -Minimum).Minimum
$wMed = Format-Mb (Get-Median $workingSamples.ToArray())
$wMax = Format-Mb ($workingSamples | Measure-Object -Maximum).Maximum

$sizeCell = '-'
if ($InstallPath) {
    if (-not (Test-Path $InstallPath)) {
        throw "Install path '$InstallPath' does not exist."
    }
    # The project's own existing method, unchanged, which is what makes the
    # figure comparable with the 248 MB already recorded for v1.
    $sum = (Get-ChildItem -Path $InstallPath -Recurse -File -Force |
        Measure-Object -Property Length -Sum).Sum
    $sizeCell = "$(Format-Mb $sum) MB"
}

Write-Host ''
Write-Host "Private Bytes  min $pMin MB  median $pMed MB  max $pMax MB"
Write-Host "Working Set    min $wMin MB  median $wMed MB  max $wMax MB"
if ($InstallPath) { Write-Host "Install        $sizeCell" }
Write-Host ''
Write-Host 'Row for docs/windows-verification.md section 5:'
Write-Host ''

$procCell = if ($ArgumentFilter) { "$ProcessName $ArgumentFilter" } else { $ProcessName }
$row = "| $Label | $procCell | $pMed MB | $pMin-$pMax MB | $wMed MB | $wMin-$wMax MB | $sizeCell | $sampleCount @ $([math]::Round(1000 / $IntervalMs, 2)) Hz |"
Write-Output $row
