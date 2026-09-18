<#
.SYNOPSIS
    Gate real de ciclo de vida: Comparar -> Home, N vezes.

.DESCRIPTION
    O processo WebView2 pode ser reutilizado pelo runtime e por isso contar
    msedgewebview2.exe nao prova quantos WebViews o NeuralIA tem vivos. Em modo
    de CI o proprio app publica apenas a contagem de controladores WebView num
    ficheiro-probe. O gate exige 3 no comparador e 0 depois de voltar a Home.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [int]$Cycles = 5,
    [int]$OpenTimeoutSec = 25,
    [int]$CloseTimeoutSec = 20,
    [double]$MaxWorkingSetGrowthMiB = 24,
    [string]$StartupInput = "neuralia lifecycle probe",
    [string]$OutputPath = "perf-cycles.json"
)

$ErrorActionPreference = "Stop"
$resolved = (Resolve-Path $ExePath).Path
$probeDir = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$probePath = Join-Path $probeDir ("neuralia-lifecycle-" + [guid]::NewGuid().ToString("N") + ".txt")

function Wait-ForProbe([scriptblock]$Predicate, [int]$TimeoutSec) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $last = -1
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if (Test-Path $probePath) {
            try {
                $raw = (Get-Content -Raw $probePath).Trim()
                if ($raw -match '^\d+$') {
                    $last = [int]$raw
                    if (& $Predicate $last) { return $last }
                }
            } catch {
                # O app pode estar a substituir o ficheiro exatamente agora.
            }
        }
        Start-Sleep -Milliseconds 120
    }
    return $last
}

$env:NEURALIA_STARTUP_INPUT = $StartupInput
$env:NEURALIA_LIFECYCLE_PROBE = $probePath
$process = Start-Process -FilePath $resolved -PassThru
$failures = New-Object System.Collections.ArrayList
$samples = New-Object System.Collections.ArrayList

try {
    $shell = New-Object -ComObject WScript.Shell

    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt 10 -and $process.MainWindowHandle -eq 0) {
        Start-Sleep -Milliseconds 50
        $process.Refresh()
        if ($process.HasExited) { throw "NeuralIA saiu antes de criar a janela." }
    }
    if ($process.MainWindowHandle -eq 0) { throw "A janela nativa nao apareceu." }

    $process.Refresh()
    $baselineMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)

    for ($cycle = 1; $cycle -le $Cycles; $cycle++) {
        if ($cycle -gt 1) {
            $null = $shell.AppActivate($process.Id)
            Start-Sleep -Milliseconds 250
            $shell.SendKeys("^a")
            $shell.SendKeys($StartupInput)
            $shell.SendKeys("{ENTER}")
        }

        $opened = Wait-ForProbe -Predicate { param($n) $n -eq 3 } -TimeoutSec $OpenTimeoutSec
        if ($opened -ne 3) {
            $null = $failures.Add("ciclo ${cycle}: esperado 3 WebViews no comparador; probe=$opened")
        }

        $null = $shell.AppActivate($process.Id)
        Start-Sleep -Milliseconds 250
        $shell.SendKeys("{ESC}")

        $closed = Wait-ForProbe -Predicate { param($n) $n -eq 0 } -TimeoutSec $CloseTimeoutSec
        if ($closed -ne 0) {
            $null = $failures.Add("ciclo ${cycle}: esperado 0 WebViews na Home; probe=$closed")
        }

        $process.Refresh()
        $null = $samples.Add([ordered]@{
            cycle = $cycle
            webviews_open = $opened
            webviews_after_home = $closed
            working_set_mib = [math]::Round($process.WorkingSet64 / 1MB, 2)
        })
    }

    $process.Refresh()
    $finalMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)
    $growthMiB = [math]::Round($finalMiB - $baselineMiB, 2)
    if ($growthMiB -gt $MaxWorkingSetGrowthMiB) {
        $null = $failures.Add("working set cresceu ${growthMiB} MiB em $Cycles ciclos (tecto ${MaxWorkingSetGrowthMiB} MiB)")
    }

    $result = [ordered]@{
        cycles = $Cycles
        baseline_working_set_mib = $baselineMiB
        final_working_set_mib = $finalMiB
        working_set_growth_mib = $growthMiB
        samples = $samples
        failures = $failures
    }
    $result | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 $OutputPath
    $result | ConvertTo-Json -Depth 5 | Write-Host

    if ($failures.Count -gt 0) {
        throw ($failures -join "; ")
    }
}
finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item $probePath -Force -ErrorAction SilentlyContinue
    Remove-Item Env:NEURALIA_LIFECYCLE_PROBE -ErrorAction SilentlyContinue
}
