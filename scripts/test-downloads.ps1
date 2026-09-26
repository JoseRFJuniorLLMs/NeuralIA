<#
E2E do gestor de downloads (downloads-manager, plano 2.3). SO CI: abre o
exe (janelas que podem ficar com o foco). Nenhum pedido sai da maquina: a
fixture (scripts/downloads-fixture.mjs) escuta so em 127.0.0.1.

Tres arranques do exe, cada um com uma pasta de dados e uma pasta de
downloads novas (a pasta vai no downloads-settings.json, o que exercita o
SetDefaultDownloadFolderPath), e o NEURALIA_STARTUP_INPUT a abrir o endereco
do download na Web completa (um endereco escrito, nao um download
automatico de uma pagina):

1. /files/setup.exe -- GATE: recusado no DownloadStarting; nenhum ficheiro na
   pasta; o downloads.json regista-o como bloqueado (programa).
2. /files/relatorio (relatorio.pdf) -- GATE: chega a pasta com a marca da Web
   (Zone.Identifier com ZoneId); o downloads.json regista-o como acabado. O
   log de depuracao diz quem escreveu a marca (o NeuralIA, Kept(Written), ou
   ja estava, Kept(AlreadyPresent)).
3. /files/lento -- SPIKE, so relata (nunca falha o passo): com o download a
   correr, a Home (a mensagem do NEURALIA_LIFECYCLE_PROBE) destroi a WebView;
   o relatorio diz se o download acabou, quem o acabou (o WebView2 sozinho,
   ou o gestor ao receber o WebViewGone) e com que motivo.

-SelfTest confere so a leitura do spike sobre registos inventados, sem abrir
nada (corre em qualquer maquina).
#>
param(
    [string]$ExePath,
    [int]$StartupSec = 120,
    [int]$SpikeWatchSec = 20,
    [switch]$SelfTest
)

$ErrorActionPreference = "Stop"

# O motivo do COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON que o Cancel() da.
$UserCanceled = 26

# A resposta do spike a partir do que o exe escreveu no log de depuracao
# (linhas pela ordem) e do que a fixture viu. `$HomeLine` e o numero de
# linhas do log quando a Home foi pedida.
function Get-SpikeVerdict([string[]]$LogLines, [object[]]$Events, [int]$HomeLine) {
    $after = @($LogLines | Select-Object -Skip $HomeLine)
    $goneAt = -1
    $interruptAt = -1
    $reason = $null
    for ($i = 0; $i -lt $after.Count; $i++) {
        if ($goneAt -lt 0 -and $after[$i] -match 'downloads: webview \d+ destruida') { $goneAt = $i }
        if ($interruptAt -lt 0 -and $after[$i] -match 'downloads: \d+ interrompido \(motivo (\d+)\)') {
            $interruptAt = $i
            $reason = [int]$Matches[1]
        }
    }
    $aborted = @($Events | Where-Object { $_.kind -eq 'aborted' -and $_.path -eq '/files/lento' })
    $finished = @($Events | Where-Object { $_.kind -eq 'finished' -and $_.path -eq '/files/lento' })
    $handler = if ($goneAt -ge 0) {
        "o handler do DownloadStarting foi largado (WebViewGone chegou ao gestor)"
    } else {
        "o handler do DownloadStarting NAO foi largado: o WebViewGone nunca chegou ao gestor"
    }
    if ($interruptAt -ge 0 -and $reason -ne $UserCanceled) {
        return [pscustomobject]@{
            Answer = "SIM, pelo WebView2"
            Detail = "destruir a WebView interrompeu o download sozinho (motivo $reason, nao o Cancel do gestor); $handler"
        }
    }
    if ($interruptAt -ge 0 -and $goneAt -ge 0 -and $interruptAt -lt $goneAt) {
        return [pscustomobject]@{
            Answer = "SIM, pelo WebView2"
            Detail = "o download acabou (motivo $reason) antes de o WebViewGone chegar ao gestor; $handler"
        }
    }
    if ($interruptAt -ge 0) {
        return [pscustomobject]@{
            Answer = "SIM, pelo gestor"
            Detail = "so o Cancel do gestor ao receber o WebViewGone acabou o download (motivo $reason = USER_CANCELED); o WebView2 sozinho nao o tinha acabado; $handler"
        }
    }
    if ($aborted.Count -gt 0) {
        return [pscustomobject]@{
            Answer = "SIM, sem StateChanged"
            Detail = "a fixture viu a ligacao fechar ($($aborted[0].sent) bytes enviados), sem nenhum StateChanged no exe; $handler"
        }
    }
    if ($finished.Count -gt 0) {
        return [pscustomobject]@{
            Answer = "NAO"
            Detail = "o download chegou ao fim depois de a WebView ser destruida; $handler"
        }
    }
    return [pscustomobject]@{
        Answer = "NAO"
        Detail = "o download continuou a correr depois de a WebView ser destruida (nenhum fim na janela de observacao); $handler"
    }
}

