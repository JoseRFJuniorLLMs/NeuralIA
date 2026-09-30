param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [int]$TimeoutSec = 30
)

$ErrorActionPreference = "Stop"
$exe = (Resolve-Path -LiteralPath $ExePath).Path
$utf8 = [Text.UTF8Encoding]::new($false)
$work = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-model-startup-" + [Guid]::NewGuid().ToString("N"))
$dataDir = Join-Path $work "data"
$spikeDir = Join-Path $work "spike"
$logPath = Join-Path $spikeDir "spike.log"
New-Item -ItemType Directory -Force -Path $dataDir, $spikeDir | Out-Null

$process = $null
$failure = $null

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
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_REDUCE_MOTION = "1"
    Remove-Item -LiteralPath "Env:NEURALIA_STARTUP_INPUT" -ErrorAction SilentlyContinue

    $process = Start-Process -FilePath $exe -PassThru

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
        throw "timeout waiting for the full-App modelstart probe"
    }

    if ([int64]$record.resident_model_bytes -ne 0) {
        throw "startup reported resident model bytes: $($record.resident_model_bytes)"
    }
    if ([bool]$record.manager_initialized) {
        throw "startup eagerly initialized ModelPackManager"
    }
    if ([bool]$record.pack_dir_exists) {
        throw "startup created/read the model-packs directory before an explicit model command"
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
    foreach ($name in "NEURALIA_DATA_DIR", "NEURALIA_ACCEL_SPIKE_DIR", "NEURALIA_NO_GMAIL", "NEURALIA_REDUCE_MOTION", "NEURALIA_STARTUP_INPUT") {
        Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    }
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 200
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failure) { throw $failure }
