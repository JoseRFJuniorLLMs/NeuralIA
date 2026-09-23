[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedExePath,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,

    [switch]$RequireValidSignature,

    # Keeps the uninstall registration and the shortcuts inside a private
    # sandbox (NEURALIA_SETUP_SANDBOX) instead of the user's. Use it on a
    # machine where NeuralIA is installed. CI runs without it, so the real
    # per-user Windows "Apps" registration is what gets checked there.
    [switch]$Isolated,

    # Where the fresh-install scenario lives. Defaults to RUNNER_TEMP on CI
    # (another disk than %TEMP% on GitHub runners, which exercises the
    # uninstaller's same-disk parking fallback) and to %TEMP% elsewhere.
    [string]$WorkRoot = ""
)

# Smoke test for the neural-setup installer (crates/neural-setup):
#   1. fresh silent install with /D= into a folder with a space; the installed
#      NeuralIA.exe must be byte-identical to the CI-tested one; the HKCU
#      uninstall registration, the uninstaller and the Start menu shortcut
#      exist; a NeuralIA.exe in use makes a reinstall exit 3 without touching
#      anything; the data folder is refused as an install folder (exit 2);
#      silent uninstall removes files, folder, shortcut and registration;
#   2. upgrade over a 2.1.x Inno Setup install (no /D=): the installer adopts
#      the Inno folder, replaces NeuralIA.exe, deletes unins000.* and the
#      {AppId}_is1 key, keeps the Start menu shortcut working; uninstall
#      afterwards leaves nothing;
#   3. the data folder (NEURALIA_DATA_DIR) is byte-identical at the end.

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$installer = (Resolve-Path -LiteralPath $InstallerPath).Path
$expectedExe = (Resolve-Path -LiteralPath $ExpectedExePath).Path
$expectedHash = (Get-FileHash -LiteralPath $expectedExe -Algorithm SHA256).Hash

if ($RequireValidSignature) {
    $signature = Get-AuthenticodeSignature -FilePath $installer
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Installer signature status is '$($signature.Status)', expected Valid."
    }
}

$innoAppId = "{8B2A98F4-7D55-4C43-ABF0-0D7D1A02C4B9}"
$runId = [Guid]::NewGuid().ToString("N")
if ([string]::IsNullOrWhiteSpace($WorkRoot)) {
    $WorkRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
}
$freshWork = Join-Path $WorkRoot ("neuralia-installer-smoke-" + $runId)
$tempWork = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-installer-upgrade-" + $runId)
# A space in the path proves /D= is read to the end of the command line.
$installDir = Join-Path $freshWork "Neural IA"
$upgradeDir = Join-Path $tempWork "Programs\NeuralIA"
$dataDir = Join-Path $tempWork "data\NeuralIA"
$sandbox = Join-Path $tempWork ("sandbox-" + $runId)

if ($Isolated) {
    $sandboxRegistryRoot = "HKCU:\Software\NeuralIA-SetupSandbox"
    $sandboxKey = "$sandboxRegistryRoot\sandbox-$runId"
    $uninstallBase = "$sandboxKey\Uninstall"
    $startMenu = Join-Path $sandbox "Start Menu\Programs"
    $desktop = Join-Path $sandbox "Desktop"
    $defaultRoot = Join-Path $sandbox "LocalAppData\Programs\NeuralIA"
}
else {
    $uninstallBase = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall"
    $startMenu = [Environment]::GetFolderPath("Programs")
    $desktop = [Environment]::GetFolderPath("Desktop")
    $defaultRoot = Join-Path $env:LOCALAPPDATA "Programs\NeuralIA"
}
$ourKey = "$uninstallBase\NeuralIA"
$innoKey = "$uninstallBase\${innoAppId}_is1"
$startLink = Join-Path $startMenu "NeuralIA.lnk"
$desktopLink = Join-Path $desktop "NeuralIA.lnk"
$uninstallerName = "Desinstalar NeuralIA.exe"