if ($SelfTest) {
    $lento = { param($kind) [pscustomobject]@{ kind = $kind; path = '/files/lento'; sent = 65536 } }
    $cases = @(
        @{ Log = @('a', 'downloads: 1 interrompido (motivo 27)', 'downloads: webview 1 destruida'); Events = @(& $lento 'aborted'); Want = 'SIM, pelo WebView2' },
        @{ Log = @('a', 'downloads: webview 1 destruida', 'downloads: 1 interrompido (motivo 26)'); Events = @(& $lento 'aborted'); Want = 'SIM, pelo gestor' },
        @{ Log = @('a', 'downloads: 1 interrompido (motivo 26)', 'downloads: webview 1 destruida'); Events = @(); Want = 'SIM, pelo WebView2' },
        @{ Log = @('a', 'downloads: webview 1 destruida'); Events = @(& $lento 'aborted'); Want = 'SIM, sem StateChanged' },
        @{ Log = @('a'); Events = @(& $lento 'finished'); Want = 'NAO' },
        @{ Log = @('a'); Events = @(); Want = 'NAO' }
    )
    foreach ($case in $cases) {
        $verdict = Get-SpikeVerdict $case.Log $case.Events 1
        if ($verdict.Answer -ne $case.Want) {
            throw "SelfTest: '$($case.Log -join ' | ')' deu '$($verdict.Answer)', esperado '$($case.Want)'."
        }
    }
    # Linhas de antes da Home nao contam.
    $old = Get-SpikeVerdict @('downloads: 1 interrompido (motivo 27)', 'b') @() 1
    if ($old.Answer -ne 'NAO') { throw "SelfTest: uma linha de antes da Home contou ($($old.Answer))." }
    Write-Host "test-downloads SelfTest: $($cases.Count + 1) casos do spike ok."
    exit 0
}

if (-not $ExePath) { throw "-ExePath e obrigatorio (ou -SelfTest)." }
$resolved = (Resolve-Path -LiteralPath $ExePath).Path
$node = (Get-Command node -ErrorAction Stop).Source

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class NeuraliaDownloadsProbe {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumChildWindows(IntPtr hWndParent, EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder className, int maxCount);

    [DllImport("user32.dll")]
    public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern uint RegisterWindowMessage(string lpString);

    // A Home do NEURALIA_LIFECYCLE_PROBE, como o measure-cycles.ps1 a pede:
    // a mensagem registada vai para a omnibox (EDIT) e para as janelas de
    // topo do processo; o exe filtra o nonce repetido.
    public static bool RequestHome(int processId, int nonce) {
        var message = RegisterWindowMessage("NeuralIA.LifecycleProbe.Home");
        if (message == 0) return false;
        bool posted = false;
        var cls = new StringBuilder(128);
        EnumWindows(delegate(IntPtr top, IntPtr data) {
            uint owner;
            GetWindowThreadProcessId(top, out owner);
            if (owner != (uint)processId) return true;
            EnumChildWindows(top, delegate(IntPtr child, IntPtr childData) {
                cls.Clear();
                GetClassName(child, cls, cls.Capacity);
                if (string.Equals(cls.ToString(), "Edit", StringComparison.Ordinal)) {
                    if (PostMessage(child, message, new IntPtr(nonce), IntPtr.Zero)) posted = true;
                }
                return true;
            }, IntPtr.Zero);
            if (PostMessage(top, message, new IntPtr(nonce), IntPtr.Zero)) posted = true;
            return true;
        }, IntPtr.Zero);
        return posted;
    }
}
"@

