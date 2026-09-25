<#
.NOTES
    Most of this can be exercised without a Windows box. PowerShell 7 runs on
    Linux and macOS, and `Get-Process`'s PrivateMemorySize64 and WorkingSet64
    are both populated there, so the sampling loop, the aggregation, the
    install-size branch, the error paths and the emitted row can all be run
    for real:

        pwsh -File ./scripts/measure.ps1 -ProcessName <something-running> `
             -Label 'smoke test' -Seconds 4 -IntervalMs 500

    What cannot: `-ArgumentFilter` and `-Role`, which need Win32_Process via
    CIM. Off Windows they find nothing and the script says so.

    `-Cpu` runs the same way (TotalProcessorTime is populated off Windows
    too), and `-SelfTest` checks the CPU arithmetic and the role matching
    against fixed samples, with no process involved:

        pwsh -File ./scripts/measure.ps1 -Cpu -ProcessName pwsh `
             -Label 'smoke test' -Seconds 4 -IntervalMs 500
        pwsh -File ./scripts/measure.ps1 -SelfTest

    Written for Windows PowerShell 5.1, which is what the box has: no `??`,
    no `? :`, no `-Parallel`, nothing newer than .NET Framework 4.x.

    Deliberately ASCII-only, including the markdown row it emits. Windows
    PowerShell 5.1 reads a .ps1 as the ANSI code page unless the file carries a
    UTF-8 BOM, so a stray en dash in here is a mojibake bug waiting for the one
    machine that runs the older shell -- which is the machine this is for.

.SYNOPSIS
    Samples memory and install size, or CPU with -Cpu, and emits the markdown
    rows for docs/windows-verification.md.

.DESCRIPTION
    WS0 task 0.3. The point is repeatability: WS0.2 takes the v1 baseline with
    this script and WS7.1 takes the v2.0.0 figure with the same script, so the
    two numbers are comparable by construction rather than by everyone
    remembering to do it the same way.

    The method is docs/measurement.md. In short: Private Bytes is the headline
    because it excludes shared pages, Working Set is recorded alongside it
    because that is what Task Manager shows, and both are sampled over time and
    reported as min/median/max rather than read once.

    With -Cpu it samples CPU instead (docs/measurement.md section 4): each
    process's TotalProcessorTime, read at every interval, and the deltas
    divided by the wall time between the reads. It reports the mean over the
    whole run and the busiest single interval, each as a percentage of the
    whole machine (all logical processors) and of one core.

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

.PARAMETER Cpu
    Sample CPU rather than memory, for -Seconds at every -IntervalMs, and emit
    one row per target (and a total row when there is more than one).

