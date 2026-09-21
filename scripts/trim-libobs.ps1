<#
.SYNOPSIS
    Trims a staged libobs directory to a keep-list, or inventories one.

.DESCRIPTION
    WS1 task 1.1, the P0a arm (#5). The trimmed backend is the fallback if the
    P0c spikes fail, and the selectable second backend for exactly one release
    if they pass (WS1.7). Either way its size is the thing being measured:
    libobs is a large share of a 248 MB install.

    Two modes, and the order matters.

    -Inventory prints what is actually staged, grouped, with sizes, and marks
    each entry against the keep-list. Run this FIRST. The keep-list shipped
    with this script was written from what the recorder demonstrably uses
    rather than from a real directory listing, because no staged directory
    exists off Windows, and a keep-list that has never been compared to reality
    is a guess.

    Without -Apply the script is a dry run: it prints what it would remove and
    the size that would be saved, and touches nothing. That is the default
    because the failure mode here is not a broken build. A plugin removed
    wrongly still compiles, still packages, still installs, and then does not
    capture, which is why the exit criterion for this task is a clean
    plugin-load log and a recording that plays rather than a green build.

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
    Actually delete. Without it the script reports and exits.

.EXAMPLE
    ./scripts/trim-libobs.ps1 -Inventory

    What is there, what the keep-list covers, and what it does not recognise.

.EXAMPLE
    ./scripts/trim-libobs.ps1

    A dry run: what would go, and how many bytes that is.

.EXAMPLE
    $env:LIBOBS_TRIM = '1'; ./scripts/trim-libobs.ps1 -Apply

    The real thing. Record the before and after sizes in DEVELOPMENT.md
    section 16's measurement table, then package and play a game.
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

$patterns = Get-Content -LiteralPath $KeepList |
    ForEach-Object { $_.Trim() } |
    Where-Object { $_ -and -not $_.StartsWith('#') }

if (-not $patterns) {
    Write-Error "The keep-list at $KeepList has no patterns. That would remove everything; refusing."
    exit 1
}

# `**` means "any depth", which -like does not know about, so it becomes `*`.
# Everything else is a plain wildcard match on a forward-slashed relative path.
$matchers = $patterns | ForEach-Object { $_.Replace('**', '*') }

$files = Get-ChildItem -LiteralPath $root -Recurse -File

$kept = New-Object System.Collections.Generic.List[object]
$doomed = New-Object System.Collections.Generic.List[object]

foreach ($file in $files) {
    $relative = $file.FullName.Substring($root.Length).TrimStart('\', '/').Replace('\', '/')
    $isKept = $false
    foreach ($matcher in $matchers) {
        if ($relative -like $matcher) { $isKept = $true; break }
    }
    $entry = [pscustomobject]@{
        Relative = $relative
        Bytes    = $file.Length
        Full     = $file.FullName
    }
    if ($isKept) { $kept.Add($entry) } else { $doomed.Add($entry) }
}

$total = ($files | Measure-Object -Property Length -Sum).Sum
$keptBytes = if ($kept.Count) { ($kept | Measure-Object -Property Bytes -Sum).Sum } else { 0 }
$doomedBytes = if ($doomed.Count) { ($doomed | Measure-Object -Property Bytes -Sum).Sum } else { 0 }

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
        Group-Object { $d = Split-Path $_.FullName -Parent; $d.Substring($root.Length).TrimStart('\', '/').Replace('\', '/') } |
        Sort-Object { ($_.Group | Measure-Object -Property Length -Sum).Sum } -Descending |
        ForEach-Object {
            $where = if ($_.Name) { $_.Name } else { '(root)' }
            $sum = ($_.Group | Measure-Object -Property Length -Sum).Sum
            Write-Host ("  {0,10}  {1,4} file(s)  {2}" -f (Format-Size $sum), $_.Count, $where)
        }

    Write-Host ''
    Write-Host 'Not matched by the keep-list, largest first:'
    $doomed | Sort-Object Bytes -Descending | Select-Object -First 40 | ForEach-Object {
        Write-Host ("  {0,10}  {1}" -f (Format-Size $_.Bytes), $_.Relative)
    }
    if ($doomed.Count -gt 40) {
        Write-Host ("  ... and {0} more" -f ($doomed.Count - 40))
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
    Write-Host 'Nothing was changed. Compare the two lists above against the keep-list before running without -Inventory.'
    exit 0
}

Write-Host ("Keep:   {0,4} file(s)  {1}" -f $kept.Count, (Format-Size $keptBytes))
Write-Host ("Remove: {0,4} file(s)  {1}" -f $doomed.Count, (Format-Size $doomedBytes))
Write-Host ("After:  {0}" -f (Format-Size $keptBytes))
if ($total -gt 0) {
    Write-Host ("Saving: {0:N1}%" -f (100.0 * $doomedBytes / $total))
}
Write-Host ''

if (-not $Apply) {
    Write-Host 'Dry run. Nothing was changed.'
    Write-Host 'Run -Inventory first if this keep-list has not been compared against a real staged directory, then re-run with -Apply.'
    exit 0
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

Write-Host ("Removed {0} file(s), {1}." -f $doomed.Count, (Format-Size $doomedBytes))
Write-Host ''
Write-Host 'Now the part this script cannot do:'
Write-Host '  1. Package, install, and play a game.'
Write-Host '  2. Check the plugin-load log is clean (no failed module loads).'
Write-Host '  3. Record the before and after sizes in DEVELOPMENT.md section 16.'
Write-Host 'A wrongly removed plugin still builds, still packages, and then does not capture.'
