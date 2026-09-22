<#
.SYNOPSIS
    Gate de ciclo de vida: Comparar -> Home, N vezes, contando WebViews.

.DESCRIPTION
    O gate de arranque (measure-home.ps1) mede a Home em repouso e por isso nao
    detectaria o pior defeito que ja existiu aqui: os WebViews do comparador
    sobreviverem ao regresso a Home. Este script fecha esse buraco.

    Para cada ciclo:
      startup/omnibox nativa -> o comparador abre e aparecem containers WRY_WEBVIEW
      o comparador só conta como aberto quando o app sinaliza fim da construção
      Home -> nenhum container WRY_WEBVIEW pode continuar visivel
      o script pede HOME e REOPEN por mensagens Win32 privadas habilitadas
      somente em NEURALIA_LIFECYCLE_PROBE; cada reopen só ocorre após Home limpa

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

# Os comandos Home/Reopen usam RegisterWindowMessage no processo do gate e
# no NeuralIA. Isso evita colisões com IDs privados de WM_APP usados por
# winit/WRY/WebView2 e mantém o transporte independente de foco/teclado.
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
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

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
    public static extern IntPtr GetWindow(IntPtr hWnd, uint uCmd);

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder className, int maxCount);

    private static bool IsWryWebViewHost(IntPtr hwnd) {
        var name = new StringBuilder(128);
        GetClassName(hwnd, name, name.Capacity);
        return string.Equals(name.ToString(), "WRY_WEBVIEW", StringComparison.Ordinal);
    }

    [return: MarshalAs(UnmanagedType.Bool)]
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern uint RegisterWindowMessage(string lpString);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern IntPtr SendMessageTimeout(
        IntPtr hWnd,
        uint msg,
        IntPtr wParam,
        IntPtr lParam,
        uint flags,
        uint timeout,
        out IntPtr result
    );

    public static IntPtr MainWindowForProcess(int processId) {
        IntPtr best = IntPtr.Zero;
        long bestArea = -1;
        EnumWindows(delegate(IntPtr hwnd, IntPtr data) {
            uint ownerPid;
            GetWindowThreadProcessId(hwnd, out ownerPid);
            if (ownerPid != (uint)processId || !IsWindowVisible(hwnd)) return true;
            RECT r;
            if (!GetWindowRect(hwnd, out r)) return true;
            long width = Math.Max(0, r.Right - r.Left);
            long height = Math.Max(0, r.Bottom - r.Top);
            long area = width * height;
            if (area > bestArea) {
                bestArea = area;
                best = hwnd;
            }
            return true;
        }, IntPtr.Zero);
        return best;
    }

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

    public static bool RequestLifecycleProbeHome(IntPtr parent, int nonce) {
        var message = RegisterWindowMessage("NeuralIA.LifecycleProbe.Home");
        if (message == 0) return false;

        uint processId;
        GetWindowThreadProcessId(parent, out processId);
        if (processId == 0) return false;

        // Alternar a decoração do comparador pode substituir/reparentar o HWND
        // externo. Não confie no "main window" escolhido pelo runner: procure o
        // EDIT da omnibox em TODAS as janelas top-level do processo, inclusive
        // as temporariamente ocultas, e entregue a mensagem à sua subclass.
        // Isso só melhora o transporte do probe; o sucesso do gate continua
        // exigindo externamente zero hosts WRY_WEBVIEW depois de HomeRequested.
        bool postedToEdit = false;
        var editClass = new StringBuilder(128);
        EnumWindows(delegate(IntPtr top, IntPtr data) {
            uint ownerPid;
            GetWindowThreadProcessId(top, out ownerPid);
            if (ownerPid != processId) return true;

            EnumChildWindows(top, delegate(IntPtr child, IntPtr childData) {
                editClass.Clear();
                GetClassName(child, editClass, editClass.Capacity);
                if (string.Equals(editClass.ToString(), "Edit", StringComparison.Ordinal)) {
                    if (PostMessage(child, message, new IntPtr(nonce), IntPtr.Zero)) {
                        postedToEdit = true;
                    }
                }
                return true;
            }, IntPtr.Zero);
            return true;
        }, IntPtr.Zero);

        // Mesmo quando algum EDIT foi encontrado, envie também para as
        // top-level do processo. WebView2/Chromium também cria controles EDIT;
        // considerar qualquer um deles como prova de entrega pode deixar o
        // verdadeiro omnibox de fora a partir do segundo ciclo.
        bool postedToWindow = false;
        EnumWindows(delegate(IntPtr top, IntPtr data) {
            uint ownerPid;
            GetWindowThreadProcessId(top, out ownerPid);
            if (ownerPid == processId && PostMessage(top, message, new IntPtr(nonce), IntPtr.Zero)) {
                postedToWindow = true;
            }
            return true;
        }, IntPtr.Zero);
        return postedToEdit || postedToWindow;
    }

    public static bool ReturnHomeViaNativeEscape(IntPtr parent) {
        // A omnibox principal é o único EDIT filho direto da janela principal.
        // A palette tem outro EDIT, mas vive dentro de um popup nativo.
        var edit = FindWindowEx(parent, IntPtr.Zero, "Edit", null);
        if (edit == IntPtr.Zero) return false;
        const uint WM_KEYDOWN = 0x0100;
        const uint WM_KEYUP = 0x0101;
        if (!PostMessage(edit, WM_KEYDOWN, new IntPtr(27), IntPtr.Zero)) return false;
        PostMessage(edit, WM_KEYUP, new IntPtr(27), IntPtr.Zero);
        return true;
    }

    private static uint ProcessIdOf(IntPtr anyWindow) {
        uint processId;
        GetWindowThreadProcessId(anyWindow, out processId);
        return processId;
    }

    public static bool RequestLifecycleProbeReopen(IntPtr parent, int nonce) {
        var message = RegisterWindowMessage("NeuralIA.LifecycleProbe.Reopen");
        if (message == 0) return false;
        var processId = ProcessIdOf(parent);
        if (processId == 0) return false;

        bool posted = false;
        EnumWindows(delegate(IntPtr top, IntPtr data) {
            uint ownerPid;
            GetWindowThreadProcessId(top, out ownerPid);
            if (ownerPid == processId && PostMessage(top, message, new IntPtr(nonce), IntPtr.Zero)) {
                posted = true;
            }
            return true;
        }, IntPtr.Zero);
        return posted;
    }

    private static bool ProbeFlagForProcess(IntPtr anyWindow, string name) {
        var message = RegisterWindowMessage(name);
        if (message == 0) return false;
        var processId = ProcessIdOf(anyWindow);
        if (processId == 0) return false;

        bool ready = false;
        EnumWindows(delegate(IntPtr top, IntPtr data) {
            uint ownerPid;
            GetWindowThreadProcessId(top, out ownerPid);
            if (ownerPid != processId) return true;

            IntPtr result;
            var sent = SendMessageTimeout(
                top,
                message,
                IntPtr.Zero,
                IntPtr.Zero,
                0x0002,
                250,
                out result
            );
            if (sent != IntPtr.Zero && result != IntPtr.Zero) {
                ready = true;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return ready;
    }

    public static bool LifecycleProbeReady(IntPtr parent) {
        return ProbeFlagForProcess(parent, "NeuralIA.LifecycleProbe.Ready");
    }

    public static bool LifecycleProbeHomeReady(IntPtr parent) {
        return ProbeFlagForProcess(parent, "NeuralIA.LifecycleProbe.HomeReady");
    }
}
"@

function Get-NeuraliaMainWindow([System.Diagnostics.Process]$Process) {
    $Process.Refresh()
    if ($Process.HasExited) { return [IntPtr]::Zero }
    $hwnd = [NeuraliaCycleWindowProbe]::MainWindowForProcess($Process.Id)
    if ($hwnd -eq [IntPtr]::Zero) {
        $hwnd = [IntPtr]$Process.MainWindowHandle
    }
    return $hwnd
}

function Get-CurrentMainWindow([System.Diagnostics.Process]$Process) {
    $Process.Refresh()
    if ($Process.HasExited) { return [IntPtr]::Zero }
    return [NeuraliaCycleWindowProbe]::MainWindowForProcess($Process.Id)
}

function Get-VisibleWebViewSurfaceRects([System.Diagnostics.Process]$Process) {
    $parent = Get-CurrentMainWindow -Process $Process
    if ($parent -eq [IntPtr]::Zero) { return @() }
    return @(
        [NeuraliaCycleWindowProbe]::VisibleWryWebViewRects($parent) |
        Sort-Object -Unique
    )
}

function Wait-ForNoVisibleWebSurfaces([System.Diagnostics.Process]$Process, [int]$TimeoutSec) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $Process.Refresh()
        if ($Process.HasExited) { return 0 }
        $count = @(Get-VisibleWebViewSurfaceRects -Process $Process).Count
        if ($count -eq 0) { return 0 }
        Start-Sleep -Milliseconds 150
    }
    $Process.Refresh()
    return @(Get-VisibleWebViewSurfaceRects -Process $Process).Count
}

