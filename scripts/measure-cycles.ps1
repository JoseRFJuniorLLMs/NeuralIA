<#
.SYNOPSIS
    Gate real de ciclo de vida: Comparador -> Home, N vezes.

.DESCRIPTION
    O processo WebView2 pode ser reutilizado pelo runtime, portanto contar
    msedgewebview2.exe nao prova quantos controladores WebView o NeuralIA tem.
    Em modo de CI o proprio app executa os ciclos pelo event loop nativo e
    publica somente a contagem de WebViews num ficheiro-probe. O gate exige
    exatamente 3 no comparador e 0 depois de cada retorno a Home.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [int]$Cycles = 5,
    [int]$TransitionTimeoutSec = 35,
    [double]$MaxWorkingSetGrowthMiB = 24,
    [string]$OutputPath = "perf-cycles.json"
)

$ErrorActionPreference = "Stop"
$resolved = (Resolve-Path $ExePath).Path
$probeDir = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$probePath = Join-Path $probeDir ("neuralia-lifecycle-" + [guid]::NewGuid().ToString("N") + ".txt")

function Wait-ForProbe([int]$Expected, [int]$TimeoutSec) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $last = -1
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if (Test-Path $probePath) {
            try {
                $raw = (Get-Content -Raw $probePath).Trim()
                if ($raw -match '^\d+$') {
                    $last = [int]$raw
                    if ($last -eq $Expected) { return $last }
                }
            } catch {
                # O app pode estar substituindo o ficheiro exatamente agora.
            }
        }
        Start-Sleep -Milliseconds 100
    }
    return $last
}

$env:NEURALIA_NO_STARTUP = "1"
$env:NEURALIA_LIFECYCLE_PROBE = $probePath
$env:NEURALIA_LIFECYCLE_SELFTEST = "$Cycles"
$process = Start-Process -FilePath $resolved -PassThru
$failures = New-Object System.Collections.ArrayList
$samples = New-Object System.Collections.ArrayList

try {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt 10 -and $process.MainWindowHandle -eq 0) {
        Start-Sleep -Milliseconds 50
        $process.Refresh()
        if ($process.HasExited) { throw "NeuralIA saiu antes de criar a janela." }
    }
    if ($process.MainWindowHandle -eq 0) { throw "A janela nativa nao apareceu." }

    $process.Refresh()
    $coldBaselineMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)
    $warmBaselineMiB = $null

    for ($cycle = 1; $cycle -le $Cycles; $cycle++) {
        $opened = Wait-ForProbe -Expected 3 -TimeoutSec $TransitionTimeoutSec
        if ($opened -ne 3) {
            $null = $failures.Add("ciclo ${cycle}: esperado 3 WebViews no comparador; probe=$opened")
            break
        }

        $closed = Wait-ForProbe -Expected 0 -TimeoutSec $TransitionTimeoutSec
        if ($closed -ne 0) {
            $null = $failures.Add("ciclo ${cycle}: esperado 0 WebViews na Home; probe=$closed")
            break
        }

        $process.Refresh()
        $currentMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)
        if ($null -eq $warmBaselineMiB) {
            # O primeiro ciclo carrega o runtime WebView2 pela primeira vez.
            # Esse custo frio e permanente no processo, mas nao e crescimento
            # por ciclo. A fronteira de leak comeca na primeira Home aquecida.
            $warmBaselineMiB = $currentMiB
        }
        $null = $samples.Add([ordered]@{
            cycle = $cycle
            webviews_open = $opened
            webviews_after_home = $closed
            working_set_mib = $currentMiB
        })
    }

    $process.Refresh()
    $finalMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)
    if ($null -eq $warmBaselineMiB) {
        $warmBaselineMiB = $coldBaselineMiB
    }
    $coldGrowthMiB = [math]::Round($finalMiB - $coldBaselineMiB, 2)
    $growthMiB = [math]::Round($finalMiB - $warmBaselineMiB, 2)
    if ($growthMiB -gt $MaxWorkingSetGrowthMiB) {
        $null = $failures.Add("working set pós-warm-up cresceu ${growthMiB} MiB em $Cycles ciclos (tecto ${MaxWorkingSetGrowthMiB} MiB)")
    }

    $result = [ordered]@{
        cycles = $Cycles
        cold_start_working_set_mib = $coldBaselineMiB
        warm_home_working_set_mib = $warmBaselineMiB
        final_working_set_mib = $finalMiB
        cold_start_growth_mib = $coldGrowthMiB
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
    Remove-Item Env:NEURALIA_NO_STARTUP -ErrorAction SilentlyContinue
    Remove-Item Env:NEURALIA_LIFECYCLE_PROBE -ErrorAction SilentlyContinue
    Remove-Item Env:NEURALIA_LIFECYCLE_SELFTEST -ErrorAction SilentlyContinue
}
