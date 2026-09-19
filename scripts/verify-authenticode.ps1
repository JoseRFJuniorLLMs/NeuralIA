param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,

    [string]$ExpectedSubject = "",

    [switch]$RequireTimestamp
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$resolved = (Resolve-Path -LiteralPath $ExePath -ErrorAction Stop).Path
$signature = Get-AuthenticodeSignature -FilePath $resolved

if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
    throw "Authenticode verification failed: $($signature.Status) $($signature.StatusMessage)"
}
if (-not $signature.SignerCertificate) {
    throw "Authenticode verification returned Valid without a signer certificate."
}
if (-not [string]::IsNullOrWhiteSpace($ExpectedSubject) -and $signature.SignerCertificate.Subject -ne $ExpectedSubject) {
    throw "Authenticode signer subject mismatch. Expected '$ExpectedSubject', got '$($signature.SignerCertificate.Subject)'."
}
if ($RequireTimestamp -and -not $signature.TimeStamperCertificate) {
    throw "Authenticode signature is valid but has no timestamp certificate."
}

Write-Host "Authenticode verification passed."
Write-Host "  Subject:    $($signature.SignerCertificate.Subject)"
Write-Host "  Thumbprint: $($signature.SignerCertificate.Thumbprint)"
if ($signature.TimeStamperCertificate) {
    Write-Host "  Timestamp:  $($signature.TimeStamperCertificate.Subject)"
}
