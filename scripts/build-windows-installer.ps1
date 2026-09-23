[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$OutputDir,

    [switch]$Sign,

    [switch]$SkipTimestamp,

    [switch]$AllowUntrustedTestCertificate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# Builds the NeuralIA installer: neural-setup (crates/neural-setup), the
# project's own installer with the brand and the animated neural tissue, with
# the exact -ExePath bytes embedded as its payload. The Inno Setup script that
# shipped 2.1.x is gone; neural-setup upgrades those installs in place.

function Get-WorkspaceVersion {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    # The version neural-setup writes to Windows "Apps" is its
    # CARGO_PKG_VERSION, i.e. [workspace.package] version.
    $manifest = Join-Path $RepoRoot "Cargo.toml"
    $section = ""
    foreach ($line in [IO.File]::ReadAllLines($manifest)) {
        if ($line -match '^\s*\[([^\]]+)\]\s*$') {
            $section = $Matches[1].Trim()
            continue
        }
        if ($section -eq "workspace.package" -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    throw "No [workspace.package] version found in $manifest."
}

function Get-CargoTargetDirectory {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    # CARGO_TARGET_DIR may redirect the build output; ask cargo instead of
    # assuming target/.
    Push-Location $RepoRoot
    try {
        $metadata = & cargo metadata --format-version 1 --no-deps --locked
        if ($LASTEXITCODE -ne 0) {
            throw "cargo metadata failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
    return ($metadata | Out-String | ConvertFrom-Json).target_directory
}

function Resolve-SignTool {
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $candidate = Get-ChildItem -Path $kitsRoot -Filter signtool.exe -File -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1

    if (-not $candidate) {
        throw "signtool.exe was not found in PATH or the Windows SDK."
    }

    return $candidate.FullName
}

function Import-SigningCertificate {
    param(
        [Parameter(Mandatory = $true)]
        [string]$PfxBase64,

        [Parameter(Mandatory = $true)]
        [string]$Password
    )

    $storePath = "Cert:\CurrentUser\My"
    $pfxPath = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-authenticode-" + [Guid]::NewGuid().ToString("N") + ".pfx")
    $bytes = $null
    $newThumbprints = @()
    $completed = $false
    try {
        $beforeThumbprints = @(
            Get-ChildItem -Path $storePath -ErrorAction Stop |
                Where-Object { $_.Thumbprint } |
                ForEach-Object { $_.Thumbprint.ToUpperInvariant() }
        )

        $bytes = [Convert]::FromBase64String($PfxBase64.Trim())
        [IO.File]::WriteAllBytes($pfxPath, $bytes)
        $securePassword = ConvertTo-SecureString $Password -AsPlainText -Force
        $imported = @(
            Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation $storePath -Password $securePassword -Exportable:$false
        )

        if ($imported.Count -eq 0) {
            throw "The Authenticode certificate could not be imported."
        }

        $signers = @($imported | Where-Object { $_.HasPrivateKey })
        if ($signers.Count -ne 1) {
            throw "The Authenticode PFX must contain exactly one certificate with a private key; found $($signers.Count)."
        }

        # Compare the store before/after instead of assuming Import-PfxCertificate
        # returns only the leaf. A PFX may carry intermediates, and cleanup must
        # remove only entries created by this invocation.
        $newThumbprints = @(
            Get-ChildItem -Path $storePath -ErrorAction Stop |
                Where-Object {
                    $_.Thumbprint -and
                    ($beforeThumbprints -notcontains $_.Thumbprint.ToUpperInvariant())
                } |
                ForEach-Object { $_.Thumbprint.ToUpperInvariant() } |
                Sort-Object -Unique
        )

        $completed = $true
        return [pscustomobject]@{
            Signer = $signers[0]
            NewThumbprints = @($newThumbprints)
        }
    }
    finally {
        # If import/validation fails, do not leak newly imported certificates.
        if (-not $completed) {
            foreach ($thumbprint in @($newThumbprints)) {
                Remove-Item -LiteralPath ("$storePath\" + $thumbprint) -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-Item -LiteralPath $pfxPath -Force -ErrorAction SilentlyContinue
        if ($bytes) {
            [Array]::Clear($bytes, 0, $bytes.Length)
        }
    }
}


if ($AllowUntrustedTestCertificate -and -not $Sign) {
    throw "-AllowUntrustedTestCertificate is valid only together with -Sign."
}
if ($AllowUntrustedTestCertificate -and -not $SkipTimestamp) {
    throw "-AllowUntrustedTestCertificate is test-only and requires -SkipTimestamp."
}

$sourceExe = (Resolve-Path -LiteralPath $ExePath).Path
if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
    throw "-ExePath '$ExePath' is not a file."
}
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Version '$Version' is not a supported semantic version."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$workspaceVersion = Get-WorkspaceVersion -RepoRoot $repoRoot
if ($Version -ne $workspaceVersion) {
    throw "Version '$Version' does not match the workspace version '$workspaceVersion'. neural-setup registers the workspace version in Windows Apps, so an installer named $Version would report $workspaceVersion."
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDir).Path
$installer = Join-Path $output "NeuralIA-Setup-$Version-x64.exe"
# A stale file with the final name must never pass for this build's output.
Remove-Item -LiteralPath $installer -Force -ErrorAction SilentlyContinue

$targetDirectory = Get-CargoTargetDirectory -RepoRoot $repoRoot
$builtSetup = Join-Path $targetDirectory "release\NeuralIA-Setup.exe"

# The payload is staged in a fresh directory that holds exactly one file: the
# -ExePath bytes, named NeuralIA.exe. NeuralIA.exe is self-contained (the
# WebView2 loader is linked statically; PDF.js, Live and the Home art are
# embedded), so nothing else is needed at runtime besides the system WebView2
# runtime -- the same single file the Inno installer shipped.
$payloadDir = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-payload-" + [Guid]::NewGuid().ToString("N"))
$previousPayloadDir = $env:NEURALIA_PAYLOAD_DIR
New-Item -ItemType Directory -Force -Path $payloadDir | Out-Null
try {
    $stagedExe = Join-Path $payloadDir "NeuralIA.exe"
    [IO.File]::Copy($sourceExe, $stagedExe, $true)
    $sourceHash = (Get-FileHash -LiteralPath $sourceExe -Algorithm SHA256).Hash
    $stagedHash = (Get-FileHash -LiteralPath $stagedExe -Algorithm SHA256).Hash
    if ($stagedHash -ne $sourceHash) {
        throw "Staged NeuralIA.exe ($stagedHash) differs from -ExePath ($sourceHash)."
    }
    Write-Host "Installer payload: NeuralIA.exe SHA256 $sourceHash"

    # Deleting the previous output makes a silent no-op build impossible to
    # mistake for a fresh one: cargo either writes it again or the check below
    # fails.
    Remove-Item -LiteralPath $builtSetup -Force -ErrorAction SilentlyContinue

    $env:NEURALIA_PAYLOAD_DIR = $payloadDir
    Push-Location $repoRoot
    try {
        & cargo build --release --locked -p neural-setup
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build -p neural-setup failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
}
finally {
    $env:NEURALIA_PAYLOAD_DIR = $previousPayloadDir
    Remove-Item -LiteralPath $payloadDir -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not (Test-Path -LiteralPath $builtSetup -PathType Leaf)) {
    throw "cargo completed but $builtSetup was not produced."
}
Copy-Item -LiteralPath $builtSetup -Destination $installer -Force

if ($Sign) {
    $pfxBase64 = $env:NEURALIA_AUTHENTICODE_PFX_B64
    $pfxPassword = $env:NEURALIA_AUTHENTICODE_PFX_PASSWORD
    if ([string]::IsNullOrWhiteSpace($pfxBase64) -or [string]::IsNullOrWhiteSpace($pfxPassword)) {
        throw "Signing was requested but NEURALIA_AUTHENTICODE_PFX_B64 and NEURALIA_AUTHENTICODE_PFX_PASSWORD are not both configured."
    }

    $certificateImport = $null
    $certificate = $null
    try {
        $certificateImport = Import-SigningCertificate -PfxBase64 $pfxBase64 -Password $pfxPassword
        $certificate = $certificateImport.Signer
        $signTool = Resolve-SignTool

        if ($SkipTimestamp) {
            & $signTool sign /sha1 $certificate.Thumbprint /s My /fd SHA256 $installer
        }
        else {
            $timestampUrl = "https://timestamp.digicert.com"
            & $signTool sign /sha1 $certificate.Thumbprint /s My /fd SHA256 /tr $timestampUrl /td SHA256 $installer
        }
        if ($LASTEXITCODE -ne 0) {
            throw "signtool failed to sign the installer."
        }

        if (-not $AllowUntrustedTestCertificate) {
            & $signTool verify /pa /all /v $installer
            if ($LASTEXITCODE -ne 0) {
                throw "signtool could not verify the signed installer."
            }
        }

        $verifyParams = @{
            InstallerPath = $installer
            ExpectedThumbprint = $certificate.Thumbprint
        }
        if (-not $SkipTimestamp) {
            $verifyParams.RequireTimestamp = $true
        }
        if ($AllowUntrustedTestCertificate) {
            $verifyParams.AllowUntrustedTestCertificate = $true
        }
        & "$PSScriptRoot/verify-windows-installer-signature.ps1" @verifyParams
    }
    finally {
        if ($certificateImport) {
            foreach ($thumbprint in @($certificateImport.NewThumbprints)) {
                Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $thumbprint) -Force -ErrorAction SilentlyContinue
            }
        }
        $certificate = $null
        $certificateImport = $null
        $pfxBase64 = $null
        $pfxPassword = $null
    }
}

Write-Output (Resolve-Path -LiteralPath $installer).Path