function Submit-LifecycleProbeQuery([System.Diagnostics.Process]$Process, [int]$Nonce) {
    $Process.Refresh()
    if ($Process.HasExited) {
        throw "NeuralIA saiu antes de reabrir o comparador."
    }
    $parent = Get-CurrentMainWindow -Process $Process
    if ($parent -eq [IntPtr]::Zero) { throw "Janela principal atual do NeuralIA não foi encontrada." }
    $ok = [NeuraliaCycleWindowProbe]::RequestLifecycleProbeReopen($parent, $Nonce)
    if (-not $ok) {
        throw "Falhou ao enfileirar o comando Win32 de reabertura do lifecycle."
    }
}

function Return-LifecycleProbeHome([System.Diagnostics.Process]$Process, [int]$Nonce) {
    $Process.Refresh()
    if ($Process.HasExited) {
        throw "NeuralIA saiu antes de regressar a Home."
    }
    $parent = Get-CurrentMainWindow -Process $Process
    if ($parent -eq [IntPtr]::Zero) { throw "Janela principal atual do NeuralIA não foi encontrada." }

    # O gate mede teardown, não entrega de teclado. Use a mensagem Win32
    # privada do modo NEURALIA_LIFECYCLE_PROBE em todos os ciclos para disparar
    # exatamente HomeRequested no event loop do produto. O caminho real de ESC
    # já é coberto separadamente; misturá-lo aqui tornou o ciclo 2+ dependente
    # do estado/foco do EDIT oculto e produziu falso negativo no runner.
    $ok = [NeuraliaCycleWindowProbe]::RequestLifecycleProbeHome($parent, $Nonce)
    if (-not $ok) {
        throw "Falhou ao enfileirar o retorno determinístico à Home."
    }
}

