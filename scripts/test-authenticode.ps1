param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$source = (Resolve-Path -LiteralPath $ExePath -ErrorAction Stop).Path
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-authenticode-test-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempDir | Out-Null

$subject = "CN=NeuralIA CI Authenticode Test"
$cert = $null
$trusted = $null

try {
    $copy = Join-Path $tempDir "NeuralIA-test.exe"
    Copy-Item -LiteralPath $source -Destination $copy

    $missingFailed = $false
    try {
        & "$PSScriptRoot/sign-authenticode.ps1" -ExePath $copy -PfxBase64 "" -PfxPassword ""
    } catch {
        $missingFailed = $_.Exception.Message -match "PFX_B64 is empty"
    }
    if (-not $missingFailed) {
        throw "Fail-closed regression: signing accepted missing certificate material."
    }

    $cert = New-SelfSignedCertificate         -Type CodeSigningCert         -Subject $subject         -CertStoreLocation "Cert:\CurrentUser\My"         -NotAfter (Get-Date).AddDays(2)

    $cerPath = Join-Path $tempDir "test.cer"
    $pfxPath = Join-Path $tempDir "test.pfx"
    $passwordText = [Guid]::NewGuid().ToString("N")
    $password = ConvertTo-SecureString -String $passwordText -AsPlainText -Force

    Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
    Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $password | Out-Null
    $trusted = Import-Certificate -FilePath $cerPath -CertStoreLocation "Cert:\CurrentUser\Root"

    $pfx64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes($pfxPath))
    & "$PSScriptRoot/sign-authenticode.ps1"         -ExePath $copy         -PfxBase64 $pfx64         -PfxPassword $passwordText         -TimestampUrl ""         -ExpectedSubject $subject

    & "$PSScriptRoot/verify-authenticode.ps1"         -ExePath $copy         -ExpectedSubject $subject

    $bytes = [IO.File]::ReadAllBytes($copy)
    $bytes[[Math]::Floor($bytes.Length / 2)] = $bytes[[Math]::Floor($bytes.Length / 2)] -bxor 0x01
    [IO.File]::WriteAllBytes($copy, $bytes)

    $tamperFailed = $false
    try {
        & "$PSScriptRoot/verify-authenticode.ps1" -ExePath $copy -ExpectedSubject $subject
    } catch {
        $tamperFailed = $true
    }
    if (-not $tamperFailed) {
        throw "Authenticode regression: verification accepted a tampered executable."
    }

    Write-Host "Authenticode smoke gate passed: missing credentials fail, a signed file verifies, and tampering fails."
} finally {
    if ($trusted) {
        Remove-Item -LiteralPath ("Cert:\CurrentUser\Root\" + $trusted.Thumbprint) -Force -ErrorAction SilentlyContinue
    }
    if ($cert) {
        Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $cert.Thumbprint) -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
