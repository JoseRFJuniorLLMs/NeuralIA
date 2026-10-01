param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [int]$TimeoutSec = 30
)

$ErrorActionPreference = "Stop"
$exe = (Resolve-Path -LiteralPath $ExePath).Path
$utf8 = [Text.UTF8Encoding]::new($false)
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
$work = [IO.Path]::GetFullPath((Join-Path $tempRoot ("neuralia-model-startup-" + [Guid]::NewGuid().ToString("N"))))
if (-not $work.StartsWith($tempRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "startup fixture directory escaped the temp root"
}
$dataDir = Join-Path $work "data"
$spikeDir = Join-Path $work "spike"
$logPath = Join-Path $spikeDir "spike.log"
$debugPath = Join-Path $work "debug.log"
New-Item -ItemType Directory -Force -Path $dataDir, $spikeDir | Out-Null

$process = $null
$failure = $null
$envNames = @("NEURALIA_DATA_DIR", "NEURALIA_ACCEL_SPIKE_DIR", "NEURALIA_DEBUG_LOG", "NEURALIA_NO_GMAIL", "NEURALIA_REDUCE_MOTION", "NEURALIA_STARTUP_INPUT")
$savedEnv = @{}
foreach ($name in $envNames) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}

function Read-Records {
    if (-not (Test-Path -LiteralPath $logPath)) { return @() }
    $stream = $null
    $reader = $null
    $records = @()
    try {
        $stream = [IO.FileStream]::new(
            $logPath,
            [IO.FileMode]::Open,
            [IO.FileAccess]::Read,
            [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete
        )
        $reader = [IO.StreamReader]::new($stream, $utf8, $true)
        while (-not $reader.EndOfStream) {
            $line = $reader.ReadLine()
            if ([string]::IsNullOrWhiteSpace($line)) { continue }
            try { $records += ($line | ConvertFrom-Json) } catch {}
        }
    }
    catch [IO.IOException] {
        return @()
    }
    finally {
        if ($reader) { $reader.Dispose() }
        elseif ($stream) { $stream.Dispose() }
    }
    return @($records)
}

try {
    if (Test-Path -LiteralPath (Join-Path $dataDir "model-packs")) {
        throw "fixture started with an unexpected model-packs directory"
    }

    $env:NEURALIA_DATA_DIR = $dataDir
    $env:NEURALIA_ACCEL_SPIKE_DIR = $spikeDir
    $env:NEURALIA_DEBUG_LOG = $debugPath
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_REDUCE_MOTION = "1"
    Remove-Item -LiteralPath "Env:NEURALIA_STARTUP_INPUT" -ErrorAction SilentlyContinue

    $process = Start-Process -FilePath $exe -PassThru -WindowStyle Hidden

    $watch = [Diagnostics.Stopwatch]::StartNew()
    $record = $null
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if ($process.HasExited) {
            throw "NeuralIA exited before emitting modelstart (code $($process.ExitCode))."
        }
        $record = @(Read-Records | Where-Object { [string]$_.t -eq "modelstart" } | Select-Object -Last 1)
        if ($record.Count -gt 0) {
            $record = $record[0]
            break
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $record) {
        $logTail = if (Test-Path -LiteralPath $logPath) { (Get-Content -LiteralPath $logPath -Tail 8) -join " | " } else { "spike log absent" }
        $debugTail = if (Test-Path -LiteralPath $debugPath) { (Get-Content -LiteralPath $debugPath -Tail 8) -join " | " } else { "debug log absent" }
        throw "timeout waiting for the full-App modelstart probe; $logTail; $debugTail"
    }
    foreach ($name in "resident_model_bytes", "manager_initialized", "pack_dir_exists") {
        if ($null -eq $record.PSObject.Properties[$name]) {
            throw "modelstart record missing $name"
        }
    }
    if ($record.resident_model_bytes -isnot [long] -or
        $record.manager_initialized -isnot [bool] -or
        $record.pack_dir_exists -isnot [bool]) {
        throw "modelstart record has invalid field types"
    }

    if ($record.resident_model_bytes -ne 0) {
        throw "startup adapter reported model bytes: $($record.resident_model_bytes)"
    }
    if ($record.manager_initialized) {
        throw "startup eagerly initialized ModelPackManager"
    }
    if ($record.pack_dir_exists) {
        throw "model-packs directory exists at the startup probe"
    }
    if (Test-Path -LiteralPath (Join-Path $dataDir "model-packs")) {
        throw "model-packs directory exists after the startup probe"
    }

    Write-Host "SPEC-0102 full-App startup: resident_model_bytes=0, manager_initialized=false, model-packs absent."
}
catch {
    $failure = $_.Exception.Message
}
finally {
    foreach ($name in $envNames) {
        if ($null -eq $savedEnv[$name]) {
            Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
        } else {
            Set-Item -LiteralPath "Env:$name" -Value $savedEnv[$name]
        }
    }
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        if (-not $process.WaitForExit(5000)) {
            $failure = "$failure; startup probe process did not exit within 5 seconds"
        }
    }
    if ($work.StartsWith($tempRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -and
        [IO.Path]::GetFileName($work).StartsWith("neuralia-model-startup-", [StringComparison]::Ordinal)) {
        try {
            Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction Stop
        } catch {
            $failure = "$failure; fixture cleanup failed: $($_.Exception.Message)"
        }
    }
}
if ($failure) { throw $failure }