if (-not $Isolated) {
    # The real registration belongs to whoever owns this machine. Refuse
    # instead of overwriting (and then uninstalling) someone's NeuralIA.
    $present = @(
        @($ourKey, $innoKey, $startLink, $desktopLink, (Join-Path $defaultRoot "NeuralIA.exe")) |
            Where-Object { Test-Path -LiteralPath $_ }
    )
    if ($present.Count -gt 0) {
        throw "Refusing to touch an existing NeuralIA installation ($($present -join ', ')). Rerun with -Isolated."
    }
}

$previousSandbox = $env:NEURALIA_SETUP_SANDBOX
$previousDataDir = $env:NEURALIA_DATA_DIR
$launchedPids = New-Object System.Collections.Generic.List[int]
$cleanupUninstallers = @(
    (Join-Path $installDir $uninstallerName),
    (Join-Path $upgradeDir $uninstallerName),
    (Join-Path $defaultRoot $uninstallerName)
)

function Invoke-Setup {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string]$Arguments
    )
    # The raw argument string reaches the process as typed: /D= must be the
    # last argument and may contain spaces without quotes.
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $FilePath
    $info.Arguments = $Arguments
    $info.UseShellExecute = $false
    $process = [System.Diagnostics.Process]::Start($info)
    $launchedPids.Add($process.Id)
    if (-not $process.WaitForExit(180000)) {
        try { $process.Kill() } catch { }
        throw "'$FilePath' $Arguments did not finish within 180 s."
    }
    return $process.ExitCode
}

