[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedExePath,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,

    [switch]$RequireValidSignature
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$installer = (Resolve-Path -LiteralPath $InstallerPath).Path
$expectedExe = (Resolve-Path -LiteralPath $ExpectedExePath).Path
$root = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$installDir = Join-Path $root ("neuralia-installer-smoke-" + [Guid]::NewGuid().ToString("N"))
$uninstaller = $null
$didUninstall = $false

if ($RequireValidSignature) {
    $signature = Get-AuthenticodeSignature -FilePath $installer
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Installer signature status is '$($signature.Status)', expected Valid."
    }
}

try {
    $install = Start-Process -FilePath $installer -ArgumentList @(
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/DIR=$installDir"
    ) -Wait -PassThru

    if ($install.ExitCode -ne 0) {
        throw "Installer exited with code $($install.ExitCode)."
    }

    $installedExe = Join-Path $installDir "NeuralIA.exe"
    if (-not (Test-Path -LiteralPath $installedExe)) {
        throw "Installer completed without producing $installedExe."
    }

    $expectedHash = (Get-FileHash -LiteralPath $expectedExe -Algorithm SHA256).Hash
    $installedHash = (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash
    if ($expectedHash -ne $installedHash) {
        throw "Installed NeuralIA.exe differs from the CI-tested payload."
    }

    $uninstallRoot = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall"
    $entry = Get-ChildItem -Path $uninstallRoot -ErrorAction Stop |
        ForEach-Object { Get-ItemProperty -LiteralPath $_.PSPath -ErrorAction SilentlyContinue } |
        Where-Object {
            $_.DisplayName -eq "NeuralIA" -and
            $_.DisplayVersion -eq $ExpectedVersion -and
            $_.InstallLocation -and
            ([IO.Path]::GetFullPath($_.InstallLocation.TrimEnd("\\")) -eq [IO.Path]::GetFullPath($installDir))
        } |
        Select-Object -First 1

    if (-not $entry) {
        throw "Per-user uninstall registration was not found in HKCU."
    }

    $uninstaller = Join-Path $installDir "unins000.exe"
    if (-not (Test-Path -LiteralPath $uninstaller)) {
        throw "Inno Setup uninstaller was not installed."
    }

    $uninstall = Start-Process -FilePath $uninstaller -ArgumentList @(
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART"
    ) -Wait -PassThru

    if ($uninstall.ExitCode -ne 0) {
        throw "Uninstaller exited with code $($uninstall.ExitCode)."
    }
    $didUninstall = $true

    Start-Sleep -Milliseconds 500
    if (Test-Path -LiteralPath $installedExe) {
        throw "NeuralIA.exe is still present after uninstall."
    }
}
finally {
    if (-not $didUninstall -and $uninstaller -and (Test-Path -LiteralPath $uninstaller)) {
        Start-Process -FilePath $uninstaller -ArgumentList @(
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART"
        ) -Wait -ErrorAction SilentlyContinue | Out-Null
    }
    if (Test-Path -LiteralPath $installDir) {
        Remove-Item -LiteralPath $installDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