$root = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-downloads-e2e-" + [guid]::NewGuid().ToString("N"))
$fixtureDir = Join-Path $root "fixture"
New-Item -ItemType Directory -Force -Path $fixtureDir | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)

function Wait-Until([scriptblock]$Condition, [int]$Seconds) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $Seconds) {
        $value = & $Condition
        if ($value) { return $value }
        Start-Sleep -Milliseconds 250
    }
    return & $Condition
}

function Get-FixtureEvents {
    $file = Join-Path $fixtureDir "events.jsonl"
    if (-not (Test-Path -LiteralPath $file)) { return @() }
    return @([IO.File]::ReadAllLines($file) | Where-Object { $_ } | ForEach-Object { $_ | ConvertFrom-Json })
}

function Get-LogLines($Run) {
    if (-not (Test-Path -LiteralPath $Run.Log)) { return @() }
    try { return @([IO.File]::ReadAllLines($Run.Log)) } catch { return @() }
}

# O registo do downloads.json de `name`, ou $null. O ficheiro e gravado por
# rename: uma leitura a meio falha e tenta-se outra vez.
function Get-Record($Run, [string]$Name) {
    $file = Join-Path $Run.Data "downloads.json"
    if (-not (Test-Path -LiteralPath $file)) { return $null }
    try {
        $json = [IO.File]::ReadAllText($file) | ConvertFrom-Json
    } catch {
        return $null
    }
    return @($json.data.entries | Where-Object { $_.name -eq $Name }) | Select-Object -First 1
}

function Start-Run([string]$Name, [string]$Url, [switch]$Probe) {
    $data = Join-Path $root "$Name-data"
    $dl = Join-Path $root "$Name-downloads"
    New-Item -ItemType Directory -Force -Path $data, $dl | Out-Null
    $settings = [ordered]@{ version = 1; data = [ordered]@{ folder = $dl; allow_programs = $false } }
    [IO.File]::WriteAllText((Join-Path $data "downloads-settings.json"), ($settings | ConvertTo-Json -Depth 4), $utf8)
    $log = Join-Path $root "$Name-debug.log"
    $env:NEURALIA_DATA_DIR = $data
    $env:NEURALIA_DEBUG_LOG = $log
    # `web:` abre a Web completa (um endereco http sem prefixo iria para o
    # Modo Leitura, que nao descarrega nada).
    $env:NEURALIA_STARTUP_INPUT = "web:$Url"
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_REDUCE_MOTION = "1"
    $env:NEURALIA_LIFECYCLE_PROBE = if ($Probe) { "1" } else { $null }
    try {
        $process = Start-Process -FilePath $resolved -PassThru
    } finally {
        foreach ($name in "NEURALIA_DATA_DIR", "NEURALIA_DEBUG_LOG", "NEURALIA_STARTUP_INPUT", "NEURALIA_NO_GMAIL", "NEURALIA_REDUCE_MOTION", "NEURALIA_LIFECYCLE_PROBE") {
            Remove-Item -Path "Env:$name" -ErrorAction SilentlyContinue
        }
    }
    Write-Host "[$Name] exe $($process.Id) abre $Url"
    return [pscustomobject]@{ Name = $Name; Process = $process; Data = $data; Dl = $dl; Log = $log }
}

