param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    # Detector de bloqueio, nao orcamento de desempenho. O gate espera pelos
    # marcos do proprio app (as tres colunas construidas e os relayouts sem
    # interacao aplicados) e so entao olha para as janelas; este prazo so
    # separa "lento" de "parado". Num runner a frio o primeiro WebView2 chegou
    # a levar mais de 12 s so para a coluna 0 (runs 36169172174 e 36252161943).
    [int]$HangTimeoutSec = 180,
    # Pasta do perfil WebView2. Vazia = a do produto (%LOCALAPPDATA%). Uma
    # pasta nova obriga o WebView2 a criar o perfil de raiz: o arranque a frio.
    [string]$WebViewProfile = "",
    [string]$OutputPath = ""
)

$ErrorActionPreference = "Stop"
$resolved = (Resolve-Path $ExePath).Path

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class NeuraliaWindowProbe {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {
        public int Left, Top, Right, Bottom;
    }

    [DllImport("user32.dll")]
    public static extern bool EnumChildWindows(IntPtr hWndParent, EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    [DllImport("user32.dll")]
    public static extern IntPtr GetWindow(IntPtr hWnd, uint uCmd);

    private const uint GW_OWNER = 4;

    public static IntPtr MainWindowForProcess(int processId) {
        IntPtr found = IntPtr.Zero;
        EnumWindows(delegate(IntPtr hwnd, IntPtr data) {
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner != (uint)processId || !IsWindowVisible(hwnd)) return true;
            if (GetWindow(hwnd, GW_OWNER) != IntPtr.Zero) return true;
            found = hwnd;
            return false;
        }, IntPtr.Zero);
        return found;
    }

    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder className, int maxCount);

    private static bool IsWryWebViewHost(IntPtr hwnd) {
        var name = new StringBuilder(128);
        GetClassName(hwnd, name, name.Capacity);
        return string.Equals(name.ToString(), "WRY_WEBVIEW", StringComparison.Ordinal);
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
}
"@

function Get-VisibleRects([System.Diagnostics.Process]$Process) {
    # Conta os containers WRY_WEBVIEW reais do produto. Eles sao os HWNDs que o
    # WRY posiciona, esconde e destroi; nao as janelas internas do processo
    # msedgewebview2.
    $parent = [NeuraliaWindowProbe]::MainWindowForProcess($Process.Id)
    if ($parent -eq [IntPtr]::Zero) { return @() }
    return @([NeuraliaWindowProbe]::VisibleWryWebViewRects($parent) | Sort-Object -Unique)
}

# O log de depuracao do app (NEURALIA_DEBUG_LOG): ms desde o arranque e a
# transicao, nunca URLs nem texto. Lido com partilha, porque o app acrescenta
# linhas enquanto o gate le.
function Read-DebugLog([string]$Path) {
    if (-not (Test-Path $Path)) { return @() }
    $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
    try {
        $reader = New-Object IO.StreamReader($stream, [Text.Encoding]::UTF8)
        $text = $reader.ReadToEnd()
    } finally {
        $stream.Dispose()
    }
    # So linhas completas: a ultima pode estar a meio de ser escrita.
    $lines = $text -split "`n"
    if (-not $text.EndsWith("`n")) { $lines = $lines | Select-Object -SkipLast 1 }
    return @($lines | ForEach-Object { $_.TrimEnd("`r") } | Where-Object { $_ -ne "" })
}

$debugLog = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-first-paint-" + [Guid]::NewGuid().ToString("N") + ".log")
$env:NEURALIA_STARTUP_INPUT = "primeira pesquisa sem mouse"
$env:NEURALIA_NO_GMAIL = "1"
$env:NEURALIA_DEBUG_LOG = $debugLog
$previousProfile = $env:WEBVIEW2_USER_DATA_FOLDER
if ($WebViewProfile) {
    New-Item -ItemType Directory -Force $WebViewProfile | Out-Null
    $env:WEBVIEW2_USER_DATA_FOLDER = (Resolve-Path $WebViewProfile).Path
}

$clock = [Diagnostics.Stopwatch]::StartNew()
$process = Start-Process -FilePath $resolved -PassThru
$spawnMs = $clock.ElapsedMilliseconds
$timeline = New-Object System.Collections.Generic.List[string]
$verdict = $null
$problem = $null
$rects = @()
$windowMs = $null
$activatedMs = $null
$settledMs = $null

