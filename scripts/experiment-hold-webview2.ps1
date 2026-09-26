# EXPERIMENT ONLY (branch ci/first-paint-probe, never merged).
# Fault injection on the external dependency, not on the product: every
# msedgewebview2 process that appears is suspended until HoldSec seconds after
# the first one appeared, then all are resumed. It stands in for a WebView2
# cold start slower than the old 12 s budget.
param(
    [Parameter(Mandatory = $true)] [int]$HoldSec,
    [Parameter(Mandatory = $true)] [string]$ReadyFile,
    [Parameter(Mandatory = $true)] [string]$ReportFile,
    [int]$GiveUpSec = 120
)
$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class HoldWebView2 {
    [DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr handle);
    [DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr handle);
}
"@
Set-Content -Path $ReadyFile -Value "ready"
$held = @{}
$first = $null
$clock = [Diagnostics.Stopwatch]::StartNew()
while ($clock.Elapsed.TotalSeconds -lt $GiveUpSec) {
    foreach ($p in @(Get-Process msedgewebview2 -ErrorAction SilentlyContinue)) {
        if ($held.ContainsKey($p.Id)) { continue }
        try {
            [HoldWebView2]::NtSuspendProcess($p.Handle) | Out-Null
            $held[$p.Id] = $p
            if ($null -eq $first) { $first = $clock.ElapsedMilliseconds }
        } catch { }
    }
    if ($null -ne $first -and ($clock.ElapsedMilliseconds - $first) -ge ($HoldSec * 1000)) { break }
    Start-Sleep -Milliseconds 10
}
foreach ($p in $held.Values) {
    try { [HoldWebView2]::NtResumeProcess($p.Handle) | Out-Null } catch { }
}
Set-Content -Path $ReportFile -Value ("held {0} msedgewebview2 process(es) for {1} ms (first seen at {2} ms)" -f $held.Count, ($clock.ElapsedMilliseconds - $first), $first)
