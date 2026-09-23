[CmdletBinding()]
param()

# Behavioural gate for scripts/build-windows-installer.ps1, with no Rust
# compiled: a fake `cargo` first on PATH records what every cargo invocation
# can see.
#   1. cargo never runs with NEURALIA_AUTHENTICODE_PFX_B64 or
#      NEURALIA_AUTHENTICODE_PFX_PASSWORD in its environment. Every build
#      script and proc-macro cargo compiles inherits that environment, and the
#      release step passes the signing secrets in its env.
#   2. The payload cargo packs (NEURALIA_PAYLOAD_DIR) is exactly one file,
#      NeuralIA.exe, with the -ExePath bytes.
#   3. With -Sign the secrets still reach the signing step (an invalid PFX
#      fails at import, never with "not both configured").

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$buildScript = Join-Path $PSScriptRoot "build-windows-installer.ps1"
$work = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-build-isolation-" + [Guid]::NewGuid().ToString("N"))
$fakeBin = Join-Path $work "bin"
$fakeTarget = Join-Path $work "target"
$log = Join-Path $work "cargo-calls.jsonl"

function Get-WorkspaceVersion {
    $section = ""
    foreach ($line in [IO.File]::ReadAllLines((Join-Path $repoRoot "Cargo.toml"))) {
        if ($line -match '^\s*\[([^\]]+)\]\s*$') {
            $section = $Matches[1].Trim()
            continue
        }
        if ($section -eq "workspace.package" -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    throw "No [workspace.package] version in Cargo.toml."
}

function Get-CargoCalls {
    if (-not (Test-Path -LiteralPath $log)) {
        return @()
    }
    return @(Get-Content -LiteralPath $log | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
}

function Assert-NoSecretSeen([object[]]$Calls) {
    foreach ($call in $Calls) {
        if ($call.secretVisible) {
            throw "cargo ran with the Authenticode secrets in its environment: cargo $($call.args)"
        }
    }
}

$saved = @{}
foreach ($name in @("PATH", "NEURALIA_AUTHENTICODE_PFX_B64", "NEURALIA_AUTHENTICODE_PFX_PASSWORD", "FAKE_CARGO_LOG", "FAKE_CARGO_TARGET")) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}

try {
    New-Item -ItemType Directory -Force -Path $fakeBin, $fakeTarget | Out-Null

    # A `cargo` that only records what it sees. `metadata` answers with the
    # fake target directory; `build` writes a fake NeuralIA-Setup.exe there.
    Set-Content -LiteralPath (Join-Path $fakeBin "cargo.cmd") -Encoding ASCII -Value @(
        "@echo off",
        "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File ""%~dp0fake-cargo.ps1"" %*",
        "exit /b %ERRORLEVEL%"
    )
    Set-Content -LiteralPath (Join-Path $fakeBin "fake-cargo.ps1") -Encoding UTF8 -Value @'
$ErrorActionPreference = "Stop"
$payload = $env:NEURALIA_PAYLOAD_DIR
$files = @()
$hash = $null
if ($payload -and (Test-Path -LiteralPath $payload)) {
    $root = (Resolve-Path -LiteralPath $payload).Path
    $files = @(Get-ChildItem -LiteralPath $root -Recurse -File | ForEach-Object { $_.FullName.Substring($root.Length).TrimStart('\') })
    $exe = Join-Path $root "NeuralIA.exe"
    if (Test-Path -LiteralPath $exe) {
        $hash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
    }
}
$record = [ordered]@{
    args = ($args -join " ")
    secretVisible = -not ([string]::IsNullOrEmpty($env:NEURALIA_AUTHENTICODE_PFX_B64) -and [string]::IsNullOrEmpty($env:NEURALIA_AUTHENTICODE_PFX_PASSWORD))
    payloadFiles = $files
    payloadHash = $hash
}
Add-Content -LiteralPath $env:FAKE_CARGO_LOG -Value ($record | ConvertTo-Json -Compress)
switch ($args[0]) {
    "metadata" {
        Write-Output (@{ target_directory = $env:FAKE_CARGO_TARGET } | ConvertTo-Json -Compress)
        exit 0
    }
    "build" {
        $release = Join-Path $env:FAKE_CARGO_TARGET "release"
        New-Item -ItemType Directory -Force -Path $release | Out-Null
        [IO.File]::WriteAllBytes((Join-Path $release "NeuralIA-Setup.exe"), [byte[]](0x4D, 0x5A, 0x66, 0x61, 0x6B, 0x65))
        exit 0
    }
    default { exit 3 }
}
'@

    $fakeExe = Join-Path $work "NeuralIA.exe"
    $bytes = New-Object byte[] 4096
    (New-Object Random 7).NextBytes($bytes)
    [IO.File]::WriteAllBytes($fakeExe, $bytes)
    $expectedHash = (Get-FileHash -LiteralPath $fakeExe -Algorithm SHA256).Hash
    $version = Get-WorkspaceVersion

    $env:PATH = "$fakeBin;$($saved['PATH'])"
    $env:FAKE_CARGO_LOG = $log
    $env:FAKE_CARGO_TARGET = $fakeTarget
    $resolved = Get-Command cargo -CommandType Application | Select-Object -First 1
    if ($resolved.Source -ne (Join-Path $fakeBin "cargo.cmd")) {
        throw "The fake cargo is not the one PowerShell runs ($($resolved.Source))."
    }

    # --- 1 and 2: unsigned build with the secrets in the environment, as the
    # release step's env puts them there.
    $env:NEURALIA_AUTHENTICODE_PFX_B64 = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("secret-pfx-marker"))
    $env:NEURALIA_AUTHENTICODE_PFX_PASSWORD = "secret-password-marker"
    $outDir = Join-Path $work "out"
    & $buildScript -ExePath $fakeExe -Version $version -OutputDir $outDir | Out-Null

    $calls = @(Get-CargoCalls)
    if (-not ($calls | Where-Object { $_.args -like "build *" })) {
        throw "build-windows-installer.ps1 never ran cargo build (calls: $(@($calls | ForEach-Object { $_.args }) -join ' | '))."
    }
    Assert-NoSecretSeen $calls
    foreach ($call in @($calls | Where-Object { $_.args -like "build *" })) {
        $files = @($call.payloadFiles)
        if ($files.Count -ne 1 -or $files[0] -ne "NeuralIA.exe") {
            throw "The payload cargo packed is not exactly NeuralIA.exe: $($files -join ', ')."
        }
        if ($call.payloadHash -ne $expectedHash) {
            throw "The payload cargo packed differs from -ExePath ($($call.payloadHash) != $expectedHash)."
        }
    }
    $installer = Join-Path $outDir "NeuralIA-Setup-$version-x64.exe"
    if (-not (Test-Path -LiteralPath $installer)) {
        throw "The build did not produce $installer."
    }

    # --- 3: -Sign still gets the secrets (read before cargo ran). The fake
    # PFX is not a PFX, so the import fails -- and nothing else may.
    Remove-Item -LiteralPath $log -Force
    $env:NEURALIA_AUTHENTICODE_PFX_B64 = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes("not a pfx"))
    $env:NEURALIA_AUTHENTICODE_PFX_PASSWORD = "secret-password-marker"
    $signError = $null
    try {
        & $buildScript -ExePath $fakeExe -Version $version -OutputDir (Join-Path $work "signed") -Sign -SkipTimestamp -AllowUntrustedTestCertificate | Out-Null
    }
    catch {
        $signError = $_.Exception.Message
    }
    Assert-NoSecretSeen @(Get-CargoCalls)
    if (-not $signError) {
        throw "Signing a fake PFX succeeded; the -Sign path did not run."
    }
    if ($signError -match "not both configured") {
        throw "The signing step lost the secrets: $signError"
    }

    Write-Host "Installer build isolation passed: cargo never saw the signing secrets, the payload is exactly the -ExePath bytes, -Sign still reads the secrets (fake PFX refused: $signError)."
}
finally {
    foreach ($name in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($name, $saved[$name], "Process")
    }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