function Stop-Run($Run) {
    if ($null -eq $Run) { return }
    if (-not $Run.Process.HasExited) {
        Stop-Process -Id $Run.Process.Id -Force -ErrorAction SilentlyContinue
        $null = $Run.Process.WaitForExit(15000)
    }
}

function Show-Diagnostics($Run) {
    Write-Host "[$($Run.Name)] log de depuracao (fim):"
    Get-LogLines $Run | Select-Object -Last 40 | ForEach-Object { Write-Host "  $_" }
    Write-Host "[$($Run.Name)] pasta de downloads:"
    Get-ChildItem -LiteralPath $Run.Dl -Force -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "  $($_.Name) ($($_.Length) bytes)" }
    Write-Host "[$($Run.Name)] eventos da fixture:"
    Get-FixtureEvents | ForEach-Object { Write-Host "  $($_ | ConvertTo-Json -Compress)" }
}

function Assert-Requested([string]$Path, $Run) {
    $seen = Wait-Until {
        if ($Run.Process.HasExited) { throw "[$($Run.Name)] o exe saiu (codigo $($Run.Process.ExitCode)) antes de pedir $Path." }
        @(Get-FixtureEvents | Where-Object { $_.kind -eq 'request' -and $_.path -eq $Path }).Count -gt 0
    } $StartupSec
    if (-not $seen) { throw "[$($Run.Name)] o exe nao pediu $Path em $StartupSec s." }
}

