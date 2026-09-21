<#
.SYNOPSIS
    Gate de ciclo de vida: Comparar -> Home, N vezes, contando WebViews.

.DESCRIPTION
    O gate de arranque (measure-home.ps1) mede a Home em repouso e por isso nao
    detectaria o pior defeito que ja existiu aqui: os WebViews do comparador
    sobreviverem ao regresso a Home. Este script fecha esse buraco.

    Para cada ciclo:
      startup/omnibox nativa -> o comparador abre e aparecem containers WRY_WEBVIEW
      Escape enviado ao EDIT nativo -> HomeRequested pelo caminho real do produto
      Home -> nenhum container WRY_WEBVIEW pode continuar visivel
      o script pede a reabertura por uma mensagem Win32 privada habilitada
      somente em NEURALIA_LIFECYCLE_PROBE, depois de observar Home limpa

    O runtime WebView2 pode manter um pool de subprocessos para reutilizacao.
    Esse pool pode sobreviver aos controllers, mas nao pode crescer de ciclo em
    ciclo. Tambem mantemos o gate de working set para apanhar crescimento real.

    A visibilidade e medida no container Win32 WRY_WEBVIEW criado pelo WRY,
    que e a superficie controlada pelo NeuralIA. Os processos msedgewebview2
    sao medidos separadamente apenas para detectar crescimento persistente do pool.
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

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class NeuraliaCycleWindowProbe {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left, Top, Right, Bottom;
    }

    [DllImport("user32.dll")]
    public static extern bool EnumChildWindows(IntPtr hWndParent, EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindowEx(
        IntPtr hWndParent,
        IntPtr hWndChildAfter,
        string lpszClass,
        string lpszWindow
    );

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder className, int maxCount);

    private static bool IsWryWebViewHost(IntPtr hwnd) {
        var name = new StringBuilder(128);
        GetClassName(hwnd, name, name.Capacity);
        return string.Equals(name.ToString(), "WRY_WEBVIEW", StringComparison.Ordinal);
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern bool SetWindowText(IntPtr hWnd, string text);

    [return: MarshalAs(UnmanagedType.Bool)]
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    public static List<string> VisibleWryWebViewRects(IntPtr parent) {
        var rows = new List<string>();
        EnumChildWindows(parent, delegate(IntPtr hwnd, IntPtr data) {
            if (!IsWindowVisible(hwnd) || !IsWryWebViewHost(hwnd)) return true;
            RECT r;
            if (!GetWindowRect(hwnd, out r)) return true;
            var width = r.Right - r.Left;
            var height = r.Bottom - r.Top;
            if (width < 1 || height < 1) return true;
            rows.Add(r.Left + "," + r.Top + "," + r.Right + "," + r.Bottom);
            return true;
        }, IntPtr.Zero);
        return rows;
    }

    public static IntPtr FindOmniboxEdit(IntPtr parent) {
        // A omnibox principal e o unico EDIT filho DIRETO da janela principal.
        // A palette tambem tem EDIT, mas vive dentro de um popup STATIC; usar
        // EnumChildWindows recursivo pode apanhar o controlo errado.
        return FindWindowEx(parent, IntPtr.Zero, "Edit", null);
    }

    public static bool RequestLifecycleProbeReopen(IntPtr parent) {
        // WM_APP + 0x4E. O NeuralIA ignora este comando fora do modo de probe.
        const uint WM_LIFECYCLE_PROBE_REOPEN = 0x8000 + 0x4E;
        return PostMessage(parent, WM_LIFECYCLE_PROBE_REOPEN, IntPtr.Zero, IntPtr.Zero);
    }

    public static bool ReturnHomeViaNativeEscape(IntPtr parent) {
        var edit = FindOmniboxEdit(parent);
        if (edit == IntPtr.Zero) return false;
        const uint WM_KEYDOWN = 0x0100;
        const uint WM_KEYUP = 0x0101;
        if (!PostMessage(edit, WM_KEYDOWN, new IntPtr(27), IntPtr.Zero)) return false;
        PostMessage(edit, WM_KEYUP, new IntPtr(27), IntPtr.Zero);
        return true;
    }
}
"@

