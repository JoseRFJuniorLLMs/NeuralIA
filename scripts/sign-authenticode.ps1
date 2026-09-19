param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,

    [Parameter(Mandatory = $true)]
    [string]$PfxBase64,

    [Parameter(Mandatory = $true)]
    [string]$PfxPassword,

    [string]$TimestampUrl = "http://timestamp.digicert.com",

    [string]$ExpectedSubject = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Find-SignTool {
    $command = Get-Command "signtool.exe" -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $kits = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kits) {
        $candidate = Get-ChildItem -Path $kits -Filter "signtool.exe" -Recurse -File -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($candidate) {
            return $candidate.FullName
        }
    }

    throw "signtool.exe was not found in PATH or the Windows SDK."
}

$resolved = (Resolve-Path -LiteralPath $ExePath -ErrorAction Stop).Path

if ([string]::IsNullOrWhiteSpace($PfxBase64)) {
    throw "Authenticode signing is enabled but NEURALIA_AUTHENTICODE_PFX_B64 is empty."
}
if ([string]::IsNullOrWhiteSpace($PfxPassword)) {
    throw "Authenticode signing is enabled but NEURALIA_AUTHENTICODE_PFX_PASSWORD is empty."
}

try {
    $pfxBytes = [Convert]::FromBase64String($PfxBase64.Trim())
} catch {
    throw "NEURALIA_AUTHENTICODE_PFX_B64 is not valid base64: $($_.Exception.Message)"
}

$tempPfx = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-authenticode-" + [Guid]::NewGuid().ToString("N") + ".pfx")
$importedByThisRun = $false
$thumbprint = $null

try {
    [IO.File]::WriteAllBytes($tempPfx, $pfxBytes)
    $password = ConvertTo-SecureString -String $PfxPassword -AsPlainText -Force

    try {
        $probe = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
            $pfxBytes,
            $PfxPassword,
            [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::DefaultKeySet
        )
    } catch {
        throw "Could not open the Authenticode PFX: $($_.Exception.Message)"
    }

    try {
        if (-not $probe.HasPrivateKey) {
            throw "The Authenticode certificate does not contain a private key."
        }
        if (-not [string]::IsNullOrWhiteSpace($ExpectedSubject) -and $probe.Subject -ne $ExpectedSubject) {
            throw "Authenticode certificate subject mismatch. Expected '$ExpectedSubject', got '$($probe.Subject)'."
        }
        $thumbprint = $probe.Thumbprint
    } finally {
        $probe.Dispose()
    }

    $storePath = "Cert:\CurrentUser\My\$thumbprint"
    if (-not (Test-Path -LiteralPath $storePath)) {
        Import-PfxCertificate -FilePath $tempPfx -CertStoreLocation "Cert:\CurrentUser\My" -Password $password |
            Out-Null
        $importedByThisRun = $true
    }

    $signTool = Find-SignTool
    $args = @(
        "sign",
        "/fd", "SHA256",
        "/sha1", $thumbprint,
        "/s", "My"
    )
    if (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) {
        $args += @("/tr", $TimestampUrl, "/td", "SHA256")
    }
    $args += $resolved

    & $signTool @args
    if ($LASTEXITCODE -ne 0) {
        throw "signtool.exe sign failed with exit code $LASTEXITCODE."
    }

    & $signTool verify /pa /v $resolved
    if ($LASTEXITCODE -ne 0) {
        throw "signtool.exe verify failed with exit code $LASTEXITCODE."
    }

    $signature = Get-AuthenticodeSignature -FilePath $resolved
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Authenticode signing did not produce a valid signature: $($signature.Status) $($signature.StatusMessage)"
    }
    if (-not [string]::IsNullOrWhiteSpace($ExpectedSubject) -and $signature.SignerCertificate.Subject -ne $ExpectedSubject) {
        throw "Authenticode signer subject mismatch after signing."
    }
    if (-not [string]::IsNullOrWhiteSpace($TimestampUrl) -and -not $signature.TimeStamperCertificate) {
        throw "The file was signed but no timestamp certificate was recorded."
    }

    Write-Host "Authenticode signature written and verified."
    Write-Host "  Subject:    $($signature.SignerCertificate.Subject)"
    Write-Host "  Thumbprint: $($signature.SignerCertificate.Thumbprint)"
    if ($signature.TimeStamperCertificate) {
        Write-Host "  Timestamp:  $($signature.TimeStamperCertificate.Subject)"
    }
} finally {
    if ($importedByThisRun -and $thumbprint) {
        Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $thumbprint) -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $tempPfx -Force -ErrorAction SilentlyContinue
    [Array]::Clear($pfxBytes, 0, $pfxBytes.Length)
}
