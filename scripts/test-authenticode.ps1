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
$trusted = $false

try {
    $copy = Join-Path $tempDir "NeuralIA-test.exe"
    Copy-Item -LiteralPath $source -Destination $copy

    Write-Host "[authenticode-test] proving missing certificate material fails closed"
    $missingFailed = $false
    try {
        & "$PSScriptRoot/sign-authenticode.ps1" -ExePath $copy -PfxBase64 "" -PfxPassword ""
    } catch {
        $missingFailed = $true
        Write-Host "Expected fail-closed result: $($_.Exception.Message)"
    }
    if (-not $missingFailed) {
        throw "Fail-closed regression: signing accepted missing certificate material."
    }

    Write-Host "[authenticode-test] generating ephemeral code-signing certificate"
    $certParams = @{
        Type = "CodeSigningCert"
        Subject = $subject
        CertStoreLocation = "Cert:\CurrentUser\My"
        NotAfter = (Get-Date).AddDays(2)
    }
    $cert = New-SelfSignedCertificate @certParams

    $cerPath = Join-Path $tempDir "test.cer"
    $pfxPath = Join-Path $tempDir "test.pfx"
    $passwordText = [Guid]::NewGuid().ToString("N")
    $password = ConvertTo-SecureString -String $passwordText -AsPlainText -Force

    Write-Host "[authenticode-test] exporting ephemeral certificate"
    Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
    Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $password | Out-Null

    Write-Host "[authenticode-test] trusting ephemeral certificate non-interactively"
    & certutil.exe -user -f -addstore Root $cerPath | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "certutil failed to add the ephemeral certificate to CurrentUser Root."
    }
    $trusted = $true

    $pfx64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes($pfxPath))
    $signParams = @{
        ExePath = $copy
        PfxBase64 = $pfx64
        PfxPassword = $passwordText
        TimestampUrl = ""
        ExpectedSubject = $subject
    }

    Write-Host "[authenticode-test] signing disposable executable copy"
    & "$PSScriptRoot/sign-authenticode.ps1" @signParams

    Write-Host "[authenticode-test] verifying valid signature"
    & "$PSScriptRoot/verify-authenticode.ps1" -ExePath $copy -ExpectedSubject $subject

    Write-Host "[authenticode-test] tampering with signed copy"
    $bytes = [IO.File]::ReadAllBytes($copy)
    $index = [Math]::Floor($bytes.Length / 2)
    $bytes[$index] = $bytes[$index] -bxor 0x01
    [IO.File]::WriteAllBytes($copy, $bytes)

    $tamperFailed = $false
    try {
        & "$PSScriptRoot/verify-authenticode.ps1" -ExePath $copy -ExpectedSubject $subject
    } catch {
        $tamperFailed = $true
        Write-Host "Expected tamper rejection: $($_.Exception.Message)"
    }
    if (-not $tamperFailed) {
        throw "Authenticode regression: verification accepted a tampered executable."
    }

    Write-Host "Authenticode smoke gate passed: missing credentials fail, valid signing verifies, tampering fails."
} finally {
    if ($trusted -and $cert) {
        & certutil.exe -user -delstore Root $cert.Thumbprint | Out-Null
    }
    if ($cert) {
        Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $cert.Thumbprint) -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