function Get-VisibleWebViewSurfaceRects([IntPtr]$Parent) {
    return @(
        [NeuraliaCycleWindowProbe]::VisibleWryWebViewRects($Parent) |
        Sort-Object -Unique
    )
}

function Wait-ForNoVisibleWebSurfaces([System.Diagnostics.Process]$Process, [int]$TimeoutSec) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $Process.Refresh()
        if ($Process.HasExited) { return 0 }
        $count = @(Get-VisibleWebViewSurfaceRects -Parent ([IntPtr]$Process.MainWindowHandle)).Count
        if ($count -eq 0) { return 0 }
        Start-Sleep -Milliseconds 150
    }
    $Process.Refresh()
    return @(Get-VisibleWebViewSurfaceRects -Parent ([IntPtr]$Process.MainWindowHandle)).Count
}

function Submit-LifecycleProbeQuery([System.Diagnostics.Process]$Process) {
    $Process.Refresh()
    if ($Process.HasExited) {
        throw "NeuralIA saiu antes de reabrir o comparador."
    }
    $ok = [NeuraliaCycleWindowProbe]::RequestLifecycleProbeReopen(
        [IntPtr]$Process.MainWindowHandle
    )
    if (-not $ok) {
        throw "Falhou ao enfileirar o comando Win32 de reabertura do lifecycle."
    }
}

function Return-LifecycleProbeHome([System.Diagnostics.Process]$Process) {
    $Process.Refresh()
    if ($Process.HasExited) {
        throw "NeuralIA saiu antes de regressar a Home."
    }
    $ok = [NeuraliaCycleWindowProbe]::ReturnHomeViaNativeEscape(
        [IntPtr]$Process.MainWindowHandle
    )
    if (-not $ok) {
        throw "Omnibox nativa nao encontrada para enviar Escape e regressar a Home."
    }
}

function Wait-ForVisibleWebSurfaces([System.Diagnostics.Process]$Process, [int]$Expected, [int]$TimeoutSec) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $Process.Refresh()
        if ($Process.HasExited) { return 0 }
        $count = @(Get-VisibleWebViewSurfaceRects -Parent ([IntPtr]$Process.MainWindowHandle)).Count
        if ($count -ge $Expected) { return $count }
        Start-Sleep -Milliseconds 150
    }
    $Process.Refresh()
    return @(Get-VisibleWebViewSurfaceRects -Parent ([IntPtr]$Process.MainWindowHandle)).Count
}

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
$env:NEURALIA_LIFECYCLE_PROBE = "1"

# O monitor do Gmail e uma excepcao INTENCIONAL ao "zero WebViews na Home": ele
# nasce quando existe sessao Google no perfil WebView2 e fica vivo mesmo depois
# de voltar a Home, por desenho. Este gate conta processos, nao intencoes, por
# isso sem esta variavel ele falharia em qualquer maquina com sessao Google --
# no CI passa por acaso, porque o runner e limpo e nunca tem sessao.
$env:NEURALIA_NO_GMAIL = "1"

$process = Start-Process -FilePath $resolved -PassThru
$failures = New-Object System.Collections.ArrayList
$samples = New-Object System.Collections.ArrayList
$webViewPoolCeiling = $null
$webViewPoolWarmupCycles = 2
$warmWorkingSetMiB = $null

