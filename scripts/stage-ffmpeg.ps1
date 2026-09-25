<#
.SYNOPSIS
    Stages the pinned ffmpeg build, and the licence texts that ship with it.

.DESCRIPTION
    The bundled ffmpeg is BtbN's LGPL static win64 build (docs/licensing.md
    section 3). This script is the only thing that fetches it, and
    scripts/ffmpeg-pin.json is the only place its version is written down:
    bumping it is an edit to that file and nothing else.

    It downloads the pinned asset from the pinned BtbN release, and refuses to
    go on unless the archive's SHA-256 is the one in the pin. Then it stages
    these files side by side in the destination folder:

      ffmpeg.exe                 the binary, from the archive's bin/
      FFMPEG-LICENSE.txt         the archive's own LICENSE.txt, which is the
                                 LGPLv3 (checked byte for byte against
                                 FFmpeg's COPYING.LGPLv3 at the pinned commit)
      FFMPEG-COPYING.GPLv3.txt   the GPLv3 the LGPLv3 incorporates by
                                 reference, from FFmpeg's source at the pinned
                                 commit (the archive does not carry it)
      FFMPEG-LICENSE.md          FFmpeg's own licensing statement, from the
                                 same commit
      FFMPEG-SOURCE.txt          which FFmpeg, built by which BtbN scripts,
                                 and where the corresponding source is

    -Verify checks an already-staged folder instead, which is what CI runs on
    a cache hit: every file is present, FFMPEG-SOURCE.txt describes this pin
    and not an older one, and on Windows `ffmpeg -version` names the pinned
    version and an LGPLv3 configuration.

.NOTES
    Deliberately ASCII-only, like measure.ps1 and trim-libobs.ps1. Windows
    PowerShell 5.1 reads a .ps1 as the ANSI code page unless the file carries
    a UTF-8 BOM.

    The licence texts are fetched by full commit hash, which names exactly one
    tree, so the URL is as fixed as the archive's checksum. The GPLv3 text is
    also checked against a constant below: it has not changed since 2007, so
    it does not move when the pin does.

.PARAMETER Pin
    The pin file. Defaults to scripts/ffmpeg-pin.json beside this script.

.PARAMETER Destination
    Where to stage. Defaults to src-tauri/target/libobs, the folder
    tauri.windows.conf.json bundles as `libobs\` and daemon::ffmpeg() and
    lib.rs::ffmpeg_path resolve ffmpeg.exe from.

.PARAMETER Verify
    Check the destination and change nothing.

.EXAMPLE
    ./scripts/stage-ffmpeg.ps1

.EXAMPLE
    ./scripts/stage-ffmpeg.ps1 -Verify
#>

[CmdletBinding()]
param(
    [string] $Pin = (Join-Path $PSScriptRoot 'ffmpeg-pin.json'),
    [string] $Destination = (Join-Path $PSScriptRoot '../src-tauri/target/libobs'),
    [switch] $Verify
)

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest's progress bar costs more than the download on a runner.
$ProgressPreference = 'SilentlyContinue'

# The GPLv3 text, as FFmpeg's COPYING.GPLv3 carries it.
$GplV3Sha256 = '8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903'

$StagedFiles = @(
    'ffmpeg.exe',
    'FFMPEG-LICENSE.txt',
    'FFMPEG-COPYING.GPLv3.txt',
    'FFMPEG-LICENSE.md',
    'FFMPEG-SOURCE.txt'
)

function Read-Pin {
    param([string] $Path)
    $json = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    foreach ($field in 'btbnTag', 'btbnCommit', 'asset', 'sha256', 'version', 'branch', 'ffmpegCommit') {
        $value = $json.PSObject.Properties[$field]
        if (-not $value -or -not $value.Value) { throw "$Path has no '$field'" }
    }
    foreach ($field in 'btbnCommit', 'ffmpegCommit') {
        if ($json.$field -notmatch '^[0-9a-f]{40}$') { throw "$Path '$field' must be a full 40-character commit hash" }
    }
    if ($json.sha256 -notmatch '^[0-9a-f]{64}$') { throw "$Path 'sha256' must be 64 lowercase hex characters" }
    # The one property of the build the licence analysis depends on: an LGPL
    # variant, never the GPL one, which carries x264 and x265.
    if ($json.asset -notmatch '-win64-lgpl(-[0-9.]+)?\.zip$') {
        throw "$Path 'asset' is not a static win64 LGPL build: $($json.asset)"
    }
    return $json
}

function Get-Sha256 {
    param([string] $Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-SourceText {
    param($P, [string] $ExeSha256)
    $release = "https://github.com/BtbN/FFmpeg-Builds/releases/tag/$($P.btbnTag)"
    # CRLF, because the only people who will read this file are on Windows.
    return @(
        'ffmpeg.exe in this folder is FFmpeg, licensed under the GNU Lesser General'
        'Public License, version 3 or later (LGPL-3.0-or-later). It is an unmodified'
        'build that ninja-recorder runs as a separate program; ninja-recorder does'
        'not link to it and never modifies it.'
        ''
        "FFmpeg version      $($P.version) (branch $($P.branch))"
        "FFmpeg commit       $($P.ffmpegCommit)"
        "Built by            BtbN/FFmpeg-Builds, release $($P.btbnTag)"
        "                    $release"
        "Build scripts       BtbN/FFmpeg-Builds commit $($P.btbnCommit)"
        "Archive             $($P.asset)"
        "Archive SHA-256     $($P.sha256)"
        "ffmpeg.exe SHA-256  $ExeSha256"
        ''
        'Corresponding source'
        ''
        '  FFmpeg, at the commit above:'
        "    https://git.ffmpeg.org/gitweb/ffmpeg.git/commit/$($P.ffmpegCommit)"
        "    https://github.com/FFmpeg/FFmpeg/tree/$($P.ffmpegCommit)"
        ''
        '  The scripts that configured and built it, including the version of'
        '  every library linked into this static build:'
        "    https://github.com/BtbN/FFmpeg-Builds/tree/$($P.btbnCommit)"
        ''
        'Licence texts beside this file'
        ''
        '  FFMPEG-LICENSE.txt        the GNU LGPL version 3, as shipped in the archive'
        '  FFMPEG-COPYING.GPLv3.txt  the GNU GPL version 3, which the LGPL incorporates'
        '  FFMPEG-LICENSE.md         FFmpeg''s own statement of which licence applies'
        ''
    ) -join "`r`n"
}

function Test-Staged {
    param($P, [string] $Dir)
    foreach ($name in $StagedFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $Dir $name))) {
            throw "$name is missing from $Dir"
        }
    }

    $exe = Join-Path $Dir 'ffmpeg.exe'
    $expected = Get-SourceText $P (Get-Sha256 $exe)
    $actual = Get-Content -LiteralPath (Join-Path $Dir 'FFMPEG-SOURCE.txt') -Raw
    if ($actual -ne $expected) {
        throw "FFMPEG-SOURCE.txt in $Dir does not describe the pinned build (or ffmpeg.exe is not the one it names). Re-stage it."
    }

    if ((Get-Sha256 (Join-Path $Dir 'FFMPEG-COPYING.GPLv3.txt')) -ne $GplV3Sha256) {
        throw 'FFMPEG-COPYING.GPLv3.txt is not the GPLv3 text'
    }

    # Only a Windows runner can execute it. Everywhere else the checksum above
    # is what ties the binary to the pin.
    if ($IsWindows -or $env:OS -eq 'Windows_NT') {
        $banner = & $exe -version | Out-String
        if ($LASTEXITCODE -ne 0) { throw "ffmpeg.exe -version exited $LASTEXITCODE" }
        $first = ($banner -split "`n")[0].Trim()
        if ($first -notlike "ffmpeg version $($P.version)-*") {
            throw "ffmpeg.exe reports '$first', not version $($P.version)"
        }
        if ($banner -notmatch '--enable-version3') {
            throw 'ffmpeg.exe was not configured with --enable-version3, so it is not the LGPLv3 build'
        }
        foreach ($flag in '--enable-gpl', '--enable-nonfree') {
            if ($banner -match [regex]::Escape($flag)) {
                throw "ffmpeg.exe was configured with $flag; only the LGPL build may ship"
            }
        }
        Write-Host $first
    }
    Write-Host "staged ffmpeg $($P.version) and its licence texts in $Dir"
}