$fixture = $null
$failures = New-Object System.Collections.Generic.List[string]
$summary = New-Object System.Collections.Generic.List[string]
try {
    $fixture = Start-Process -FilePath $node -ArgumentList @((Join-Path $PSScriptRoot "downloads-fixture.mjs"), $fixtureDir) -PassThru -WindowStyle Hidden
    $portFile = Join-Path $fixtureDir "port"
    if (-not (Wait-Until { Test-Path -LiteralPath $portFile } 15)) { throw "a fixture nao arrancou." }
    $base = "http://127.0.0.1:" + ([IO.File]::ReadAllText($portFile).Trim())

    # 1. GATE: setup.exe recusado, nada no disco, registado como bloqueado.
    $run = Start-Run "setup" "$base/files/setup.exe"
    try {
        Assert-Requested "/files/setup.exe" $run
        $record = Wait-Until { Get-Record $run "setup.exe" } 30
        if ($null -eq $record) {
            $failures.Add("setup.exe: o downloads.json nao o registou em 30 s")
        } elseif ($record.outcome.kind -ne 'blocked' -or $record.outcome.reason -ne 'program') {
            $failures.Add("setup.exe: registado como $($record.outcome | ConvertTo-Json -Compress), esperado blocked/program")
        }
        # O cancelamento e no DownloadStarting: nem o .crdownload chega a existir.
        Start-Sleep -Seconds 3
        $left = @(Get-ChildItem -LiteralPath $run.Dl -Force -ErrorAction SilentlyContinue)
        if ($left.Count -gt 0) {
            $failures.Add("setup.exe: a pasta de downloads tem $($left.Name -join ', ')")
        }
        $summary.Add("setup.exe: recusado ($(if ($record) { $record.outcome | ConvertTo-Json -Compress } else { 'sem registo' })), pasta com $($left.Count) ficheiro(s)")
        if ($failures.Count -gt 0) { Show-Diagnostics $run }
    } finally {
        Stop-Run $run
    }

    # 2. GATE: o PDF chega com a marca da Web.
    $before = $failures.Count
    $run = Start-Run "pdf" "$base/files/relatorio"
    try {
        Assert-Requested "/files/relatorio" $run
        $record = Wait-Until {
            $found = Get-Record $run "relatorio.pdf"
            if ($found -and $found.outcome.kind -ne 'running') { $found }
        } 60
        $file = Join-Path $run.Dl "relatorio.pdf"
        if ($null -eq $record) {
            $failures.Add("relatorio.pdf: o downloads.json nao o registou em 60 s")
        } elseif ($record.outcome.kind -ne 'completed') {
            $failures.Add("relatorio.pdf: registado como $($record.outcome | ConvertTo-Json -Compress), esperado completed")
        }
        $zone = $null
        if (-not (Test-Path -LiteralPath $file)) {
            $failures.Add("relatorio.pdf: nao esta na pasta escolhida ($($run.Dl))")
        } else {
            $zone = Get-Content -LiteralPath $file -Stream Zone.Identifier -Raw -ErrorAction SilentlyContinue
            if (-not $zone -or $zone -notmatch '(?m)^\s*ZoneId\s*=\s*(\d+)') {
                $failures.Add("relatorio.pdf: sem Zone.Identifier (a marca da Web)")
            }
        }
        $zoneId = if ($zone -and $zone -match '(?m)^\s*ZoneId\s*=\s*(\d+)') { $Matches[1] } else { 'nenhum' }
        $writer = switch -Regex ((Get-LogLines $run) -join "`n") {
            'acabou \(Kept\(Written\)\)' { 'o NeuralIA'; break }
            'acabou \(Kept\(AlreadyPresent\)\)' { 'o WebView2 (o NeuralIA nao a reescreveu)'; break }
            default { 'desconhecido' }
        }
        $summary.Add("relatorio.pdf: $(if ($record) { $record.outcome.kind } else { 'sem registo' }), ZoneId=$zoneId, marca escrita por $writer")
        if ($failures.Count -gt $before) { Show-Diagnostics $run }
    } finally {
        Stop-Run $run
    }

    # 3. SPIKE: destruir a WebView cancela o download dela? So relata.
    $run = Start-Run "spike" "$base/files/lento" -Probe
    try {
        Assert-Requested "/files/lento" $run
        $started = Wait-Until { (Get-LogLines $run) -match 'downloads: \d+ comecou' } 30
        if (-not $started) { throw "o download lento nao comecou no exe." }
        Start-Sleep -Seconds 2
        $homeLine = (Get-LogLines $run).Count
        if (-not [NeuraliaDownloadsProbe]::RequestHome($run.Process.Id, 1)) {
            throw "nao deu para pedir a Home ao exe."
        }
        Write-Host "[spike] Home pedida com o download a correr; a observar $SpikeWatchSec s."
        $null = Wait-Until {
            @(Get-FixtureEvents | Where-Object { $_.path -eq '/files/lento' -and $_.kind -in 'aborted', 'finished' }).Count -gt 0
        } $SpikeWatchSec
        Start-Sleep -Seconds 2
        $verdict = Get-SpikeVerdict (Get-LogLines $run) (Get-FixtureEvents) $homeLine
        $line = "DOWNLOADS SPIKE (destruir a WebView cancela o download dela?): $($verdict.Answer) -- $($verdict.Detail)"
        Write-Host $line
        $summary.Add($line)
        Show-Diagnostics $run
    } catch {
        $line = "DOWNLOADS SPIKE: sem resposta ($($_.Exception.Message))"
        Write-Host $line
        $summary.Add($line)
        Show-Diagnostics $run
    } finally {
        Stop-Run $run
    }
} finally {
    if ($fixture -and -not $fixture.HasExited) {
        Stop-Process -Id $fixture.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}

foreach ($line in $summary) { Write-Host $line }
if ($env:GITHUB_STEP_SUMMARY) {
    $lines = @("### Downloads E2E") + @($summary | ForEach-Object { "- $_" })
    [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, ($lines -join "`n") + "`n", $utf8)
}
if ($failures.Count -gt 0) {
    throw "Downloads E2E: $($failures.Count) gate(s) falharam:`n$($failures -join "`n")"
}
Write-Host "Downloads E2E: setup.exe recusado e PDF com a marca da Web no exe testado."