.PARAMETER Role
    With -Cpu, measure these processes side by side in one run, each found by
    image and command line, instead of the one -ProcessName/-ArgumentFilter
    target:

        daemon   <ProcessName> with --daemon on its command line
        ui       <ProcessName> with neither --daemon nor --capture-worker
        worker   <ProcessName> with --capture-worker (the own backend)
        libobs   extprocess_recorder (the libobs backend's worker)

    A role with no process running is reported and skipped, since "no capture
    worker" is the true idle state; at least one role must match something.

.PARAMETER SelfTest
    Check the CPU arithmetic and the role matching against fixed samples, and
    exit non-zero on any failure. Touches no process.

.EXAMPLE
    .\scripts\measure.ps1 -Label 'v1 0.8.0 - window closed' `
        -InstallPath "$env:LOCALAPPDATA\ninja-recorder"

.EXAMPLE
    .\scripts\measure.ps1 -Label 'v2.0.0 - daemon only' -ArgumentFilter '--daemon'

.EXAMPLE
    .\scripts\measure.ps1 -Cpu -ProcessName ninja-recorder-dev `
        -Role daemon,ui,worker,libobs -Label 'own (hardware) - recording'
#>
[CmdletBinding(DefaultParameterSetName = 'Measure')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute(
    'PSAvoidUsingWriteHost', '',
    Justification = 'The progress and summary lines are for a human at a console and are not data. The one thing that *is* data -- the markdown row -- goes to Write-Output, so it can still be piped or captured.')]
param(
    [string] $ProcessName = 'ninja-recorder',
    [string] $ArgumentFilter,
    [int]    $Seconds = 60,
    [int]    $IntervalMs = 1000,
    [string] $InstallPath,
    [Parameter(Mandatory = $true, ParameterSetName = 'Measure')]
    [string] $Label,
    [Parameter(ParameterSetName = 'Measure')]
    [switch] $Cpu,
    [Parameter(ParameterSetName = 'Measure')]
    [ValidateSet('daemon', 'ui', 'worker', 'libobs')]
    [string[]] $Role,
    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch] $SelfTest
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

# --- CPU (docs/measurement.md section 4) ------------------------------------

# A percentage with two decimals and a '.' whatever the culture. `-f` and
# ToString() format with the current culture, which on a German or French
# Windows is a ',' -- and that would not paste into a table the same way.
function Format-Pct {
    param([double] $Value)
    $Value.ToString('F2', [Globalization.CultureInfo]::InvariantCulture)
}

# The arithmetic, pure, so -SelfTest can call it with fake samples.
#
# $WallMs[i] is a stopwatch reading and $CpuMs[i] the total processor time of
# the target's processes (every thread, user plus kernel) read right after
# it, both cumulative. So:
#
#   one interval, % of one core  = 100 * dCpu / dWall
#   the mean, % of one core      = 100 * (CpuLast - CpuFirst) / (WallLast - WallFirst)
#   % of the machine             = % of one core / logical processors
#
# The mean is taken over the whole span rather than averaged over intervals,
# so an interval that ran long (the sleep overshot) weighs what it lasted.
# The peak is the busiest single interval. % of one core can exceed 100: a
# process with three busy threads is 300% of one core.
function Get-CpuStats {
    param(
        [double[]] $WallMs,
        [double[]] $CpuMs,
        [int] $LogicalProcessors
    )
    if ($WallMs.Count -ne $CpuMs.Count) {
        throw "Got $($WallMs.Count) wall readings and $($CpuMs.Count) CPU readings; they must pair up."
    }
    if ($WallMs.Count -lt 2) {
        throw 'CPU needs at least two readings, one interval. Check -Seconds and -IntervalMs.'
    }
    if ($LogicalProcessors -lt 1) {
        throw "Logical processor count $LogicalProcessors is not a count."
    }
    $last = $WallMs.Count - 1
    $peakCore = 0.0
    for ($i = 1; $i -le $last; $i++) {
        $dWall = $WallMs[$i] - $WallMs[$i - 1]
        $dCpu = $CpuMs[$i] - $CpuMs[$i - 1]
        if ($dWall -le 0) {
            throw "Interval $i has no wall time ($dWall ms)."
        }
        if ($dCpu -lt 0) {
            throw "Interval $i has negative CPU time ($dCpu ms): a process was replaced. The run is void."
        }
        $core = 100.0 * $dCpu / $dWall
        if ($core -gt $peakCore) { $peakCore = $core }
    }
    $meanCore = 100.0 * ($CpuMs[$last] - $CpuMs[0]) / ($WallMs[$last] - $WallMs[0])
    [pscustomobject]@{
        MeanCore          = $meanCore
        PeakCore          = $peakCore
        MeanMachine       = $meanCore / $LogicalProcessors
        PeakMachine       = $peakCore / $LogicalProcessors
        Intervals         = $last
        LogicalProcessors = $LogicalProcessors
    }
}

# Element-wise sum of several cumulative series, for the total row. Its peak
# is the busiest interval of the sum, which is not the sum of the peaks.
function Add-Series {
    param([object[]] $Series)
    $sum = New-Object double[] ($Series[0].Count)
    foreach ($s in $Series) {
        for ($i = 0; $i -lt $sum.Count; $i++) { $sum[$i] += $s[$i] }
    }
    # The comma keeps a one-element array an array on the way out.
    return , $sum
}

# One markdown row, in the shape of the memory row: label, process, figures,
# samples. The header is in docs/measurement.md section 4.
function Format-CpuRow {
    param([string] $RowLabel, [string] $Cell, $Stats, [int] $Interval)
    $hz = (1000.0 / $Interval).ToString('0.##', [Globalization.CultureInfo]::InvariantCulture)
    "| $RowLabel | $Cell | $(Format-Pct $Stats.MeanMachine)% | $(Format-Pct $Stats.PeakMachine)% | " +
    "$(Format-Pct $Stats.MeanCore)% | $(Format-Pct $Stats.PeakCore)% | $($Stats.LogicalProcessors) | " +
    "$($Stats.Intervals) @ $hz Hz |"
}

# What each -Role means: an image name and what its command line must and
# must not contain. The daemon, the UI and the own backend's worker are all
# the same executable, so only argv tells them apart.
function Get-RoleSpec {
    param([string] $Name, [string] $Image)
    switch ($Name) {
        'daemon' { return @{ Image = $Image; Include = '--daemon'; Exclude = @(); Cell = "$Image --daemon" } }
        'worker' { return @{ Image = $Image; Include = '--capture-worker'; Exclude = @(); Cell = "$Image --capture-worker" } }
        'ui' { return @{ Image = $Image; Include = ''; Exclude = @('--daemon', '--capture-worker'); Cell = "$Image (UI)" } }
        'libobs' { return @{ Image = 'extprocess_recorder'; Include = ''; Exclude = @(); Cell = 'extprocess_recorder' } }
    }
    throw "Unknown role '$Name'."
}

# Whether a command line belongs to a role. A role that is told apart by what
# its command line lacks (the UI) needs the command line: an unreadable one
# ($null) is not evidence that the flag is absent.
function Test-CommandLine {
    param([string] $CommandLine, [string] $Include, [string[]] $Exclude)
    if ($Include) {
        if (-not $CommandLine) { return $false }
        if (-not $CommandLine.Contains($Include)) { return $false }
    }
    if ($Exclude -and $Exclude.Count -gt 0) {
        if (-not $CommandLine) { return $false }
        foreach ($x in $Exclude) {
            if ($CommandLine.Contains($x)) { return $false }
        }
    }
    return $true
}

function Get-CommandLine {
    param([int] $ProcessId)
    try {
        $cim = Get-CimInstance Win32_Process -Filter "ProcessId = $ProcessId" -ErrorAction Stop
        if ($null -ne $cim) { return [string] $cim.CommandLine }
    } catch {
        return $null
    }
    return $null
}

# The processes a role names, resolved once like Resolve-TargetProcess. May
# be empty: a role that is not running is a state, not an error.
function Find-RoleProcess {
    param($Spec)
    $found = @()
    foreach ($p in @(Get-Process -Name $Spec.Image -ErrorAction SilentlyContinue)) {
        $needsArgs = [bool] $Spec.Include -or $Spec.Exclude.Count -gt 0
        $line = $null
        if ($needsArgs) { $line = Get-CommandLine $p.Id }
        if (Test-CommandLine $line $Spec.Include $Spec.Exclude) { $found += $p }
    }
    return $found
}

# Total processor time of a set of processes, in ms. Re-read per call: a
# Process object caches its counters. An exit voids the run, as for memory.
function Read-CpuMs {
    param([object[]] $Processes)
    $total = 0.0
    foreach ($p in $Processes) {
        $live = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
        if ($null -eq $live) {
            throw "Process $($p.Id) exited during sampling. The run is void; nothing is reported."
        }
        $total += $live.TotalProcessorTime.TotalMilliseconds
    }
    return $total
}

function Invoke-CpuMeasurement {
    if ($InstallPath) { throw '-InstallPath is for the memory run; leave it off with -Cpu.' }
    if ($Role -and $ArgumentFilter) {
        throw '-Role finds each process by command line itself; leave -ArgumentFilter off with it.'
    }

    $targets = @()
    if ($Role) {
        foreach ($r in $Role) {
            $spec = Get-RoleSpec $r $ProcessName
            $procs = @(Find-RoleProcess $spec)
            if ($procs.Count -eq 0) {
                Write-Host "$($spec.Cell): not running, so no row."
                continue
            }
            $targets += , @{ Cell = $spec.Cell; Procs = $procs }
        }
        if ($targets.Count -eq 0) {
            throw "None of the roles ($($Role -join ', ')) matched a running process. Check -ProcessName; if the processes are there, run this shell as the user that started them."
        }
    } else {
        $cell = $ProcessName
        if ($ArgumentFilter) { $cell = "$ProcessName $ArgumentFilter" }
        $targets += , @{ Cell = $cell; Procs = @(Resolve-TargetProcess) }
    }

    $logical = [Environment]::ProcessorCount
    foreach ($t in $targets) {
        Write-Host "$($t.Cell): $(@($t.Procs).Count) process(es), pid $(@($t.Procs | ForEach-Object { $_.Id }) -join ', ')"
    }
    Write-Host "Sampling CPU for ${Seconds}s at every ${IntervalMs}ms on $logical logical processors. Leave the machine in the state you are measuring."

    $wall = New-Object System.Collections.Generic.List[double]
    $cpu = @()
    foreach ($t in $targets) { $cpu += , (New-Object System.Collections.Generic.List[double]) }

    # Readings are scheduled against one stopwatch, not slept between, so a
    # slow read does not push every later one back; the arithmetic uses the
    # actual times either way.
    $clock = [System.Diagnostics.Stopwatch]::StartNew()
    $k = 0
    while ($true) {
        $wall.Add($clock.Elapsed.TotalMilliseconds)
        for ($i = 0; $i -lt $targets.Count; $i++) {
            $cpu[$i].Add((Read-CpuMs $targets[$i].Procs))
        }
        $k++
        $next = [double] $k * $IntervalMs
        if ($next -gt $Seconds * 1000.0) { break }
        $wait = [int] [math]::Ceiling($next - $clock.Elapsed.TotalMilliseconds)
        if ($wait -gt 0) { Start-Sleep -Milliseconds $wait }
    }

    $wallArray = $wall.ToArray()
    $rows = @()
    Write-Host ''
    for ($i = 0; $i -lt $targets.Count; $i++) {
        $stats = Get-CpuStats $wallArray $cpu[$i].ToArray() $logical
        Write-Host "$($targets[$i].Cell): mean $(Format-Pct $stats.MeanMachine)% of the machine ($(Format-Pct $stats.MeanCore)% of one core), peak interval $(Format-Pct $stats.PeakMachine)% ($(Format-Pct $stats.PeakCore)%)"
        $rows += Format-CpuRow $Label $targets[$i].Cell $stats $IntervalMs
    }
    if ($targets.Count -gt 1) {
        $sum = Add-Series @($cpu | ForEach-Object { , $_.ToArray() })
        $stats = Get-CpuStats $wallArray $sum $logical
        $cell = 'total: ' + (@($targets | ForEach-Object { $_.Cell }) -join ' + ')
        Write-Host "total: mean $(Format-Pct $stats.MeanMachine)% of the machine, peak interval $(Format-Pct $stats.PeakMachine)%"
        $rows += Format-CpuRow $Label $cell $stats $IntervalMs
    }
    Write-Host ''
    Write-Host 'Rows for the CPU table (docs/measurement.md section 4):'
    Write-Host ''
    foreach ($row in $rows) { Write-Output $row }
}

function Invoke-SelfTest {
    $script:failures = 0
    function Assert-Near([double] $Actual, [double] $Expected, [string] $What) {
        if ([math]::Abs($Actual - $Expected) -gt 1e-9) {
            Write-Host "FAIL $What`: expected $Expected, got $Actual"
            $script:failures++
        } else {
            Write-Host "ok   $What"
        }
    }
    function Assert-True([bool] $Condition, [string] $What) {
        if ($Condition) { Write-Host "ok   $What" } else { Write-Host "FAIL $What"; $script:failures++ }
    }
    function Assert-Throws([scriptblock] $Block, [string] $What) {
        $threw = $false
        try { & $Block | Out-Null } catch { $threw = $true }
        Assert-True $threw $What
    }

    # Half a core, steadily, on four logical processors.
    $s = Get-CpuStats @(0, 1000, 2000, 3000) @(0, 500, 1000, 1500) 4
    Assert-Near $s.MeanCore 50 'steady: mean, one core'
    Assert-Near $s.PeakCore 50 'steady: peak, one core'
    Assert-Near $s.MeanMachine 12.5 'steady: mean, machine'
    Assert-Near $s.PeakMachine 12.5 'steady: peak, machine'
    Assert-True ($s.Intervals -eq 3) 'steady: three intervals from four readings'

    # One busy second among quiet ones: the mean moves a little, the peak a lot.
    $s = Get-CpuStats @(0, 1000, 2000, 3000, 4000) @(0, 100, 1100, 1200, 1300) 8
    Assert-Near $s.MeanCore 32.5 'spike: mean, one core'
    Assert-Near $s.PeakCore 100 'spike: peak, one core'
    Assert-Near $s.MeanMachine 4.0625 'spike: mean, machine'
    Assert-Near $s.PeakMachine 12.5 'spike: peak, machine'

    # A sleep that overshot: the long interval weighs what it lasted.
    $s = Get-CpuStats @(0, 1000, 3000) @(0, 200, 400) 1
    Assert-Near $s.MeanCore (40000.0 / 3000) 'uneven: mean over the span, not of the intervals'
    Assert-Near $s.PeakCore 20 'uneven: peak is per interval'

    # Several busy threads are more than one core.
    $s = Get-CpuStats @(0, 1000) @(0, 2500) 16
    Assert-Near $s.MeanCore 250 'threads: 250% of one core'
    Assert-Near $s.MeanMachine 15.625 'threads: 15.625% of 16 logical processors'

    # Readings need not start at zero; only the deltas count.
    $s = Get-CpuStats @(5000, 6000, 7000) @(90000, 90250, 90500) 2
    Assert-Near $s.MeanCore 25 'offset: cumulative counters from any origin'

    Assert-Throws { Get-CpuStats @(0) @(0) 4 } 'one reading is no interval'
    Assert-Throws { Get-CpuStats @(0, 1000) @(0) 4 } 'unpaired readings'
    Assert-Throws { Get-CpuStats @(0, 1000) @(0, 10) 0 } 'no logical processors'
    Assert-Throws { Get-CpuStats @(0, 1000, 1000) @(0, 10, 20) 4 } 'an interval with no wall time'
    Assert-Throws { Get-CpuStats @(0, 1000, 2000) @(0, 500, 100) 4 } 'CPU time going backwards'

    # The total's peak is the busiest interval of the sum.
    $first = [double[]] @(0, 1000, 1000)
    $second = [double[]] @(0, 0, 1000)
    $sum = Add-Series @($first, $second)
    Assert-True ($sum.Count -eq 3) 'sum: same length'
    Assert-Near $sum[2] 2000 'sum: element-wise'
    $s = Get-CpuStats @(0, 1000, 2000) $sum 1
    Assert-Near $s.PeakCore 100 'sum: peak of the sum, not the sum of the peaks (200)'

    Assert-True ((Format-Pct 12.3456) -eq '12.35') 'format: two decimals, rounded'
    Assert-True ((Format-Pct 0.5) -eq '0.50') 'format: a point, trailing zero kept'
    $row = Format-CpuRow 'own - idle' 'x --daemon' (Get-CpuStats @(0, 1000) @(0, 500) 4) 1000
    Assert-True ($row -eq '| own - idle | x --daemon | 12.50% | 12.50% | 50.00% | 50.00% | 4 | 1 @ 1 Hz |') "row: $row"

    $exe = 'ninja-recorder-dev'
    $daemon = Get-RoleSpec 'daemon' $exe
    $ui = Get-RoleSpec 'ui' $exe
    $worker = Get-RoleSpec 'worker' $exe
    $libobs = Get-RoleSpec 'libobs' $exe
    $daemonLine = '"C:\Users\me\AppData\Local\ninja-recorder-dev\ninja-recorder-dev.exe" --daemon'
    $workerLine = '"C:\Users\me\AppData\Local\ninja-recorder-dev\ninja-recorder-dev.exe" --capture-worker'
    $uiLine = '"C:\Users\me\AppData\Local\ninja-recorder-dev\ninja-recorder-dev.exe"'
    Assert-True (Test-CommandLine $daemonLine $daemon.Include $daemon.Exclude) 'role: the daemon is the daemon'
    Assert-True (-not (Test-CommandLine $uiLine $daemon.Include $daemon.Exclude)) 'role: the UI is not the daemon'
    Assert-True (Test-CommandLine $uiLine $ui.Include $ui.Exclude) 'role: the UI is the UI'
    Assert-True (-not (Test-CommandLine $daemonLine $ui.Include $ui.Exclude)) 'role: the daemon is not the UI'
    Assert-True (-not (Test-CommandLine $workerLine $ui.Include $ui.Exclude)) 'role: the worker is not the UI'
    Assert-True (-not (Test-CommandLine $null $ui.Include $ui.Exclude)) 'role: an unreadable command line is not the UI'
    Assert-True (Test-CommandLine $workerLine $worker.Include $worker.Exclude) 'role: the worker is the worker'
    Assert-True (-not (Test-CommandLine $daemonLine $worker.Include $worker.Exclude)) 'role: the daemon is not the worker'
    Assert-True (Test-CommandLine $null $libobs.Include $libobs.Exclude) 'role: libobs is found by image alone'
    Assert-True ($libobs.Image -eq 'extprocess_recorder') 'role: libobs is extprocess_recorder'
    Assert-True ($daemon.Image -eq $exe -and $ui.Image -eq $exe -and $worker.Image -eq $exe) 'role: the three share -ProcessName'

    Write-Host ''
    if ($script:failures -gt 0) {
        Write-Host "$($script:failures) self-test failure(s)."
        exit 1
    }
    Write-Host 'All self-tests passed.'
    exit 0
}

if ($SelfTest) { Invoke-SelfTest }
if ($Cpu) {
    Invoke-CpuMeasurement
    return
}
if ($Role) { throw '-Role is for -Cpu. The memory run takes -ProcessName and -ArgumentFilter.' }

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
