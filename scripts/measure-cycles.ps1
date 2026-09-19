<#
.SYNOPSIS
    Gate de ciclo de vida: Comparar -> Home, N vezes, contando WebViews.

.DESCRIPTION
    O gate de arranque (measure-home.ps1) mede a Home em repouso e por isso nao
    detectaria o pior defeito que ja existiu aqui: os WebViews do comparador
    sobreviverem ao regresso a Home. Este script fecha esse buraco.

    Para cada ciclo:
      Enter  -> o comparador abre e nascem processos msedgewebview2.exe
      Escape -> volta a Home e esses processos tem de desaparecer TODOS

    So contam os msedgewebview2.exe que descendem do nosso processo: a maquina
    pode ter outras aplicacoes WebView2 a correr.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [int]$Cycles = 5,
    [int]$OpenTimeoutSec = 25,
    [int]$CloseTimeoutSec = 20,
    [double]$MaxWorkingSetGrowthMiB = 24,
    [string]$StartupInput = "jose r f junior",
    [string]$OutputPath = "perf-cycles.json"
)

$ErrorActionPreference = "Stop"
$resolved = (Resolve-Path $ExePath).Path

function Get-DescendantIds([int]$RootId) {
    # Win32_Process via CIM is useful for ownership, but on hosted Windows
    # runners a WMI/CIM query can occasionally stall for minutes. Bound this
    # optional precision source; the total-process delta remains the fallback.
    try {
        $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name -OperationTimeoutSec 2 -ErrorAction Stop)
    }
    catch {
        Write-Warning "Win32_Process snapshot unavailable; using WebView process delta: $($_.Exception.Message)"
        return @()
    }
    $byParent = @{}
    foreach ($proc in $all) {
        if (-not $byParent.ContainsKey($proc.ParentProcessId)) {
            $byParent[$proc.ParentProcessId] = New-Object System.Collections.ArrayList
        }
        $null = $byParent[$proc.ParentProcessId].Add($proc)
    }

    $found = New-Object System.Collections.ArrayList
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue($RootId)
    while ($queue.Count -gt 0) {
        $current = $queue.Dequeue()
        if (-not $byParent.ContainsKey($current)) { continue }
        foreach ($child in $byParent[$current]) {
            $null = $found.Add($child)
            $queue.Enqueue($child.ProcessId)
        }
    }
    return $found
}

function Get-TotalWebViewCount {
    return @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue).Count
}

# Conta por ascendencia E por diferenca em relacao a linha de base. A ascendencia
# e mais precisa quando funciona, mas o WebView2 nem sempre mantem os processos
# como descendentes de quem os criou -- num runner do GitHub a arvore deu zero
# enquanto a RAM subia 13 MiB. A diferenca e imune a isso; as outras aplicacoes
# WebView2 da maquina mantem a sua contagem constante.
function Get-WebViewCount([int]$RootId) {
    $descendants = Get-DescendantIds -RootId $RootId
    $owned = @($descendants | Where-Object { $_.Name -eq "msedgewebview2.exe" }).Count
    $delta = (Get-TotalWebViewCount) - $script:WebViewBaseline
    return [math]::Max($owned, [math]::Max($delta, 0))
}

function Wait-ForWebViews([int]$RootId, [scriptblock]$Predicate, [int]$TimeoutSec) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $count = Get-WebViewCount -RootId $RootId
        if (& $Predicate $count) { return $count }
        Start-Sleep -Milliseconds 400
    }
    return Get-WebViewCount -RootId $RootId
}

$script:WebViewBaseline = @(Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue).Count

$env:NEURALIA_STARTUP_INPUT = $StartupInput

# O monitor do Gmail e uma excepcao INTENCIONAL ao "zero WebViews na Home": ele
# nasce quando existe sessao Google no perfil WebView2 e fica vivo mesmo depois
# de voltar a Home, por desenho. Este gate conta processos, nao intencoes, por
# isso sem esta variavel ele falharia em qualquer maquina com sessao Google --
# no CI passa por acaso, porque o runner e limpo e nunca tem sessao.
$env:NEURALIA_NO_GMAIL = "1"

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
            # A omnibox mantem texto e foco ao voltar a Home, mas reescrevemos
            # a consulta para o ciclo nao depender do estado anterior.
            $null = $shell.AppActivate($process.Id)
            Start-Sleep -Milliseconds 400
            $shell.SendKeys("^a")
            $shell.SendKeys($StartupInput)
            $shell.SendKeys("{ENTER}")
        }

        $opened = Wait-ForWebViews -RootId $process.Id -Predicate { param($n) $n -gt 0 } -TimeoutSec $OpenTimeoutSec
        # WebView2 pode reutilizar processos entre controllers. A contagem
        # prova a abertura no primeiro ciclo; nos seguintes o gate principal e
        # retorno a Home + crescimento de working set.
        if ($cycle -eq 1 -and $opened -le 0) {
            $null = $failures.Add("ciclo 1: o comparador nao criou nenhum WebView observavel")
        }

        $null = $shell.AppActivate($process.Id)
        Start-Sleep -Milliseconds 400
        $shell.SendKeys("{ESC}")

        $closed = Wait-ForWebViews -RootId $process.Id -Predicate { param($n) $n -eq 0 } -TimeoutSec $CloseTimeoutSec
        if ($closed -ne 0) {
            $null = $failures.Add("ciclo ${cycle}: ${closed} processo(s) WebView2 sobreviveram ao regresso a Home")
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
        webview_baseline = $script:WebViewBaseline
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
}
