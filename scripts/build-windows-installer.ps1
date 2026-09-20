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

function Resolve-InnoCompiler {
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $candidates = @(
        (Join-Path ([Environment]::GetFolderPath("ProgramFilesX86")) "Inno Setup 6\ISCC.exe"),
        (Join-Path ([Environment]::GetFolderPath("ProgramFiles")) "Inno Setup 6\ISCC.exe")
    )

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    throw "Inno Setup compiler (ISCC.exe) was not found on this Windows host."
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
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Version '$Version' is not a supported semantic version."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$definition = Join-Path $repoRoot "installer\NeuralIA.iss"
$icon = Join-Path $repoRoot "assets\logo.ico"
if (-not (Test-Path -LiteralPath $definition)) {
    throw "Installer definition not found: $definition"
}
if (-not (Test-Path -LiteralPath $icon)) {
    throw "Installer icon not found: $icon"
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDir).Path
$iscc = Resolve-InnoCompiler

$arguments = @(
    "/Qp",
    "/DAppVersion=$Version",
    "/DSourceExe=$sourceExe",
    "/DOutputDir=$output",
    "/DIconPath=$icon",
    $definition
)
& $iscc @arguments
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup failed with exit code $LASTEXITCODE."
}

$installer = Join-Path $output "NeuralIA-Setup-$Version-x64.exe"
if (-not (Test-Path -LiteralPath $installer)) {
    throw "Inno Setup completed but the expected installer was not produced: $installer"
}

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
