[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$OutputDir,

    [switch]$Sign,

    [switch]$SkipTimestamp
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Resolve-NsisCompiler {
    $command = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $candidate = Join-Path ([Environment]::GetFolderPath("ProgramFilesX86")) "NSIS\makensis.exe"
    if (Test-Path -LiteralPath $candidate) {
        return (Resolve-Path -LiteralPath $candidate).Path
    }

    throw "NSIS compiler (makensis.exe) was not found on this Windows host."
}

function Resolve-SignTool {
    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $candidate = Get-ChildItem -Path $kitsRoot -Filter signtool.exe -File -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
        Sort-Object FullName -Descending |
        Select-Object -First 1

    if (-not $candidate) {
        throw "signtool.exe was not found in the Windows SDK."
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

    $pfxPath = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-authenticode-" + [Guid]::NewGuid().ToString("N") + ".pfx")
    try {
        [IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($PfxBase64))
        $securePassword = ConvertTo-SecureString $Password -AsPlainText -Force
        $certificate = Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation "Cert:\CurrentUser\My" -Password $securePassword -Exportable:$false

        if (-not $certificate) {
            throw "The Authenticode certificate could not be imported."
        }
        return $certificate
    }
    finally {
        Remove-Item -LiteralPath $pfxPath -Force -ErrorAction SilentlyContinue
    }
}

$sourceExe = (Resolve-Path -LiteralPath $ExePath).Path
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Version '$Version' is not a supported semantic version."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$definition = Join-Path $repoRoot "installer\NeuralIA.nsi"
$icon = Join-Path $repoRoot "assets\logo.ico"
if (-not (Test-Path -LiteralPath $definition)) {
    throw "Installer definition not found: $definition"
}
if (-not (Test-Path -LiteralPath $icon)) {
    throw "Installer icon not found: $icon"
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$output = (Resolve-Path -LiteralPath $OutputDir).Path
$makensis = Resolve-NsisCompiler

& $makensis /V3 "/DAPP_VERSION=$Version" "/DSOURCE_EXE=$sourceExe" "/DOUTPUT_DIR=$output" "/DICON_PATH=$icon" $definition
if ($LASTEXITCODE -ne 0) {
    throw "NSIS failed with exit code $LASTEXITCODE."
}

$installer = Join-Path $output "NeuralIA-Setup-$Version-x64.exe"
if (-not (Test-Path -LiteralPath $installer)) {
    throw "NSIS completed but the expected installer was not produced: $installer"
}

if ($Sign) {
    $pfxBase64 = $env:NEURALIA_AUTHENTICODE_PFX_B64
    $pfxPassword = $env:NEURALIA_AUTHENTICODE_PFX_PASSWORD
    if ([string]::IsNullOrWhiteSpace($pfxBase64) -or [string]::IsNullOrWhiteSpace($pfxPassword)) {
        throw "Signing was requested but NEURALIA_AUTHENTICODE_PFX_B64 and NEURALIA_AUTHENTICODE_PFX_PASSWORD are not both configured."
    }

    $certificate = $null
    try {
        $certificate = Import-SigningCertificate -PfxBase64 $pfxBase64 -Password $pfxPassword
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

        & $signTool verify /pa /all /v $installer
        if ($LASTEXITCODE -ne 0) {
            throw "signtool could not verify the signed installer."
        }

        $signature = Get-AuthenticodeSignature -FilePath $installer
        if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
            throw "Authenticode verification returned status '$($signature.Status)'."
        }
    }
    finally {
        if ($certificate) {
            Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $certificate.Thumbprint) -Force -ErrorAction SilentlyContinue
        }
        $pfxBase64 = $null
        $pfxPassword = $null
    }
}

Write-Output (Resolve-Path -LiteralPath $installer).Path
