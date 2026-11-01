[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$OutputDir,
    [string]$ArchivePath = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# These are reviewed together. A moved/mutable upstream package fails closed
# instead of silently changing the encoder shipped inside the installer.
$encoderVersion = "9.0.2"
$archiveName = "ffmpeg-9.0.2-essentials_build.zip"
$archiveUrl = "https://www.gyan.dev/ffmpeg/builds/packages/$archiveName"
$archiveSha256 = "60F467265B1E312373DBCD92200C2618A74850F98D3D078E94296BB3FA2047BA"
$sourceUrl = "https://github.com/FFmpeg/FFmpeg/commit/946fcce07b"
$buildUrl = "https://www.gyan.dev/ffmpeg/builds/"

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDir).Path
if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
    $ArchivePath = Join-Path $output $archiveName
    if (-not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) {
        Invoke-WebRequest -Uri $archiveUrl -OutFile $ArchivePath -UseBasicParsing
    }
}
$archive = (Resolve-Path -LiteralPath $ArchivePath).Path
$actualHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
if ($actualHash -ne $archiveSha256) {
    throw "FFmpeg archive SHA256 mismatch: expected $archiveSha256, got $actualHash. No files were extracted."
}

$extract = Join-Path $output ("extract-" + [Guid]::NewGuid().ToString("N"))
$prepared = Join-Path $output "ffmpeg-$encoderVersion"
try {
    Expand-Archive -LiteralPath $archive -DestinationPath $extract
    $root = Join-Path $extract "ffmpeg-9.0.2-essentials_build"
    foreach ($relative in @("bin/ffmpeg.exe", "LICENSE", "README.txt")) {
        if (-not (Test-Path -LiteralPath (Join-Path $root $relative) -PathType Leaf)) {
            throw "The pinned FFmpeg package is missing $relative."
        }
    }
    $readme = Get-Content -LiteralPath (Join-Path $root "README.txt") -Raw
    if ($readme -notmatch 'Version: 9\.0\.2-essentials_build' -or $readme -notmatch 'License: GPL v3') {
        throw "The pinned FFmpeg package identity/license is unexpected."
    }
    New-Item -ItemType Directory -Force -Path $prepared | Out-Null
    Copy-Item -LiteralPath (Join-Path $root "bin/ffmpeg.exe") -Destination (Join-Path $prepared "ffmpeg.exe") -Force
    Copy-Item -LiteralPath (Join-Path $root "LICENSE") -Destination (Join-Path $prepared "LICENSE.txt") -Force
    Copy-Item -LiteralPath (Join-Path $root "README.txt") -Destination (Join-Path $prepared "README.txt") -Force
    $provenance = [ordered]@{
        name = "FFmpeg"
        version = $encoderVersion
        license = "GPL-3.0-or-later"
        archive_url = $archiveUrl
        archive_sha256 = $archiveSha256.ToLowerInvariant()
        ffmpeg_sha256 = (Get-FileHash -LiteralPath (Join-Path $prepared "ffmpeg.exe") -Algorithm SHA256).Hash.ToLowerInvariant()
        source_url = $sourceUrl
        build_url = $buildUrl
    }
    $provenance | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $prepared "PROVENANCE.json") -Encoding UTF8
    @"
FFmpeg $encoderVersion is distributed as a separate command-line program.
Its license is included in LICENSE.txt. The upstream build configuration and
external library versions are included in README.txt.
FFmpeg source revision: $sourceUrl
Source archive: https://github.com/FFmpeg/FFmpeg/archive/946fcce07b.tar.gz
Build supplier and external-library information: $buildUrl
The unmodified binary package is pinned by SHA-256 in PROVENANCE.json.
"@ | Set-Content -LiteralPath (Join-Path $prepared "SOURCE_CODE.txt") -Encoding UTF8
    Write-Output $prepared
}
finally {
    # Delete only the fresh child we created under the explicit output root.
    $resolvedExtract = [IO.Path]::GetFullPath($extract)
    $prefix = [IO.Path]::GetFullPath($output).TrimEnd('\') + '\'
    if (-not $resolvedExtract.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove a directory outside the encoder output root."
    }
    Remove-Item -LiteralPath $resolvedExtract -Recurse -Force -ErrorAction SilentlyContinue
}