function Wait-ForVisibleWebSurfaces([System.Diagnostics.Process]$Process, [int]$Expected, [int]$TimeoutSec) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $Process.Refresh()
        if ($Process.HasExited) { return 0 }
        $count = @(Get-VisibleWebViewSurfaceRects -Process $Process).Count
        if ($count -ge $Expected) { return $count }
        Start-Sleep -Milliseconds 150
    }
    $Process.Refresh()
    return @(Get-VisibleWebViewSurfaceRects -Process $Process).Count
}

function Wait-ForLifecycleProbeReady([System.Diagnostics.Process]$Process, [int]$TimeoutSec) {
    # Tres hosts visiveis provam a geometria, mas NAO provam que a janela
    # nativa terminou a troca de HWND/decorations. O ciclo 2 mostrou exatamente
    # isso: aceitar 750 ms de estabilidade deixava o gate enviar Home para um
    # HWND sem a subclass do NeuralIA. Agora Ready é obrigatório.
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $Process.Refresh()
        if ($Process.HasExited) { return $false }

        $visible = @(Get-VisibleWebViewSurfaceRects -Process $Process).Count
        if ($visible -ge 3) {
            $main = Get-NeuraliaMainWindow -Process $Process
            if ($main -ne [IntPtr]::Zero -and [NeuraliaCycleWindowProbe]::LifecycleProbeReady($main)) {
                return $true
            }
        }

        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Wait-ForLifecycleProbeHomeReady([System.Diagnostics.Process]$Process, [int]$TimeoutSec) {
    # Zero hosts WRY prova o teardown. Este segundo handshake prova que a
    # transição de decorations/HWND da Home também terminou antes do próximo
    # reopen. Sem ele o ciclo 2 podia começar dentro do teardown do ciclo 1.
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $Process.Refresh()
        if ($Process.HasExited) { return $false }
        $main = Get-NeuraliaMainWindow -Process $Process
        if ($main -ne [IntPtr]::Zero -and [NeuraliaCycleWindowProbe]::LifecycleProbeHomeReady($main)) {
            return $true
        }
        Start-Sleep -Milliseconds 100
    }
    return $false
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
$webViewPoolWarmupCycles = 3
$warmWorkingSetMiB = $null

try {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $mainWindow = [IntPtr]::Zero
    while ($watch.Elapsed.TotalSeconds -lt 10 -and $mainWindow -eq [IntPtr]::Zero) {
        Start-Sleep -Milliseconds 50
        $process.Refresh()
        if ($process.HasExited) { throw "NeuralIA saiu antes de criar a janela." }
        $mainWindow = Get-NeuraliaMainWindow -Process $process
    }
    if ($mainWindow -eq [IntPtr]::Zero) { throw "A janela nativa nao apareceu." }

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

        # WRY/WebView2 pode criar os HWNDs enquanto build_as_child ainda está
        # dentro de um pump aninhado. Mandar Home nesse instante só enfileira o
        # evento para depois e transforma um gate de teardown num teste de race.
        # O handshake fica true somente no fim de open_comparator.
        if (-not (Wait-ForLifecycleProbeReady -Process $process -TimeoutSec $OpenTimeoutSec)) {
            $null = $failures.Add("ciclo ${cycle}: comparador ficou visível mas não concluiu a abertura")
            break
        }

        # O gate comanda explicitamente a transição depois do handshake Ready.
        # Antes a própria app agendava Home/Reopen por timers internos. Isso
        # misturava duas coisas: teardown real e atraso do pump aninhado do
        # WebView2. A mensagem privada passa pelo mesmo EventLoopProxy e chama
        # exatamente HomeRequested, mas deixa o teste decidir quando medir.
        Return-LifecycleProbeHome -Process $process -Nonce $cycle

        $visibleAfterHome = Wait-ForNoVisibleWebSurfaces -Process $process -TimeoutSec $CloseTimeoutSec

        # A Home precisa ficar sem nenhum container WRY_WEBVIEW visivel.
        # Este e o HWND hospedeiro criado e controlado pelo WRY; as HWNDs
        # internas do Chromium podem continuar com WS_VISIBLE mesmo quando o
        # controller/host esta oculto, portanto nao servem como autoridade.
        if ($visibleAfterHome -ne 0) {
            $null = $failures.Add("ciclo ${cycle}: ${visibleAfterHome} superficie(s) WebView continuaram visiveis depois de voltar a Home")
        }

        # Zero WRY prova teardown; HomeReady prova que a transição de
        # decorations/HWND terminou antes de qualquer reabertura.
        if (-not (Wait-ForLifecycleProbeHomeReady -Process $process -TimeoutSec $CloseTimeoutSec)) {
            $null = $failures.Add("ciclo ${cycle}: Home ficou sem WebView mas não concluiu RestoreHomeDecorations")
            break
        }

        Start-Sleep -Milliseconds 250

        # Mede memoria no estado Home, nao no meio da abertura seguinte.
        $process.Refresh()
        $postHomeMiB = [math]::Round($process.WorkingSet64 / 1MB, 2)
        if ($cycle -eq $webViewPoolWarmupCycles) {
            # Os três primeiros ciclos carregam runtime/processos auxiliares
            # legitimamente. Leak é crescimento persistente DEPOIS desse aquecimento.
            $warmWorkingSetMiB = $postHomeMiB
        }

        # O Edge WebView2 pode manter um pool de subprocessos para reutilizacao
        # mesmo depois de os controllers terem sido fechados. O que nao pode
        # acontecer e esse pool crescer a cada ciclo: isso sim denuncia leak.
        $pooled = Get-WebViewCount -RootId $process.Id
        if ($cycle -le $webViewPoolWarmupCycles) {
            # O runtime pode completar o pool gradualmente até a terceira
            # abertura. A partir daí, qualquer novo teto é crescimento
            # persistente e passa a ser regressão.
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

        # O próximo comparador só nasce depois de o script ter observado
        # teardown=0 e HomeReady. Isso impede que a medição do ciclo N conte
        # superfícies já pertencentes ao ciclo N+1.
        if ($cycle -lt $Cycles) {
            Submit-LifecycleProbeQuery -Process $process -Nonce ($cycle + 1)
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