try {
    while ($clock.Elapsed.TotalSeconds -lt $HangTimeoutSec -and $process.MainWindowHandle -eq 0) {
        Start-Sleep -Milliseconds 50
        $process.Refresh()
        if ($process.HasExited) { throw "NeuralIA saiu antes de criar a janela (codigo $($process.ExitCode))." }
    }
    if ($process.MainWindowHandle -eq 0) { throw "Janela principal nao apareceu em $HangTimeoutSec s." }
    $windowMs = $clock.ElapsedMilliseconds

    # Deliberadamente nao mexe no rato, nao envia tecla e nao ativa a janela.
    # O defeito da 2.1.0 só acordava os paineis depois dessa interacao.
    #
    # O gate e uma relacao de ordem, nao um prazo: depois de o app dizer que
    # as tres colunas existem (activate_comparator) e que os relayouts sem
    # interacao ja correram, as tres superficies tem de estar visiveis. Antes
    # disso o numero de superficies visiveis so vai para a linha do tempo:
    # durante a construcao a coluna 0 ja tem o container WRY (nasce visivel)
    # enquanto o WebView2 ainda arranca, e a 1 e a 2 ainda nao existem.
    $lastCount = -1
    $relayoutsNeeded = 2
    while ($true) {
        $process.Refresh()
        if ($process.HasExited) { throw "NeuralIA saiu durante a primeira pesquisa (codigo $($process.ExitCode))." }

        $log = Read-DebugLog $debugLog
        $failed = @($log | Where-Object { $_ -match "open_comparator: coluna \d+ falhou" })
        if ($failed.Count -gt 0) {
            $verdict = "column-failed"
            break
        }
        $activateIndex = -1
        for ($i = 0; $i -lt $log.Count; $i++) {
            if ($log[$i] -match "activate_comparator: 3 coluna\(s\)") { $activateIndex = $i; break }
        }
        $relayouts = 0
        if ($activateIndex -ge 0) {
            if ($null -eq $activatedMs) { $activatedMs = $clock.ElapsedMilliseconds }
            $relayouts = @($log | Select-Object -Skip ($activateIndex + 1) | Where-Object { $_ -match "RelayoutComparator: aplicado" }).Count
        }

        # A amostra e tirada DEPOIS de ler o log: se o log ja dizia que os
        # relayouts correram, o estado das janelas que se le agora e o que o
        # utilizador tem sem tocar em nada.
        $rects = @(Get-VisibleRects $process)
        if ($rects.Count -ne $lastCount) {
            $timeline.Add(("{0,7} ms  {1} superficie(s) visivel(is): {2}" -f $clock.ElapsedMilliseconds, $rects.Count, ($rects -join " ")))
            $lastCount = $rects.Count
        }

        if ($activateIndex -ge 0 -and $relayouts -ge $relayoutsNeeded) {
            $settledMs = $clock.ElapsedMilliseconds
            $verdict = if ($rects.Count -ge 3) { "ok" } else { "not-visible" }
            break
        }
        if ($clock.Elapsed.TotalSeconds -ge $HangTimeoutSec) {
            $verdict = "hang"
            break
        }
        Start-Sleep -Milliseconds 100
    }
}
catch {
    # Sem janela ou processo morto: o veredito sai na mesma com a linha do
    # tempo e o log do app, que e o que diz onde parou.
    $verdict = "error"
    $problem = $_.Exception.Message
}
finally {
    $env:NEURALIA_STARTUP_INPUT = $null
    $env:NEURALIA_NO_GMAIL = $null
    $env:NEURALIA_DEBUG_LOG = $null
    $env:WEBVIEW2_USER_DATA_FOLDER = $previousProfile
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $process.WaitForExit(10000) | Out-Null
    }
}

$appLog = Read-DebugLog $debugLog
Remove-Item $debugLog -ErrorAction SilentlyContinue
$result = [ordered]@{
    verdict = $verdict
    problem = $problem
    distinct_visible_wry_webview_rects = $rects.Count
    rects = $rects
    mouse_or_keyboard_injected = $false
    spawn_ms = $spawnMs
    window_ms = $windowMs
    comparator_activated_ms = $activatedMs
    relayouts_settled_ms = $settledMs
    cold_profile = [bool]$WebViewProfile
    timeline = @($timeline)
    app_log = @($appLog)
}
$json = $result | ConvertTo-Json -Depth 4
$json | Write-Host
if ($OutputPath) { $json | Set-Content -Encoding utf8 $OutputPath }

switch ($verdict) {
    "ok" { }
    "not-visible" {
        throw "Primeira pesquisa: com as 3 colunas construidas e os relayouts sem interacao aplicados, so $($rects.Count) superficie(s) WebView visivel(is); eram esperadas pelo menos 3."
    }
    "column-failed" {
        throw "Primeira pesquisa: o WebView2 recusou uma coluna (ver app_log)."
    }
    "error" {
        throw "Primeira pesquisa: $problem"
    }
    default {
        throw "Primeira pesquisa: o comparador nao ficou pronto em $HangTimeoutSec s (ver timeline e app_log: qual coluna ficou por construir)."
    }
}
