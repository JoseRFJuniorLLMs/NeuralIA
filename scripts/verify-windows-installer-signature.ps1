[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [string]$ExpectedThumbprint = "",

    [switch]$RequireTimestamp,

    [switch]$AllowUntrustedTestCertificate
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$resolved = (Resolve-Path -LiteralPath $InstallerPath -ErrorAction Stop).Path
$signature = Get-AuthenticodeSignature -FilePath $resolved

if (-not $signature.SignerCertificate) {
    throw "Authenticode verification found no signer certificate (status '$($signature.Status)')."
}

if (-not [string]::IsNullOrWhiteSpace($ExpectedThumbprint)) {
    $expected = ($ExpectedThumbprint -replace '\s', '').ToUpperInvariant()
    $actual = ($signature.SignerCertificate.Thumbprint -replace '\s', '').ToUpperInvariant()
    if ($actual -ne $expected) {
        throw "Authenticode signer thumbprint mismatch. Expected '$expected', got '$actual'."
    }
}

if ($RequireTimestamp -and -not $signature.TimeStamperCertificate) {
    throw "Authenticode signature has no timestamp certificate."
}

if ($signature.Status -eq [System.Management.Automation.SignatureStatus]::Valid) {
    Write-Host "Authenticode verification passed: trusted signature."
    return
}

if ($AllowUntrustedTestCertificate) {
    $statusText = $signature.Status.ToString()
    $message = [string]$signature.StatusMessage
    $isTrustOnlyFailure =
        $statusText -in @("NotTrusted", "UnknownError") -and
        $message -match '(?i)(not trusted|untrusted|root certificate|trust provider)'

    if ($isTrustOnlyFailure) {
        Write-Host "Authenticode cryptographic verification passed with the expected untrusted CI certificate."
        Write-Host "  Status:  $statusText"
        Write-Host "  Signer:  $($signature.SignerCertificate.Subject)"
        return
    }
}

throw "Authenticode verification failed: $($signature.Status) $($signature.StatusMessage)"