function Get-PathKey([string]$Path) {
    return [IO.Path]::GetFullPath($Path).TrimEnd('\').ToLowerInvariant()
}

function Assert-Registration([string]$Location) {
    if (-not (Test-Path -LiteralPath $ourKey)) {
        throw "Per-user uninstall registration was not found at $ourKey."
    }
    $entry = Get-ItemProperty -LiteralPath $ourKey
    if ($entry.DisplayName -ne "NeuralIA" -or $entry.DisplayVersion -ne $ExpectedVersion) {
        throw "Uninstall registration contains unexpected product metadata: DisplayName='$($entry.DisplayName)', DisplayVersion='$($entry.DisplayVersion)'."
    }
    if ((Get-PathKey $entry.InstallLocation) -ne (Get-PathKey $Location)) {
        throw "Uninstall registration points at '$($entry.InstallLocation)', expected '$Location'."
    }
    $uninstaller = Join-Path $Location $uninstallerName
    if ($entry.UninstallString -ne "`"$uninstaller`" --uninstall" -or $entry.QuietUninstallString -ne "`"$uninstaller`" --uninstall /S") {
        throw "Uninstall commands are wrong: '$($entry.UninstallString)' / '$($entry.QuietUninstallString)'."
    }
}

function Get-ShortcutTarget([string]$Link) {
    $shell = New-Object -ComObject WScript.Shell
    try {
        return $shell.CreateShortcut($Link).TargetPath
    }
    finally {
        [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
    }
}

function Assert-ShortcutTarget([string]$Link, [string]$Target) {
    if (-not (Test-Path -LiteralPath $Link)) {
        throw "Shortcut $Link was not created."
    }
    $actual = Get-ShortcutTarget $Link
    if ((Get-PathKey $actual) -ne (Get-PathKey $Target)) {
        throw "Shortcut $Link opens '$actual', expected '$Target'."
    }
}

function Assert-InstalledPayload([string]$Folder) {
    $installedExe = Join-Path $Folder "NeuralIA.exe"
    if (-not (Test-Path -LiteralPath $installedExe)) {
        throw "Installer completed without producing $installedExe."
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Folder $uninstallerName))) {
        throw "neural-setup did not install its uninstaller in $Folder."
    }
    $installedHash = (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash
    if ($installedHash -ne $expectedHash) {
        throw "Installed NeuralIA.exe differs from the CI-tested payload."
    }
}

function Assert-Uninstalled([string]$Folder) {
    if (Test-Path -LiteralPath (Join-Path $Folder "NeuralIA.exe")) {
        throw "NeuralIA.exe is still present after uninstall."
    }
    if (Test-Path -LiteralPath (Join-Path $Folder $uninstallerName)) {
        throw "The uninstaller is still present after uninstall."
    }
    if (Test-Path -LiteralPath $Folder) {
        throw "The install folder $Folder is still present after uninstall."
    }
    if (Test-Path -LiteralPath $ourKey) {
        throw "The uninstall registration is still present after uninstall."
    }
    foreach ($link in @($startLink, $desktopLink)) {
        if (Test-Path -LiteralPath $link) {
            throw "Shortcut $link is still present after uninstall."
        }
    }
}

# What the owner keeps in the data folder. Nothing in it may change.
$dataFiles = [ordered]@{
    "history.jsonl" = "historico"
    "gemini-live.key" = "chave"
    "memory\memory.sqlite" = "memoria"
    "WebView2\Local State" = "sessoes"
}

try {
    $env:NEURALIA_DATA_DIR = $dataDir
    if ($Isolated) {
        New-Item -ItemType Directory -Force -Path $sandbox | Out-Null
        $env:NEURALIA_SETUP_SANDBOX = $sandbox
    }
    foreach ($name in $dataFiles.Keys) {
        $path = Join-Path $dataDir $name
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
        [IO.File]::WriteAllText($path, $dataFiles[$name])
    }

    # --- 1. Fresh install -------------------------------------------------
    $code = Invoke-Setup $installer "/S /D=$installDir"
    if ($code -ne 0) {
        throw "Installer exited with code $code."
    }
    Assert-InstalledPayload $installDir
    Assert-Registration $installDir
    $installedExe = Join-Path $installDir "NeuralIA.exe"
    Assert-ShortcutTarget $startLink $installedExe
    Assert-ShortcutTarget $desktopLink $installedExe

    # NeuralIA open: the reinstall must refuse with exit code 3 and leave the
    # installed files exactly as they were.
    $lock = [IO.File]::Open($installedExe, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
    try {
        $code = Invoke-Setup $installer "/S /D=$installDir"
    }
    finally {
        $lock.Dispose()
    }
    if ($code -ne 3) {
        throw "With NeuralIA.exe in use the installer exited with code $code, expected 3."
    }
    Assert-InstalledPayload $installDir

    # The data folder is never an install folder.
    $code = Invoke-Setup $installer "/S /D=$dataDir"
    if ($code -ne 2) {
        throw "Installing into the data folder exited with code $code, expected 2."
    }
    if (Test-Path -LiteralPath (Join-Path $dataDir "NeuralIA.exe")) {
        throw "The installer wrote NeuralIA.exe into the data folder."
    }

    $code = Invoke-Setup (Join-Path $installDir $uninstallerName) "--uninstall /S"
    if ($code -ne 0) {
        throw "Uninstaller exited with code $code."
    }
    Assert-Uninstalled $installDir

    # --- 2. Upgrade over the Inno Setup 2.1.x install ---------------------
    New-Item -ItemType Directory -Force -Path $upgradeDir | Out-Null
    [IO.File]::WriteAllText((Join-Path $upgradeDir "NeuralIA.exe"), "NeuralIA 2.1.5 (Inno Setup)")
    [IO.File]::WriteAllText((Join-Path $upgradeDir "unins000.exe"), "MZ Inno Setup uninstaller")
    # The real header: 64 bytes of signature, then the AppId in ASCII.
    $log = New-Object byte[] 1681
    $signatureBytes = [Text.Encoding]::ASCII.GetBytes("Inno Setup Uninstall Log (b) 64-bit")
    [Array]::Copy($signatureBytes, $log, $signatureBytes.Length)
    $appIdBytes = [Text.Encoding]::ASCII.GetBytes($innoAppId)
    [Array]::Copy($appIdBytes, 0, $log, 64, $appIdBytes.Length)
    [IO.File]::WriteAllBytes((Join-Path $upgradeDir "unins000.dat"), $log)
    New-Item -Path $innoKey -Force | Out-Null
    $innoUninstaller = Join-Path $upgradeDir "unins000.exe"
    foreach ($value in @(
            @("DisplayName", "NeuralIA"),
            @("DisplayVersion", "2.1.5"),
            @("InstallLocation", "$upgradeDir\"),
            @("UninstallString", "`"$innoUninstaller`""),
            @("QuietUninstallString", "`"$innoUninstaller`" /SILENT")
        )) {
        New-ItemProperty -LiteralPath $innoKey -Name $value[0] -Value $value[1] -PropertyType String -Force | Out-Null
    }
    # Inno's Start menu shortcut lives where neural-setup puts its own.
    New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    try {
        $link = $shell.CreateShortcut($startLink)
        $link.TargetPath = Join-Path $upgradeDir "NeuralIA.exe"
        $link.WorkingDirectory = $upgradeDir
        $link.Save()
    }
    finally {
        [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
    }

    # No /D=: a double-click on the new installer must find the Inno folder.
    $code = Invoke-Setup $installer "/S"
    if ($code -ne 0) {
        throw "Upgrade over the Inno install exited with code $code."
    }
    Assert-InstalledPayload $upgradeDir
    foreach ($stale in @("unins000.exe", "unins000.dat")) {
        if (Test-Path -LiteralPath (Join-Path $upgradeDir $stale)) {
            throw "The Inno Setup file $stale survived the upgrade."
        }
    }
    if (Test-Path -LiteralPath $innoKey) {
        throw "The Inno Setup registration survived the upgrade: Windows Apps would list NeuralIA twice."
    }
    Assert-Registration $upgradeDir
    Assert-ShortcutTarget $startLink (Join-Path $upgradeDir "NeuralIA.exe")

    $code = Invoke-Setup (Join-Path $upgradeDir $uninstallerName) "--uninstall /S"
    if ($code -ne 0) {
        throw "Uninstaller exited with code $code after the upgrade."
    }
    Assert-Uninstalled $upgradeDir

    # --- 3. The owner's data ----------------------------------------------
    foreach ($name in $dataFiles.Keys) {
        $path = Join-Path $dataDir $name
        if (-not (Test-Path -LiteralPath $path) -or [IO.File]::ReadAllText($path) -ne $dataFiles[$name]) {
            throw "The data file $name changed during install, upgrade or uninstall."
        }
    }
    Write-Host "Installer smoke passed: exact payload, registration $ExpectedVersion, in-use refusal, data-folder refusal, Inno upgrade, clean uninstall, data untouched."
}
finally {
    foreach ($candidate in $cleanupUninstallers) {
        if (Test-Path -LiteralPath $candidate) {
            try { Invoke-Setup $candidate "--uninstall /S" | Out-Null } catch { }
        }
    }
    if ($Isolated) {
        Remove-Item -LiteralPath $sandboxKey -Recurse -Force -ErrorAction SilentlyContinue
        if ((Test-Path -LiteralPath $sandboxRegistryRoot) -and -not (Get-ChildItem -LiteralPath $sandboxRegistryRoot)) {
            Remove-Item -LiteralPath $sandboxRegistryRoot -Force -ErrorAction SilentlyContinue
        }
    }
    else {
        # Absent before this run (checked above), so anything here is ours.
        foreach ($key in @($ourKey, $innoKey)) {
            Remove-Item -LiteralPath $key -Recurse -Force -ErrorAction SilentlyContinue
        }
        foreach ($link in @($startLink, $desktopLink)) {
            Remove-Item -LiteralPath $link -Force -ErrorAction SilentlyContinue
        }
    }
    foreach ($launched in $launchedPids) {
        Remove-Item -LiteralPath (Join-Path ([IO.Path]::GetTempPath()) "neuralia-uninstaller-$launched.exe") -Force -ErrorAction SilentlyContinue
    }
    foreach ($folder in @($freshWork, $tempWork)) {
        Remove-Item -LiteralPath $folder -Recurse -Force -ErrorAction SilentlyContinue
    }
    $env:NEURALIA_SETUP_SANDBOX = $previousSandbox
    $env:NEURALIA_DATA_DIR = $previousDataDir
}
