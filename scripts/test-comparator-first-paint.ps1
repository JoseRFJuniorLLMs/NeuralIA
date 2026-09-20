param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [int]$TimeoutSec = 12
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
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    [DllImport("user32.dll")]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder className, int maxCount);

    public static List<string> VisibleLargeRects(IntPtr parent) {
        var rows = new List<string>();
        EnumChildWindows(parent, delegate(IntPtr hwnd, IntPtr data) {
            if (!IsWindowVisible(hwnd)) return true;
            RECT r;
            if (!GetWindowRect(hwnd, out r)) return true;
            var width = r.Right - r.Left;
            var height = r.Bottom - r.Top;
            if (width < 180 || height < 220) return true;
            var name = new StringBuilder(256);
            GetClassName(hwnd, name, name.Capacity);
            rows.Add(r.Left + "," + r.Top + "," + r.Right + "," + r.Bottom + "|" + name.ToString());
            return true;
        }, IntPtr.Zero);
        return rows;
    }
}
"@

$env:NEURALIA_STARTUP_INPUT = "primeira pesquisa sem mouse"
$env:NEURALIA_NO_GMAIL = "1"
$process = Start-Process -FilePath $resolved -PassThru

try {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt 8 -and $process.MainWindowHandle -eq 0) {
        Start-Sleep -Milliseconds 50
        $process.Refresh()
        if ($process.HasExited) { throw "NeuralIA saiu antes de criar a janela." }
    }
    if ($process.MainWindowHandle -eq 0) { throw "Janela principal nao apareceu." }

    # Deliberadamente nao mexe no rato, nao envia tecla e nao ativa a janela.
    # O defeito da 2.1.0 só acordava os paineis depois dessa interacao.
    $rects = @()
    $watch.Restart()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $process.Refresh()
        $rows = [NeuraliaWindowProbe]::VisibleLargeRects([IntPtr]$process.MainWindowHandle)

        # Dedupe pela geometria: cada WebView2 pode ter mais de um HWND interno
        # com o mesmo rectangulo, mas as tres colunas têm rectangulos distintos.
        $rects = @(
            $rows |
            ForEach-Object { ($_ -split '\|')[0] } |
            Sort-Object -Unique
        )

        if ($rects.Count -ge 3) { break }
        Start-Sleep -Milliseconds 150
    }

    $result = [ordered]@{
        distinct_visible_large_child_rects = $rects.Count
        rects = $rects
        mouse_or_keyboard_injected = $false
    }
    $result | ConvertTo-Json -Depth 4 | Write-Host

    if ($rects.Count -lt 3) {
        throw "Primeira pesquisa abriu apenas $($rects.Count) superficie(s) WebView visivel(is) sem interacao; eram esperadas pelo menos 3."
    }
}
finally {
    $env:NEURALIA_STARTUP_INPUT = $null
    $env:NEURALIA_NO_GMAIL = $null
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
}
