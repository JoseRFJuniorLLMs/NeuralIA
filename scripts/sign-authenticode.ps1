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

$flags = [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
try {
    $cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
        $pfxBytes,
        $PfxPassword,
        $flags
    )
} catch {
    throw "Could not open the Authenticode PFX: $($_.Exception.Message)"
}

try {
    if (-not $cert.HasPrivateKey) {
        throw "The Authenticode certificate does not contain a private key."
    }
    if (-not [string]::IsNullOrWhiteSpace($ExpectedSubject) -and $cert.Subject -ne $ExpectedSubject) {
        throw "Authenticode certificate subject mismatch. Expected '$ExpectedSubject', got '$($cert.Subject)'."
    }

    $params = @{
        FilePath = $resolved
        Certificate = $cert
        HashAlgorithm = "SHA256"
    }
    if (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) {
        $params.TimestampServer = $TimestampUrl
    }

    $signature = Set-AuthenticodeSignature @params
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Authenticode signing did not produce a valid signature: $($signature.Status) $($signature.StatusMessage)"
    }

    Write-Host "Authenticode signature written."
    Write-Host "  Subject:    $($signature.SignerCertificate.Subject)"
    Write-Host "  Thumbprint: $($signature.SignerCertificate.Thumbprint)"
    if ($signature.TimeStamperCertificate) {
        Write-Host "  Timestamp:  $($signature.TimeStamperCertificate.Subject)"
    } elseif (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) {
        throw "The file was signed but no RFC3161/Authenticode timestamp certificate was recorded."
    }
} finally {
    $cert.Dispose()
    [Array]::Clear($pfxBytes, 0, $pfxBytes.Length)
}