try {
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
        # O primeiro comparador abre por NEURALIA_STARTUP_INPUT. Os seguintes
        # so reabrem DEPOIS de este script observar Home sem WRY_WEBVIEW.
        # Nao dependemos de foco global, AppActivate ou SendKeys.
        $opened = Wait-ForVisibleWebSurfaces -Process $process -Expected 3 -TimeoutSec $OpenTimeoutSec
        if ($opened -lt 3) {
            $null = $failures.Add("ciclo ${cycle}: comparador abriu apenas ${opened} container(s) WRY_WEBVIEW visivel(is)")
        }

        # Envia Escape ao EDIT nativo da omnibox. O controlo pode estar oculto
        # por baixo dos WebViews, mas a sua subclass continua a ser o caminho
        # real do produto: WM_KEYDOWN(ESC) -> HomeRequested -> show_home().
        Return-LifecycleProbeHome -Process $process

        # A Home precisa ficar sem nenhum container WRY_WEBVIEW visivel.
        # Este e o HWND hospedeiro criado e controlado pelo WRY; as HWNDs
        # internas do Chromium podem continuar com WS_VISIBLE mesmo quando o
        # controller/host esta oculto, portanto nao servem como autoridade.
        $visibleAfterHome = Wait-ForNoVisibleWebSurfaces -Process $process -TimeoutSec $CloseTimeoutSec
        if ($visibleAfterHome -ne 0) {
            $null = $failures.Add("ciclo ${cycle}: ${visibleAfterHome} superficie(s) WebView continuaram visiveis depois de voltar a Home")
        }

        # Mede memoria no estado Home, nao no meio da abertura seguinte.
        $process.Refresh()
        $postHomeMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)
        if ($cycle -eq $webViewPoolWarmupCycles) {
            # Primeiro/segundo ciclo carregam runtime e caches legitimamente.
            # Leak e crescimento persistente DEPOIS desse aquecimento.
            $warmWorkingSetMiB = $postHomeMiB
        }

        # O Edge WebView2 pode manter um pool de subprocessos para reutilizacao
        # mesmo depois de os controllers terem sido fechados. O que nao pode
        # acontecer e esse pool crescer a cada ciclo: isso sim denuncia leak.
        $pooled = Get-WebViewCount -RootId $process.Id
        if ($cycle -le $webViewPoolWarmupCycles) {
            # O runtime pode completar o seu pool entre a primeira e a segunda
            # abertura. A partir do ciclo seguinte, qualquer crescimento e
            # persistente e passa a ser regressao.
            if ($null -eq $webViewPoolCeiling -or $pooled -gt $webViewPoolCeiling) {
                $webViewPoolCeiling = $pooled
            }
        }
        elseif ($pooled -gt $webViewPoolCeiling) {
            $null = $failures.Add("ciclo ${cycle}: pool WebView2 cresceu de ${webViewPoolCeiling} para ${pooled} processo(s) depois do aquecimento")
            $webViewPoolCeiling = $pooled
        }

        $process.Refresh()
        $null = $samples.Add([ordered]@{
            cycle = $cycle
            webviews_open = $opened
            visible_surfaces_after_home = $visibleAfterHome
            webview_process_pool_after_home = $pooled
            working_set_mib = $postHomeMiB
        })

        if ($cycle -lt $Cycles -and $visibleAfterHome -eq 0) {
            Submit-LifecycleProbeQuery -Process $process
        }
    }

    $process.Refresh()
    $finalMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)
    $coldGrowthMiB = [math]::Round($finalMiB - $baselineMiB, 2)
    if ($null -eq $warmWorkingSetMiB) {
        $warmWorkingSetMiB = $baselineMiB
    }
    $growthMiB = [math]::Round($finalMiB - $warmWorkingSetMiB, 2)
    if ($growthMiB -gt $MaxWorkingSetGrowthMiB) {
        $null = $failures.Add("working set cresceu ${growthMiB} MiB depois do aquecimento (tecto ${MaxWorkingSetGrowthMiB} MiB)")
    }

    $result = [ordered]@{
        cycles = $Cycles
        webview_baseline = $script:WebViewBaseline
        baseline_working_set_mib = $baselineMiB
        warm_working_set_mib = $warmWorkingSetMiB
        final_working_set_mib = $finalMiB
        cold_working_set_growth_mib = $coldGrowthMiB
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
    $env:NEURALIA_STARTUP_INPUT = $null
    $env:NEURALIA_LIFECYCLE_PROBE = $null
    $env:NEURALIA_NO_GMAIL = $null
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
}