$p = Read-Pin $Pin

if ($Verify) {
    Test-Staged $p $Destination
    exit 0
}

$work = Join-Path ([IO.Path]::GetTempPath()) "ninja-ffmpeg-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
    $zip = Join-Path $work $p.asset
    $url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$($p.btbnTag)/$($p.asset)"
    Write-Host "downloading $url"
    Invoke-WebRequest -Uri $url -OutFile $zip

    $got = Get-Sha256 $zip
    if ($got -ne $p.sha256) {
        throw "SHA-256 mismatch for $($p.asset): expected $($p.sha256), got $got. Refusing to stage it."
    }
    Write-Host "SHA-256 matches the pin: $got"

    $unpacked = Join-Path $work 'unpacked'
    Expand-Archive -LiteralPath $zip -DestinationPath $unpacked
    $top = Join-Path $unpacked ([IO.Path]::GetFileNameWithoutExtension($p.asset))
    $exe = Join-Path $top 'bin/ffmpeg.exe'
    $licence = Join-Path $top 'LICENSE.txt'
    foreach ($f in $exe, $licence) {
        if (-not (Test-Path -LiteralPath $f)) { throw "$f is not in the archive; its layout has changed" }
    }

    $raw = "https://raw.githubusercontent.com/FFmpeg/FFmpeg/$($p.ffmpegCommit)"
    $lgpl = Join-Path $work 'COPYING.LGPLv3'
    $gpl = Join-Path $work 'COPYING.GPLv3'
    $statement = Join-Path $work 'LICENSE.md'
    Invoke-WebRequest -Uri "$raw/COPYING.LGPLv3" -OutFile $lgpl
    Invoke-WebRequest -Uri "$raw/COPYING.GPLv3" -OutFile $gpl
    Invoke-WebRequest -Uri "$raw/LICENSE.md" -OutFile $statement

    # The archive's LICENSE.txt is whatever BtbN's variant names as the licence
    # file. For the LGPL variant that is COPYING.LGPLv3; anything else means
    # the variant, or its licence, is not what docs/licensing.md says it is.
    if ((Get-Sha256 $licence) -ne (Get-Sha256 $lgpl)) {
        throw "the archive's LICENSE.txt is not FFmpeg's COPYING.LGPLv3 at $($p.ffmpegCommit)"
    }
    if ((Get-Sha256 $gpl) -ne $GplV3Sha256) {
        throw "COPYING.GPLv3 at $($p.ffmpegCommit) is not the GPLv3 text this script expects"
    }

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Copy-Item -LiteralPath $exe -Destination (Join-Path $Destination 'ffmpeg.exe') -Force
    Copy-Item -LiteralPath $licence -Destination (Join-Path $Destination 'FFMPEG-LICENSE.txt') -Force
    Copy-Item -LiteralPath $gpl -Destination (Join-Path $Destination 'FFMPEG-COPYING.GPLv3.txt') -Force
    Copy-Item -LiteralPath $statement -Destination (Join-Path $Destination 'FFMPEG-LICENSE.md') -Force

    $text = Get-SourceText $p (Get-Sha256 (Join-Path $Destination 'ffmpeg.exe'))
    [IO.File]::WriteAllText((Join-Path $Destination 'FFMPEG-SOURCE.txt'), $text, [Text.Encoding]::ASCII)

    Test-Staged $p $Destination
}
finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
