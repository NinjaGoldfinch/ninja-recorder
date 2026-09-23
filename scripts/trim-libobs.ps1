<#
.SYNOPSIS
    Trims a staged libobs directory to a keep-list, or inventories one.

.DESCRIPTION
    WS1 task 1.1, the P0a arm (#5). The trimmed backend is the fallback if the
    P0c spikes fail, and the selectable second backend for exactly one release
    if they pass (WS1.7). Either way its size is the thing being measured:
    libobs is a large share of a 248 MB install.

    Every staged file lands in one of three groups. KEPT matches a plain line
    of the keep-list. EXPECTED REMOVAL matches a `!` line. UNRECOGNISED matches
    neither, and the script refuses to go on while there is one: a libobs bump
    that adds a DLL something imports would otherwise be trimmed away without
    anyone having looked at it.

    -Inventory prints what is actually staged, grouped, with sizes, and marks
    each entry against the keep-list. It changes nothing.

    Without -Apply the script is a dry run: it prints the size before and
    after, and every file it would remove, and touches nothing. That is the
    default because the failure mode here is not a broken build. A plugin
    removed wrongly still compiles, still packages, still installs, and then
    does not capture, which is why the exit criterion for this task is a clean
    plugin-load log and a recording that plays rather than a green build.

    -Apply also requires the environment variable LIBOBS_TRIM to be 1, which the
    CI step sets for a trimmed devtools build and nothing else sets.

    The sizes it prints are the staged directory's, for the P0a notes. The
    install size that counts is measured on the installed build with
    measure.ps1 (docs/windows-verification.md section 8).

.NOTES
    Deliberately ASCII-only, like measure.ps1. Windows PowerShell 5.1 reads a
    .ps1 as the ANSI code page unless the file carries a UTF-8 BOM, so a stray
    en dash is a mojibake bug waiting for the one machine that runs the older
    shell.

    This operates on the STAGED copy under src-tauri/target/, never on a
    source tree or a cargo cache. It refuses to run anywhere that does not
    look like a staged libobs directory, because a glob-driven delete pointed
    at the wrong path is the one mistake this script could make that matters.

.PARAMETER Path
    The staged directory. Defaults to src-tauri/target/libobs, which is where
    the CI staging step and build.rs both put it.

.PARAMETER KeepList
    The keep-list file. Defaults to scripts/libobs-keep.txt beside this script.

.PARAMETER Inventory
    Print what is staged and how it matches the keep-list, and change nothing.

.PARAMETER Apply
    Actually delete. Without it the script reports and exits. Refused unless
    LIBOBS_TRIM is 1 and nothing staged is unrecognised.

.EXAMPLE
    ./scripts/trim-libobs.ps1 -Inventory

    What is there, what the keep-list covers, and what it does not recognise.

.EXAMPLE
    ./scripts/trim-libobs.ps1

    A dry run: the size before and after, and every file that would go.

.EXAMPLE
    $env:LIBOBS_TRIM = '1'; ./scripts/trim-libobs.ps1 -Apply

    The real thing. Then package, install, play a game, and fill in
    docs/windows-verification.md section 8.
#>

[CmdletBinding()]
param(
    [string] $Path = (Join-Path $PSScriptRoot '../src-tauri/target/libobs'),
    [string] $KeepList = (Join-Path $PSScriptRoot 'libobs-keep.txt'),
    [switch] $Inventory,
    [switch] $Apply
)

$ErrorActionPreference = 'Stop'

function Format-Size {
    param([long] $Bytes)
    if ($Bytes -ge 1GB) { return ('{0:N2} GB' -f ($Bytes / 1GB)) }
    if ($Bytes -ge 1MB) { return ('{0:N1} MB' -f ($Bytes / 1MB)) }
    if ($Bytes -ge 1KB) { return ('{0:N0} KB' -f ($Bytes / 1KB)) }
    return "$Bytes B"
}

if (-not (Test-Path -LiteralPath $Path)) {
    Write-Error "No staged libobs at $Path. The CI step 'Stage libobs capture backend' is what creates it; build.rs only makes an empty placeholder."
    exit 1
}

$root = (Resolve-Path -LiteralPath $Path).Path

# A glob-driven delete pointed at the wrong directory is the one mistake here
# that matters, so refuse anything that does not carry the two files a staged
# backend always has.
$looksStaged = (Test-Path -LiteralPath (Join-Path $root 'obs.dll')) -or
               (Test-Path -LiteralPath (Join-Path $root 'extprocess_recorder.exe'))
if (-not $looksStaged) {
    Write-Error "$root has neither obs.dll nor extprocess_recorder.exe, so it is not a staged libobs directory. Refusing to delete anything in it."
    exit 1
}

if (-not (Test-Path -LiteralPath $KeepList)) {
    Write-Error "No keep-list at $KeepList."
    exit 1
}

$lines = @(Get-Content -LiteralPath $KeepList |
    ForEach-Object { $_.Trim() } |
    Where-Object { $_ -and -not $_.StartsWith('#') })

# A plain line keeps; a `!` line names an expected removal.
$patterns = @($lines | Where-Object { -not $_.StartsWith('!') })
$dropPatterns = @($lines | Where-Object { $_.StartsWith('!') } | ForEach-Object { $_.Substring(1).Trim() })

if (-not $patterns) {
    Write-Error "The keep-list at $KeepList has no keep patterns. That would remove everything; refusing."
    exit 1
}

# `**` means "any depth", which -like does not know about, so it becomes `*`.
# Everything else is a plain wildcard match on a forward-slashed relative path.
$matchers = @($patterns | ForEach-Object { $_.Replace('**', '*') })
$dropMatchers = @($dropPatterns | ForEach-Object { $_.Replace('**', '*') })

function Test-AnyMatch {
    param([string] $Relative, [string[]] $Globs)
    foreach ($glob in $Globs) {
        if ($Relative -like $glob) { return $true }
    }
    return $false
}

# A plain loop, not `@($Entries) | Measure-Object`: wrapping a generic List
# passed in as a parameter in `@()` throws "Argument types do not match"
# under PowerShell 7, which is what stopped this script's first real run.
function Get-Bytes {
    param($Entries)
    $sum = [long]0
    foreach ($entry in $Entries) { $sum += [long]$entry.Bytes }
    return $sum
}

$files = @(Get-ChildItem -LiteralPath $root -Recurse -File)

$kept = New-Object System.Collections.Generic.List[object]
$expected = New-Object System.Collections.Generic.List[object]
$unknown = New-Object System.Collections.Generic.List[object]

foreach ($file in $files) {
    $relative = ($file.FullName.Substring($root.Length) -replace '^[\\/]+', '').Replace('\', '/')
    $entry = [pscustomobject]@{
        Relative = $relative
        Bytes    = $file.Length
        Full     = $file.FullName
    }
    # Keep wins over a removal, so an overlap errs toward a bigger bundle
    # rather than a broken one.
    if (Test-AnyMatch $relative $matchers) { $kept.Add($entry) }
    elseif ($dropMatchers.Count -and (Test-AnyMatch $relative $dropMatchers)) { $expected.Add($entry) }
    else { $unknown.Add($entry) }
}

# Everything not kept is what a trim would remove; whether it is allowed to
# is the split between expected and unrecognised.
# `.ToArray()` first, for the reason Get-Bytes gives.
$doomed = @($expected.ToArray()) + @($unknown.ToArray())

$total = if ($files.Count) { [long](($files | Measure-Object -Property Length -Sum).Sum) } else { [long]0 }
$keptBytes = Get-Bytes $kept
$doomedBytes = Get-Bytes $doomed
$unknownBytes = Get-Bytes $unknown

Write-Host ''
Write-Host "Staged libobs: $root"
Write-Host ("  files  {0}" -f $files.Count)
Write-Host ("  size   {0}" -f (Format-Size $total))
Write-Host ''

if ($Inventory) {
    # Grouped by directory, because the interesting question at this point is
    # "which plugins are even here", not "which 400 files are here".
    Write-Host 'By directory:'
    $files |
        Group-Object { $d = Split-Path $_.FullName -Parent; ($d.Substring($root.Length) -replace '^[\\/]+', '').Replace('\', '/') } |
        Sort-Object { ($_.Group | Measure-Object -Property Length -Sum).Sum } -Descending |
        ForEach-Object {
            $where = if ($_.Name) { $_.Name } else { '(root)' }
            $sum = ($_.Group | Measure-Object -Property Length -Sum).Sum
            Write-Host ("  {0,10}  {1,4} file(s)  {2}" -f (Format-Size $sum), $_.Count, $where)
        }

    Write-Host ''
    Write-Host 'Unrecognised, on neither side of the keep-list, largest first:'
    if ($unknown.Count) {
        $unknown | Sort-Object Bytes -Descending | ForEach-Object {
            Write-Host ("  {0,10}  {1}" -f (Format-Size $_.Bytes), $_.Relative)
        }
    } else {
        Write-Host '  (none)'
    }

    Write-Host ''
    Write-Host 'Expected removals, largest first:'
    $expected | Sort-Object Bytes -Descending | Select-Object -First 40 | ForEach-Object {
        Write-Host ("  {0,10}  {1}" -f (Format-Size $_.Bytes), $_.Relative)
    }
    if ($expected.Count -gt 40) {
        Write-Host ("  ... and {0} more" -f ($expected.Count - 40))
    }

    Write-Host ''
    Write-Host 'Keep-list entries that matched nothing:'
    $unused = @()
    foreach ($i in 0..($patterns.Count - 1)) {
        $hits = @($kept | Where-Object { $_.Relative -like $matchers[$i] })
        if (-not $hits) { $unused += $patterns[$i] }
    }
    if ($unused) {
        # This is the half that catches a keep-list written from memory: a
        # pattern matching nothing is either a file that moved or a file that
        # was never there.
        $unused | ForEach-Object { Write-Host "  $_" }
    } else {
        Write-Host '  (none)'
    }

    Write-Host ''
    Write-Host 'Nothing was changed.'
    exit 0
}

# Exact bytes beside the rounded figure, so a note quoting it carries the
# number rather than one re-derived from a rounding.
Write-Host ("Before: {0,4} file(s)  {1}  ({2} bytes)" -f $files.Count, (Format-Size $total), $total)
Write-Host ("After:  {0,4} file(s)  {1}  ({2} bytes)" -f $kept.Count, (Format-Size $keptBytes), $keptBytes)
Write-Host ("Remove: {0,4} file(s)  {1}  ({2} bytes)" -f $doomed.Count, (Format-Size $doomedBytes), $doomedBytes)
if ($total -gt 0) {
    Write-Host ("Saving: {0:N1}%" -f (100.0 * $doomedBytes / $total))
}
Write-Host ''

Write-Host 'Files to remove:'
if ($doomed.Count) {
    $doomed | Sort-Object Relative | ForEach-Object {
        $mark = if ($unknown.Contains($_)) { '  UNRECOGNISED' } else { '' }
        Write-Host ("  {0,10}  {1}{2}" -f (Format-Size $_.Bytes), $_.Relative, $mark)
    }
} else {
    Write-Host '  (none)'
}
Write-Host ''

if ($unknown.Count) {
    # Refused in a dry run too, as a non-zero exit, so CI stops before building
    # an installer nobody should install.
    Write-Error ("{0} staged file(s), {1}, are on neither side of the keep-list (marked UNRECOGNISED above). Add each to the keep-list or to its expected removals, with a reason, before trimming." -f $unknown.Count, (Format-Size $unknownBytes))
    exit 1
}

if (-not $Apply) {
    Write-Host 'Dry run. Nothing was changed.'
    exit 0
}

if ($env:LIBOBS_TRIM -ne '1') {
    Write-Error 'LIBOBS_TRIM is not 1, so -Apply is refused. The trim is opt-in (#5); set it in this shell to mean it.'
    exit 1
}

foreach ($entry in $doomed) {
    Remove-Item -LiteralPath $entry.Full -Force
}

# Directories that held nothing but removed files are noise in the bundle and
# in the next inventory, so they go too. Deepest first, or a parent is never
# empty when it is looked at.
Get-ChildItem -LiteralPath $root -Recurse -Directory |
    Sort-Object { $_.FullName.Length } -Descending |
    ForEach-Object {
        if (-not (Get-ChildItem -LiteralPath $_.FullName -Recurse -File)) {
            Remove-Item -LiteralPath $_.FullName -Force -Recurse
        }
    }

$remaining = @(Get-ChildItem -LiteralPath $root -Recurse -File)
$remainingBytes = if ($remaining.Count) { [long](($remaining | Measure-Object -Property Length -Sum).Sum) } else { [long]0 }

Write-Host ("Removed {0} file(s), {1}." -f $doomed.Count, (Format-Size $doomedBytes))
Write-Host ("Staged libobs is now {0} file(s), {1} ({2} bytes)." -f $remaining.Count, (Format-Size $remainingBytes), $remainingBytes)
Write-Host ''
Write-Host 'Now the part this script cannot do (docs/windows-verification.md section 8):'
Write-Host '  1. Install this build, record a real game, and play it back.'
Write-Host '  2. Check libobs.log for failed module loads.'
Write-Host '  3. Measure the installed size with measure.ps1.'
Write-Host 'A wrongly removed plugin still builds, still packages, and then does not capture.'
