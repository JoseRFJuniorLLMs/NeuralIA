param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [int]$MaxStartupMs = 1000,
    [double]$MaxWorkingSetMiB = 64,
    [int]$MaxThreads = 16,
    [double]$MaxIdleCpuPercent = 25,
    [string]$OutputPath = "perf-home.json"
)

$ErrorActionPreference = "Stop"
$resolved = (Resolve-Path $ExePath).Path

# Este gate mede a Home nativa EM REPOUSO. A aplicacao abre por omissao com uma
# consulta automatica, que criaria WebViews e inflacionaria RAM e threads --
# scripts/measure-cycles.ps1 e que mede esse caminho.
$env:NEURALIA_NO_STARTUP = "1"

$watch = [System.Diagnostics.Stopwatch]::StartNew()
$process = Start-Process -FilePath $resolved -PassThru

try {
    $windowReady = $false
    while ($watch.ElapsedMilliseconds -lt $MaxStartupMs) {
        Start-Sleep -Milliseconds 20
        $process.Refresh()
        if ($process.HasExited) {
            throw "NeuralIA exited before creating the native Home window."
        }
        if ($process.MainWindowHandle -ne 0) {
            $windowReady = $true
            break
        }
    }

    $startupMs = $watch.ElapsedMilliseconds
    if (-not $windowReady) {
        throw "Native Home window was not observed within ${MaxStartupMs}ms."
    }

    Start-Sleep -Milliseconds 500
    $process.Refresh()
    $workingSetMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)
    $threads = $process.Threads.Count
    $binaryMiB = [math]::Round((Get-Item $resolved).Length / 1MB, 2)

    # CPU em repouso, em percentagem de UM nucleo: delta de tempo de CPU a
    # dividir pela janela real de 3 s. A animacao da Home corre a ~15 FPS
    # enquanto a janela esta visivel e focada, e para completamente quando a
    # janela e minimizada ou tapada -- aqui ela esta visivel, portanto medimos
    # o pior caso. Se o redesenho deixar de ser barato (invalidacao da janela
    # inteira, temporizador a disparar de mais), esta amostra sobe logo.
    $cpuBefore = $process.TotalProcessorTime
    Start-Sleep -Seconds 3
    $process.Refresh()
    $cpuAfter = $process.TotalProcessorTime
    $idleCpuPercent = [math]::Round(($cpuAfter - $cpuBefore).TotalMilliseconds / 3000 * 100, 2)

    $result = [ordered]@{
        startup_ms = $startupMs
        working_set_mib = $workingSetMiB
        threads = $threads
        binary_mib = $binaryMiB
        idle_cpu_percent = $idleCpuPercent
        product_targets = [ordered]@{
            startup_ms = 200
            working_set_mib = 50
            # A SPEC-0008 pede "low single-digit" para a animacao nativa da
            # Home; 5 e a leitura numerica disso.
            idle_cpu_percent = 5
        }
        ci_regression_ceilings = [ordered]@{
            startup_ms = $MaxStartupMs
            working_set_mib = $MaxWorkingSetMiB
            threads = $MaxThreads
            # Tecto generoso: um runner partilhado tem ruido de escalonamento e
            # GPU por software, nada disto e uma bancada de medicao.
            idle_cpu_percent = $MaxIdleCpuPercent
        }
    }

    $result | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 $OutputPath
    $result | ConvertTo-Json -Depth 4 | Write-Host

    if ($workingSetMiB -gt $MaxWorkingSetMiB) {
        throw "Idle working set ${workingSetMiB} MiB exceeds CI ceiling ${MaxWorkingSetMiB} MiB."
    }
    if ($threads -gt $MaxThreads) {
        throw "Idle thread count $threads exceeds CI ceiling $MaxThreads."
    }
    if ($idleCpuPercent -gt $MaxIdleCpuPercent) {
        throw "Idle CPU ${idleCpuPercent}% of one core exceeds CI ceiling ${MaxIdleCpuPercent}%."
    }
}
finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
}
