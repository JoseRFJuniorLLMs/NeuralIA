# EXPERIMENT ONLY (branch ci/first-paint-probe, never merged).
# Runs the old gate (fixed 12 s after the window) and the new gate (event
# ordered, hang detector only) against the same exe under the same conditions,
# and keeps the app debug log of every launch.
param(
    [Parameter(Mandatory = $true)] [string]$ExePath,
    [Parameter(Mandatory = $true)] [string]$OutDir,
    [int]$StressRounds = 4,
    [int]$QuietRounds = 2
)
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force $OutDir | Out-Null
$rows = New-Object System.Collections.Generic.List[object]

function Start-Stress {
    $procs = @()
    $n = [Environment]::ProcessorCount * 2
    for ($i = 0; $i -lt $n; $i++) {
        $procs += Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @(
            "-NoProfile", "-Command", '$x = 0; while ($true) { $x = ($x + 1) % 1000003 }'
        )
    }
    $disk = Join-Path $env:RUNNER_TEMP "stress-disk"
    New-Item -ItemType Directory -Force $disk | Out-Null
    $procs += Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @(
        "-NoProfile", "-Command",
        "`$buf = New-Object byte[] (8MB); (New-Object Random).NextBytes(`$buf); while (`$true) { `$f = [IO.File]::Open('$disk\\blob.bin', 'Create', 'Write'); for (`$j = 0; `$j -lt 64; `$j++) { `$f.Write(`$buf, 0, `$buf.Length); `$f.Flush(`$true) }; `$f.Dispose() }"
    )
    Start-Sleep -Seconds 2
    return $procs
}

function Stop-Stress($procs) {
    foreach ($p in $procs) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
}

function Invoke-Launch([string]$Label, [string]$Gate, [bool]$FreshProfile, [bool]$Stress) {
    $profileDir = ""
    if ($FreshProfile) {
        $profileDir = Join-Path $env:RUNNER_TEMP ("wv2-" + [Guid]::NewGuid().ToString("N"))
    }
    $stressProcs = @()
    if ($Stress) { $stressProcs = Start-Stress }
    $json = Join-Path $OutDir "$Label.json"
    $log = Join-Path $OutDir "$Label.applog.txt"
    $console = Join-Path $OutDir "$Label.console.txt"
    $passed = $false
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        if ($Gate -eq "new") {
            $gateArgs = @{ ExePath = $ExePath; OutputPath = $json }
            if ($profileDir) { $gateArgs.WebViewProfile = $profileDir }
            & ./scripts/test-comparator-first-paint.ps1 @gateArgs *> $console
        } else {
            # The old gate does not know about the profile or the debug log:
            # both reach the app through the inherited environment.
            if ($profileDir) {
                New-Item -ItemType Directory -Force $profileDir | Out-Null
                $env:WEBVIEW2_USER_DATA_FOLDER = $profileDir
            }
            $env:NEURALIA_DEBUG_LOG = $log
            & ./scripts/experiment-old-first-paint.ps1 -ExePath $ExePath *> $console
        }
        $passed = $true
    } catch {
        Add-Content $console "THROWN: $($_.Exception.Message)"
    } finally {
        $env:WEBVIEW2_USER_DATA_FOLDER = $null
        $env:NEURALIA_DEBUG_LOG = $null
        Stop-Stress $stressProcs
        Get-Process msedgewebview2 -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        Get-Process NeuralIA -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 1
    }
    $row = [ordered]@{
        label = $Label; gate = $Gate; fresh_profile = $FreshProfile; stress = $Stress
        passed = $passed; wall_ms = $watch.ElapsedMilliseconds
        window_ms = $null; activated_ms = $null; settled_ms = $null; first3_ms = $null
        col_build = ""; verdict = ""
    }
    if ($Gate -eq "new" -and (Test-Path $json)) {
        $r = Get-Content $json -Raw | ConvertFrom-Json
        $row.verdict = $r.verdict
        $row.window_ms = $r.window_ms
        $row.activated_ms = $r.comparator_activated_ms
        $row.settled_ms = $r.relayouts_settled_ms
        foreach ($t in $r.timeline) {
            if ($t -match '^\s*(\d+) ms\s+(\d+) superficie' -and [int]$Matches[2] -ge 3) { $row.first3_ms = [int]$Matches[1]; break }
        }
        $appLines = @($r.app_log)
    } else {
        $appLines = if (Test-Path $log) { @(Get-Content $log) } else { @() }
        $row.verdict = if ($passed) { "old-ok" } else { "old-fail" }
    }
    # Column build durations from the app log (ms since the first log line).
    $starts = @{}; $parts = @()
    foreach ($line in $appLines) {
        if ($line -match '^\s*(\d+) ms\s+open_comparator: coluna (\d) a construir') { $starts[$Matches[2]] = [int]$Matches[1] }
        elseif ($line -match '^\s*(\d+) ms\s+open_comparator: coluna (\d) construida') {
            $c = $Matches[2]; if ($starts.ContainsKey($c)) { $parts += "c$c=" + ([int]$Matches[1] - $starts[$c]) }
        }
    }
    foreach ($c in $starts.Keys) { if (-not ($parts -match "^c$c=")) { $parts += "c$c=UNFINISHED" } }
    $row.col_build = ($parts | Sort-Object) -join " "
    $rows.Add([pscustomobject]$row)
    Write-Host ("{0,-22} gate={1} fresh={2} stress={3} passed={4} verdict={5} window={6} first3={7} activated={8} cols[{9}] wall={10}" -f `
        $Label, $Gate, $FreshProfile, $Stress, $passed, $row.verdict, $row.window_ms, $row.first3_ms, $row.activated_ms, $row.col_build, $row.wall_ms)
}

# 1. Exactly the CI condition: first WebView2 launch on this VM, product profile.
Invoke-Launch "00-natural-new" "new" $false $false
# 2. Old and new alternately on a fresh WebView2 profile, under load and quiet.
for ($k = 1; $k -le $StressRounds; $k++) {
    Invoke-Launch ("{0:D2}-stress-old" -f $k) "old" $true $true
    Invoke-Launch ("{0:D2}-stress-new" -f $k) "new" $true $true
}
for ($k = 1; $k -le $QuietRounds; $k++) {
    Invoke-Launch ("{0:D2}-quiet-old" -f (10 + $k)) "old" $true $false
    Invoke-Launch ("{0:D2}-quiet-new" -f (10 + $k)) "new" $true $false
}

$rows | ConvertTo-Json -Depth 3 | Set-Content -Encoding utf8 (Join-Path $OutDir "summary.json")
$md = @("| launch | gate | fresh | stress | passed | verdict | window ms | first 3 visible ms | activated ms | column builds (ms) |", "|---|---|---|---|---|---|---|---|---|---|")
foreach ($r in $rows) {
    $md += "| $($r.label) | $($r.gate) | $($r.fresh_profile) | $($r.stress) | $($r.passed) | $($r.verdict) | $($r.window_ms) | $($r.first3_ms) | $($r.activated_ms) | $($r.col_build) |"
}
$md -join "`n" | Add-Content $env:GITHUB_STEP_SUMMARY
$md -join "`n" | Write-Host
