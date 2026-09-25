param(
    # O exe de release compilado com `--features accel-spike`. So no CI: este
    # script abre a janela do NeuralIA e carrega em teclas (SendInput).
    [string]$ExePath,
    # A ordem importa: os quatro primeiros vivem no comparador que o
    # NEURALIA_STARTUP_INPUT abre; External, Reader, Pdf e Epub substituem-no.
    [string[]]$Hosts = @("Column", "Split", "PrivateSplit", "SidePanel", "Service", "External", "Reader", "Pdf", "Epub"),
    # Sabotagem (Handled=false no handler): o passo so e verde se a tabela
    # mostrar que a pagina viu a tecla.
    [switch]$ExpectLeak,
    # Corre so o avaliador sobre registos sinteticos (sem exe, sem janela).
    [switch]$SelfTest,
    # Quanto se espera depois de largar as teclas antes de ler a pagina.
    [int]$SettleMs = 700,
    [string]$TablePath = "accel-spike-table.md"
)

# Spike do AcceleratorKeyPressed (infra-accel-spike, plano 2.3).
#
# Para cada hospedeiro de WebView e cada atalho nativo por omissao do plano,
# uma tentativa com a tecla carregada uma vez e outra com ela presa
# (auto-repeat). Passa quando:
#   - o keydown da pagina nunca ve o atalho (a sonda do exe no documento de
#     topo e, na fixture de 127.0.0.1, os ouvintes da propria pagina);
#   - nenhum act() do NEURALIA_KEYMAP_SCRIPT dispara (os eventos IPC que o
#     exe do spike conta e engole, e a barra de procura do Ctrl+F);
#   - o lado nativo (o AcceleratorKeyPressed desse hospedeiro) dispara
#     exatamente uma vez -- e, na tecla presa, as repeticoes chegam tratadas.
# Uma tentativa sem sonda, com a sonda de outro documento (a pagina navegou a meio)
# ou sem foco nunca passa.
# Uma tentativa cujo SendInput falhou, ou uma corrida em que nenhuma tecla
# chegou a um AcceleratorKeyPressed (linha EXECUCAO INVALIDA na tabela), e
# invalida: nao sugere fallback nenhum.
#
# Imprime a tabela por hospedeiro (tambem no resumo do job) e falha se algum
# atalho falhar. A decisao (nativo / guarda de uma linha no mapa de teclas /
# IPC reservado) e do dono, sobre esta tabela: o script so a sugere.

$ErrorActionPreference = "Stop"

# A tabela do brief. O exe escreve a dele na primeira linha do registo e as
# duas tem de ser iguais (crates/neural-app/src/accel_spike.rs, SPIKE_CHORDS).
$Chords = @(
    [pscustomobject]@{ Name = "Ctrl+D"; Vk = 0x44; Ctrl = $true; Shift = $false; Code = "KeyD" },
    [pscustomobject]@{ Name = "Ctrl+J"; Vk = 0x4A; Ctrl = $true; Shift = $false; Code = "KeyJ" },
    [pscustomobject]@{ Name = "Ctrl+Shift+E"; Vk = 0x45; Ctrl = $true; Shift = $true; Code = "KeyE" },
    [pscustomobject]@{ Name = "Ctrl+Shift+A"; Vk = 0x41; Ctrl = $true; Shift = $true; Code = "KeyA" },
    [pscustomobject]@{ Name = "Ctrl+Shift+N"; Vk = 0x4E; Ctrl = $true; Shift = $true; Code = "KeyN" },
    [pscustomobject]@{ Name = "Ctrl+Shift+P"; Vk = 0x50; Ctrl = $true; Shift = $true; Code = "KeyP" },
    [pscustomobject]@{ Name = "F1"; Vk = 0x70; Ctrl = $false; Shift = $false; Code = "F1" },
    [pscustomobject]@{ Name = "Ctrl+Shift+S"; Vk = 0x53; Ctrl = $true; Shift = $true; Code = "KeyS" },
    [pscustomobject]@{ Name = "Ctrl+Shift+F"; Vk = 0x46; Ctrl = $true; Shift = $true; Code = "KeyF" },
    [pscustomobject]@{ Name = "Ctrl+O"; Vk = 0x4F; Ctrl = $true; Shift = $false; Code = "KeyO" }
)
$KnownHosts = @("Column", "Split", "PrivateSplit", "External", "Reader", "Pdf", "Epub", "SidePanel", "Service")
$FixtureHosts = @("Split", "PrivateSplit", "External", "Service")
# Quantas descidas tem a tentativa da tecla presa (a primeira e a real).
$RepeatDowns = 5

function Get-List($value) {
    if ($null -eq $value) { return @() }
    return @($value)
}

# A tecla da pagina e a do atalho? Pelo `code` (layout) e pela `key`, para uma
# pagina que so preencha um dos dois nao passar por cega.
function Test-KeyIsChord($entry, $chord) {
    if ($null -eq $entry) { return $false }
    if ([string]$entry.code -eq $chord.Code) { return $true }
    $key = [string]$entry.key
    if ($chord.Code -eq "F1") { return $key -eq "F1" }
    return $key.Length -eq 1 -and $key.ToUpperInvariant() -eq $chord.Code.Substring(3)
}

# O veredito de uma tentativa, so com o que se leu dela. Sem globais: o
# -SelfTest chama isto com registos sinteticos.
function Get-TrialVerdict {
    param(
        $Chord,
        [string]$HostName,
        [bool]$Repeat,
        [string]$ArmToken,
        $Page,
        $Native,
        $Acts,
        [bool]$Hooked,
        # O SendInput desta tentativa lancou: a tecla pode nao ter saido.
        [string]$InputError = "",
        # A corrida inteira nao mediu nada (Get-RunInvalidReason).
        [string]$RunInvalid = ""
    )
    $reasons = New-Object System.Collections.Generic.List[string]
    $pageDown = 0
    $pagePress = 0
    $find = $false
    if ($RunInvalid) { $reasons.Add("execucao invalida: $RunInvalid") }
    if ($InputError) { $reasons.Add("SendInput falhou: $InputError") }
    if (-not $Hooked) { $reasons.Add("handler nativo nao instalado neste hospedeiro") }
    if ($null -eq $Page) {
        $reasons.Add("sem leitura da pagina")
    }
    else {
        if ($null -eq $Page.token -or [string]$Page.token -eq "") {
            $reasons.Add("sonda ausente (o documento mudou?)")
        }
        elseif ([string]$Page.token -ne $ArmToken) {
            $reasons.Add("sonda de outro documento")
        }
        if (-not [bool]$Page.focus) { $reasons.Add("documento de topo sem foco ($($Page.active))") }
        foreach ($entry in @(Get-List $Page.down) + @(Get-List $Page.fixtureDown)) {
            if (Test-KeyIsChord $entry $Chord) { $pageDown++ }
        }
        foreach ($entry in @(Get-List $Page.press) + @(Get-List $Page.fixturePress)) {
            if (Test-KeyIsChord $entry $Chord) { $pagePress++ }
        }
        $find = [bool]$Page.find
    }
    if ($pageDown -gt 0) { $reasons.Add("a pagina viu o keydown ($pageDown)") }

    $actNames = @(Get-List $Acts | ForEach-Object { [string]$_.act })
    if ($find) { $actNames += "findbar" }
    if ($actNames.Count -gt 0) { $reasons.Add("act() do mapa de teclas: $($actNames -join ',')") }

    $mine = @(Get-List $Native | Where-Object { [string]$_.host -eq $HostName -and [string]$_.chord -eq $Chord.Name })
    $elsewhere = @(Get-List $Native | Where-Object { [string]$_.host -ne $HostName })
    $fired = @($mine | Where-Object { [bool]$_.fired }).Count
    $repeats = @($mine | Where-Object { [bool]$_.repeat -and [bool]$_.handled }).Count
    $unhandled = @($mine | Where-Object { -not [bool]$_.handled }).Count
    if ($fired -ne 1) { $reasons.Add("nativo disparou $fired vez(es)") }
    if ($unhandled -gt 0) { $reasons.Add("nativo sem Handled em $unhandled evento(s)") }
    if ($Repeat -and $repeats -lt 1) { $reasons.Add("a tecla presa nao chegou repetida ao nativo") }
    if ($elsewhere.Count -gt 0) {
        $reasons.Add("tecla vista por outro hospedeiro ($((@($elsewhere | ForEach-Object { $_.host }) | Sort-Object -Unique) -join ','))")
    }

    return [pscustomobject]@{
        Chord = $Chord.Name
        Host = $HostName
        Repeat = $Repeat
        Pass = $reasons.Count -eq 0
        Reasons = @($reasons)
        PageDown = $pageDown
        PagePress = $pagePress
        Acts = $actNames
        Fired = $fired
        Repeats = $repeats
    }
}

# O que uma falha sugere, pelas regras do brief. So uma sugestao: a decisao
# fica PENDENTE para o dono sobre a tabela. Uma tentativa em que a tecla pode
# nao ter chegado (SendInput falhou, corrida sem input) e invalida, nunca
# fallback-2: senao a tabela pedia um numero IPC para um atalho que o WebView2
# nunca recebeu.
function Get-FailureClass($verdict) {
    if ($verdict.Pass) { return "ok" }
    $invalid = @($verdict.Reasons | Where-Object {
            $_ -like "sonda*" -or $_ -like "sem leitura*" -or $_ -like "documento de topo sem foco*" -or $_ -like "handler nativo*" -or
            $_ -like "SendInput*" -or $_ -like "execucao invalida*"
        })
    if ($invalid.Count -gt 0) { return "invalido" }
    if ($verdict.Fired -ne 1) { return "fallback-2" }
    if ($verdict.PageDown -gt 0 -or $verdict.Acts.Count -gt 0) { return "fallback-1" }
    return "falha"
}

# Uma corrida em que nenhuma tecla da tabela chegou a um AcceleratorKeyPressed
# nao mediu o WebView2: o runner nao entregou o input ou a janela nao estava em
# primeiro plano. Devolve o motivo (todas as tentativas ficam invalidas) ou "".
function Get-RunInvalidReason([int]$NativeSeen, [int]$Trials) {
    if ($Trials -gt 0 -and $NativeSeen -eq 0) {
        return "nenhuma tecla da tabela chegou a um AcceleratorKeyPressed em $Trials tentativa(s) (o runner nao entregou o input ou a janela nao estava em primeiro plano); a tabela nao mede o WebView2"
    }
    return ""
}

function Format-SpikeTable([object[]]$verdicts, [string[]]$hostOrder, [string]$RunInvalid = "") {
    $out = New-Object System.Collections.Generic.List[string]
    $out.Add("## Spike do AcceleratorKeyPressed (infra-accel-spike)")
    $out.Add("")
    if ($RunInvalid) {
        $out.Add("**EXECUCAO INVALIDA (INVALID RUN):** $RunInvalid. Todas as tentativas contam como invalido; nada nesta tabela sugere fallback.")
        $out.Add("")
    }
    $out.Add("Passa = o keydown da pagina nunca ve o atalho, nenhum act() do NEURALIA_KEYMAP_SCRIPT dispara e o nativo dispara exatamente uma vez (tambem com a tecla presa).")
    $out.Add("")
    $header = "| Atalho | " + ($hostOrder -join " | ") + " |"
    $out.Add($header)
    $out.Add("|---|" + (($hostOrder | ForEach-Object { "---" }) -join "|") + "|")
    foreach ($chord in $Chords) {
        $cells = foreach ($hostName in $hostOrder) {
            $pair = @($verdicts | Where-Object { $_.Chord -eq $chord.Name -and $_.Host -eq $hostName })
            if ($pair.Count -eq 0) { "-" }
            elseif (@($pair | Where-Object { -not $_.Pass }).Count -eq 0) { "ok" }
            else {
                $classes = @($pair | Where-Object { -not $_.Pass } | ForEach-Object { Get-FailureClass $_ }) | Sort-Object -Unique
                "FALHA (" + ($classes -join ",") + ")"
            }
        }
        $out.Add("| $($chord.Name) | " + ($cells -join " | ") + " |")
    }
    foreach ($hostName in $hostOrder) {
        $rows = @($verdicts | Where-Object { $_.Host -eq $hostName })
        if ($rows.Count -eq 0) { continue }
        $out.Add("")
        $out.Add("### $hostName")
        $out.Add("")
        $out.Add("| Atalho | Tecla | keydown na pagina | keypress | act() | nativo (disparos / repeticoes tratadas) | Resultado |")
        $out.Add("|---|---|---|---|---|---|---|")
        foreach ($row in $rows) {
            $mode = if ($row.Repeat) { "presa ($RepeatDowns descidas)" } else { "uma vez" }
            $acts = if ($row.Acts.Count -gt 0) { $row.Acts -join "," } else { "0" }
            $result = if ($row.Pass) { "ok" } else { "FALHA: " + ($row.Reasons -join "; ") }
            $out.Add("| $($row.Chord) | $mode | $($row.PageDown) | $($row.PagePress) | $acts | $($row.Fired) / $($row.Repeats) | $result |")
        }
    }
    $out.Add("")
    $out.Add("### Sugestao pelas regras do brief (decisao PENDENTE do dono)")
    $out.Add("")
    foreach ($chord in $Chords) {
        $rows = @($verdicts | Where-Object { $_.Chord -eq $chord.Name })
        if ($rows.Count -eq 0) { continue }
        $failed = @($rows | Where-Object { -not $_.Pass })
        if ($failed.Count -eq 0) {
            $out.Add("- $($chord.Name): todos os hospedeiros passam -> despacho nativo como planeado.")
            continue
        }
        $byClass = $failed | Group-Object { Get-FailureClass $_ }
        $parts = foreach ($group in $byClass) {
            $hostsHit = (@($group.Group | ForEach-Object { $_.Host }) | Sort-Object -Unique) -join ","
            switch ($group.Name) {
                "fallback-1" { "a pagina/o mapa de teclas ainda a ve em $hostsHit -> fallback 1 (guarda de uma linha no NEURALIA_KEYMAP_SCRIPT, sim do dono)" }
                "fallback-2" { "o nativo nao a trata uma vez em $hostsHit -> fallback 2 (numero IPC reservado so para este atalho)" }
                "invalido" { "medicao invalida em $hostsHit (sonda/foco/handler/input) -> repetir antes de decidir" }
                default { "falha em $hostsHit" }
            }
        }
        $out.Add("- $($chord.Name): " + ($parts -join "; "))
    }
    return $out
}

function Invoke-SelfTest {
    $chord = $Chords | Where-Object { $_.Name -eq "Ctrl+Shift+N" }
    $clean = [pscustomobject]@{ token = "t1"; focus = $true; active = "BODY"; find = $false; down = @(); press = @(); fixtureDown = @(); fixturePress = @() }
    $native = @(
        [pscustomobject]@{ host = "Split"; chord = "Ctrl+Shift+N"; kind = "down"; handled = $true; fired = $true; repeat = $false },
        [pscustomobject]@{ host = "Split"; chord = "Ctrl+Shift+N"; kind = "up"; handled = $true; fired = $false; repeat = $false }
    )
    $held = $native + @(
        [pscustomobject]@{ host = "Split"; chord = "Ctrl+Shift+N"; kind = "down"; handled = $true; fired = $false; repeat = $true },
        [pscustomobject]@{ host = "Split"; chord = "Ctrl+Shift+N"; kind = "down"; handled = $true; fired = $false; repeat = $true }
    )
    $leak = [pscustomobject]@{ token = "t1"; focus = $true; active = "BODY"; find = $false
        down = @([pscustomobject]@{ key = "N"; code = "KeyN"; ctrl = $true; shift = $true; repeat = $false })
        press = @(); fixtureDown = @([pscustomobject]@{ key = "N"; code = "KeyN" }); fixturePress = @()
    }
    # A tecla vista so pela `key` (code vazio) tambem conta.
    $keyOnly = [pscustomobject]@{ token = "t1"; focus = $true; active = "BODY"; find = $false
        down = @([pscustomobject]@{ key = "n"; code = "" }); press = @(); fixtureDown = $null; fixturePress = $null
    }
    $findBar = [pscustomobject]@{ token = "t1"; focus = $true; active = "INPUT"; find = $true; down = @(); press = @(); fixtureDown = @(); fixturePress = @() }
    $otherDoc = [pscustomobject]@{ token = "t2"; focus = $true; active = "BODY"; find = $false; down = @(); press = @(); fixtureDown = @(); fixturePress = @() }
    $noProbe = [pscustomobject]@{ token = $null; focus = $true; active = "BODY"; find = $false; down = @(); press = @(); fixtureDown = $null; fixturePress = $null }
    $noFocus = [pscustomobject]@{ token = "t1"; focus = $false; active = "IFRAME"; find = $false; down = @(); press = @(); fixtureDown = @(); fixturePress = @() }
    $twice = $native + @([pscustomobject]@{ host = "Split"; chord = "Ctrl+Shift+N"; kind = "down"; handled = $true; fired = $true; repeat = $false })
    $foreign = $native + @([pscustomobject]@{ host = "Column"; chord = "Ctrl+Shift+N"; kind = "down"; handled = $true; fired = $true; repeat = $false })
    $unhandled = @([pscustomobject]@{ host = "Split"; chord = "Ctrl+Shift+N"; kind = "down"; handled = $false; fired = $true; repeat = $false })
    $newtab = @([pscustomobject]@{ act = "newtab" })

    $cases = @(
        @{ Name = "limpo"; Repeat = $false; Page = $clean; Native = $native; Acts = @(); Hooked = $true; Pass = $true; Class = "ok" },
        @{ Name = "presa limpa"; Repeat = $true; Page = $clean; Native = $held; Acts = @(); Hooked = $true; Pass = $true; Class = "ok" },
        @{ Name = "presa sem repeticao"; Repeat = $true; Page = $clean; Native = $native; Acts = @(); Hooked = $true; Pass = $false; Class = "falha" },
        @{ Name = "Handled=false"; Repeat = $false; Page = $leak; Native = $native; Acts = $newtab; Hooked = $true; Pass = $false; Class = "fallback-1" },
        @{ Name = "so a key"; Repeat = $false; Page = $keyOnly; Native = $native; Acts = @(); Hooked = $true; Pass = $false; Class = "fallback-1" },
        @{ Name = "barra de procura"; Repeat = $false; Page = $findBar; Native = $native; Acts = @(); Hooked = $true; Pass = $false; Class = "fallback-1" },
        @{ Name = "sem nativo"; Repeat = $false; Page = $clean; Native = @(); Acts = @(); Hooked = $true; Pass = $false; Class = "fallback-2" },
        @{ Name = "nativo duas vezes"; Repeat = $false; Page = $clean; Native = $twice; Acts = @(); Hooked = $true; Pass = $false; Class = "fallback-2" },
        @{ Name = "outro hospedeiro"; Repeat = $false; Page = $clean; Native = $foreign; Acts = @(); Hooked = $true; Pass = $false; Class = "falha" },
        @{ Name = "nativo sem Handled"; Repeat = $false; Page = $clean; Native = $unhandled; Acts = @(); Hooked = $true; Pass = $false; Class = "falha" },
        @{ Name = "outro documento"; Repeat = $false; Page = $otherDoc; Native = $native; Acts = @(); Hooked = $true; Pass = $false; Class = "invalido" },
        @{ Name = "sem sonda"; Repeat = $false; Page = $noProbe; Native = $native; Acts = @(); Hooked = $true; Pass = $false; Class = "invalido" },
        @{ Name = "sem foco"; Repeat = $false; Page = $noFocus; Native = $native; Acts = @(); Hooked = $true; Pass = $false; Class = "invalido" },
        @{ Name = "sem leitura"; Repeat = $false; Page = $null; Native = $native; Acts = @(); Hooked = $true; Pass = $false; Class = "invalido" },
        @{ Name = "sem handler"; Repeat = $false; Page = $clean; Native = $native; Acts = @(); Hooked = $false; Pass = $false; Class = "invalido" },
        # A tecla pode nao ter saido: igual a "sem nativo" na pagina e no
        # registo, mas invalido, nunca fallback-2.
        @{ Name = "SendInput falhou"; Repeat = $false; Page = $clean; Native = @(); Acts = @(); Hooked = $true; InputError = "SendInput recusou a tecla 78 (erro 5)"; Pass = $false; Class = "invalido" },
        @{ Name = "corrida sem input"; Repeat = $true; Page = $clean; Native = @(); Acts = @(); Hooked = $true; RunInvalid = (Get-RunInvalidReason -NativeSeen 0 -Trials 2); Pass = $false; Class = "invalido" }
    )
    $failures = 0
    $verdicts = @()
    foreach ($case in $cases) {
        $verdict = Get-TrialVerdict -Chord $chord -HostName "Split" -Repeat $case.Repeat -ArmToken "t1" -Page $case.Page -Native $case.Native -Acts $case.Acts -Hooked $case.Hooked -InputError ([string]$case.InputError) -RunInvalid ([string]$case.RunInvalid)
        $class = Get-FailureClass $verdict
        $ok = ($verdict.Pass -eq $case.Pass) -and ($class -eq $case.Class)
        if (-not $ok) { $failures++ }
        Write-Host ("self-test {0,-20} pass={1,-5} class={2,-10} {3} {4}" -f $case.Name, $verdict.Pass, $class, $(if ($ok) { "ok" } else { "ERRADO" }), ($verdict.Reasons -join "; "))
        $verdicts += $verdict
    }
    $table = Format-SpikeTable $verdicts @("Split")
    $table | ForEach-Object { Write-Host $_ }
    if (-not ($table -match "FALHA")) { $failures++; Write-Host "self-test: a tabela nao mostrou a falha" }
    if ($table -cmatch "INVALID RUN") { $failures++; Write-Host "self-test: uma corrida com nativo saiu como execucao invalida" }

    # A corrida inteira sem nenhum AcceleratorKeyPressed: todas as tentativas
    # invalidas, a linha EXECUCAO INVALIDA na tabela (e no resumo do job, que e
    # esta tabela) e nenhuma sugestao de fallback.
    $runChecks = 0
    $runReason = Get-RunInvalidReason -NativeSeen 0 -Trials 2
    if (-not $runReason) { $failures++; Write-Host "self-test: corrida sem nativo nao ficou invalida" }
    if (Get-RunInvalidReason -NativeSeen 3 -Trials 2) { $failures++; Write-Host "self-test: corrida com nativo ficou invalida" }
    $runVerdicts = @(foreach ($repeat in @($false, $true)) {
            Get-TrialVerdict -Chord $chord -HostName "Split" -Repeat $repeat -ArmToken "t1" -Page $clean -Native @() -Acts @() -Hooked $true -RunInvalid $runReason
        })
    $runClasses = @($runVerdicts | ForEach-Object { Get-FailureClass $_ }) | Sort-Object -Unique
    $runTable = @(Format-SpikeTable $runVerdicts @("Split") $runReason)
    $runOk = (($runClasses -join ",") -eq "invalido") -and (@($runTable -cmatch "INVALID RUN").Count -eq 1) -and (@($runTable -match "-> fallback [12]").Count -eq 0)
    $runChecks++
    Write-Host ("self-test {0,-20} classes={1,-10} {2}" -f "tabela sem input", ($runClasses -join ","), $(if ($runOk) { "ok" } else { "ERRADO" }))
    if (-not $runOk) { $failures++; $runTable | ForEach-Object { Write-Host $_ } }

    if ($failures -gt 0) { throw "self-test do avaliador: $failures caso(s) errado(s)." }
    Write-Host "self-test do avaliador: $($cases.Count + $runChecks) casos certos."
}

if ($SelfTest) {
    Invoke-SelfTest
    return
}

if (-not $ExePath) { throw "-ExePath e obrigatorio (ou -SelfTest)." }
foreach ($hostName in $Hosts) {
    if ($KnownHosts -notcontains $hostName) { throw "Hospedeiro desconhecido: $hostName" }
}
$exe = (Resolve-Path -LiteralPath $ExePath).Path

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Threading;

public static class AccelSpikeInput {
    [StructLayout(LayoutKind.Sequential)]
    struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)]
    struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)]
    struct InputUnion { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)]
    struct INPUT { public uint type; public InputUnion u; }

    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);
    [DllImport("user32.dll")] static extern uint MapVirtualKey(uint code, uint mapType);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, IntPtr pid);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint from, uint to, bool attach);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr hwnd, int cmd);

    const uint INPUT_KEYBOARD = 1;
    const uint KEYEVENTF_KEYUP = 0x2;
    const ushort VK_SHIFT = 0x10;
    const ushort VK_CONTROL = 0x11;

    static void Key(ushort vk, bool up) {
        var input = new INPUT();
        input.type = INPUT_KEYBOARD;
        input.u.ki.wVk = vk;
        // O scan code vai no lParam: e dele que o Chromium tira o `code`.
        input.u.ki.wScan = (ushort)MapVirtualKey(vk, 0);
        input.u.ki.dwFlags = up ? KEYEVENTF_KEYUP : 0;
        if (SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT))) != 1) {
            throw new InvalidOperationException("SendInput recusou a tecla " + vk + " (erro " + Marshal.GetLastWin32Error() + ")");
        }
    }

    // Modificadores em baixo, a tecla `downs` vezes (a partir da segunda o
    // Windows marca-a como repetida: estava em baixo), a tecla em cima, os
    // modificadores em cima.
    public static void Chord(ushort vk, bool ctrl, bool shift, int downs, int gapMs) {
        if (ctrl) { Key(VK_CONTROL, false); Thread.Sleep(15); }
        if (shift) { Key(VK_SHIFT, false); Thread.Sleep(15); }
        for (int i = 0; i < downs; i++) {
            Key(vk, false);
            Thread.Sleep(i + 1 < downs ? gapMs : 30);
        }
        Key(vk, true);
        Thread.Sleep(15);
        if (shift) { Key(VK_SHIFT, true); Thread.Sleep(15); }
        if (ctrl) { Key(VK_CONTROL, true); }
    }

    public static uint WindowPid(IntPtr hwnd) {
        uint pid;
        GetWindowThreadProcessId(hwnd, out pid);
        return pid;
    }

    // SetForegroundWindow com a fila de input do primeiro plano atual
    // anexada: sem isso o Windows recusa tirar o primeiro plano a outro.
    public static bool BringToFront(IntPtr hwnd) {
        IntPtr current = GetForegroundWindow();
        uint me = GetCurrentThreadId();
        uint them = current == IntPtr.Zero ? 0 : GetWindowThreadProcessId(current, IntPtr.Zero);
        bool attached = them != 0 && them != me && AttachThreadInput(me, them, true);
        try {
            ShowWindow(hwnd, 5);
            BringWindowToTop(hwnd);
            return SetForegroundWindow(hwnd);
        } finally {
            if (attached) { AttachThreadInput(me, them, false); }
        }
    }
}
"@

$work = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-accel-spike-" + [Guid]::NewGuid().ToString("N"))
$spikeDir = Join-Path $work "spike"
$dataDir = Join-Path $work "data"
New-Item -ItemType Directory -Force $spikeDir, $dataDir | Out-Null
$logPath = Join-Path $spikeDir "spike.log"
$commandPath = Join-Path $spikeDir "cmd.txt"
$debugLog = Join-Path $work "debug.log"
$portFile = Join-Path $work "fixture.port"

$script:Records = New-Object System.Collections.Generic.List[object]
$script:LogOffset = 0
$script:LogTail = ""
$script:Seq = 0

function Update-Records {
    if (-not (Test-Path -LiteralPath $logPath)) { return }
    $stream = [IO.File]::Open($logPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
    try {
        if ($stream.Length -le $script:LogOffset) { return }
        $null = $stream.Seek($script:LogOffset, [IO.SeekOrigin]::Begin)
        $buffer = New-Object byte[] ($stream.Length - $script:LogOffset)
        $read = $stream.Read($buffer, 0, $buffer.Length)
        $script:LogOffset += $read
        $text = $script:LogTail + [Text.Encoding]::UTF8.GetString($buffer, 0, $read)
    }
    finally {
        $stream.Dispose()
    }
    $lines = $text -split "`n"
    $script:LogTail = $lines[-1]
    foreach ($line in $lines[0..($lines.Count - 2)]) {
        $trimmed = $line.Trim()
        if ($trimmed -eq "") { continue }
        try { $script:Records.Add(($trimmed | ConvertFrom-Json)) }
        catch { Write-Host "linha do registo ilegivel: $trimmed" }
    }
}

function Wait-Record([scriptblock]$match, [int]$timeoutMs, [string]$what) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $timeoutMs) {
        Update-Records
        foreach ($record in $script:Records) {
            if (& $match $record) { return $record }
        }
        if ($script:Process.HasExited) { throw "O NeuralIA saiu (codigo $($script:Process.ExitCode)) a espera de: $what" }
        Start-Sleep -Milliseconds 40
    }
    throw "Sem resposta em $timeoutMs ms: $what"
}

function Send-SpikeCommand([string]$verb, [string]$hostName, [string]$argument) {
    $script:Seq++
    $line = "$($script:Seq) $verb $hostName"
    if ($argument) { $line += " $argument" }
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (Test-Path -LiteralPath $commandPath) {
        if ($watch.ElapsedMilliseconds -gt 10000) { throw "O exe nao consumiu o comando anterior." }
        Start-Sleep -Milliseconds 20
    }
    $temp = "$commandPath.tmp"
    [IO.File]::WriteAllText($temp, $line + "`n", [Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temp -Destination $commandPath
    return $script:Seq
}

function Invoke-Spike([string]$verb, [string]$hostName, [string]$argument, [int]$timeoutMs = 15000) {
    $seq = Send-SpikeCommand $verb $hostName $argument
    $ack = Wait-Record { param($r) $r.t -eq "ack" -and [int64]$r.seq -eq $seq } $timeoutMs "ack de '$verb $hostName'"
    return $ack
}

# Corre um script da sonda e devolve o que a pagina respondeu (objeto), ou
# $null se ela nao respondeu.
function Invoke-Probe([string]$phase, [string]$hostName, [int]$trial) {
    $argument = if ($trial -gt 0) { [string]$trial } else { "" }
    $seq = Send-SpikeCommand $phase $hostName $argument
    $ack = Wait-Record { param($r) $r.t -eq "ack" -and [int64]$r.seq -eq $seq } 15000 "ack de '$phase $hostName'"
    if (-not $ack.ok) { Write-Host "  $phase ${hostName}: $($ack.detail)"; return $null }
    try {
        $page = Wait-Record { param($r) $r.t -eq "page" -and [int64]$r.seq -eq $seq } 15000 "resposta da pagina a '$phase $hostName'"
    }
    catch {
        Write-Host "  $phase ${hostName}: $($_.Exception.Message)"
        return $null
    }
    if ([string]$page.result -eq "" -or [string]$page.result -eq "null") { return $null }
    try { return ($page.result | ConvertFrom-Json) } catch { return $null }
}

function Set-Foreground([string]$hostName) {
    for ($attempt = 1; $attempt -le 4; $attempt++) {
        $ack = Invoke-Spike "focus" $hostName ""
        Start-Sleep -Milliseconds 150
        $foreground = [AccelSpikeInput]::GetForegroundWindow()
        $ours = [AccelSpikeInput]::WindowPid($foreground) -eq [uint32]$script:Process.Id
        if ($ack.ok -and $ours) { return $true }
        Write-Host "  foco ${hostName} (tentativa $attempt): ack=$($ack.ok) '$($ack.detail)' primeiro-plano-nosso=$ours"
        $script:Process.Refresh()
        if ($script:Process.MainWindowHandle -ne [IntPtr]::Zero) {
            $null = [AccelSpikeInput]::BringToFront($script:Process.MainWindowHandle)
        }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

$fixture = $null
$script:Process = $null
$verdicts = New-Object System.Collections.Generic.List[object]
$trialInfo = @{}
$trial = 0
$failedHard = $null

try {
    $fixture = Start-Process -FilePath "node" -ArgumentList @("`"$(Join-Path $PSScriptRoot 'accel-spike-fixture.mjs')`"", "`"$portFile`"") -PassThru -NoNewWindow
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $portFile)) {
        if ($watch.ElapsedMilliseconds -gt 10000 -or $fixture.HasExited) { throw "A fixture de 127.0.0.1 nao arrancou." }
        Start-Sleep -Milliseconds 50
    }
    $port = ([IO.File]::ReadAllText($portFile)).Trim()
    $fixtureUrl = "http://127.0.0.1:$port/fixture.html"
    Write-Host "fixture: $fixtureUrl"

    $env:NEURALIA_ACCEL_SPIKE_DIR = $spikeDir
    $env:NEURALIA_DATA_DIR = $dataDir
    $env:NEURALIA_DEBUG_LOG = $debugLog
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_STARTUP_INPUT = "atalhos nativos do spike"
    $script:Process = Start-Process -FilePath $exe -PassThru

    $hello = Wait-Record { param($r) $r.t -eq "hello" } 20000 "linha hello do exe do spike"
    if ([string]$hello.build -ne "NEURALIA-ACCEL-SPIKE-BUILD-v1") { throw "Marcador inesperado: $($hello.build)" }
    $theirs = @($hello.chords | ForEach-Object { "{0}:{1}:{2}:{3}:{4}" -f $_.name, $_.vk, $_.ctrl, $_.shift, $_.code })
    $ours = @($Chords | ForEach-Object { "{0}:{1}:{2}:{3}:{4}" -f $_.Name, $_.Vk, $_.Ctrl, $_.Shift, $_.Code })
    if (($theirs -join "|") -ne ($ours -join "|")) {
        throw "A tabela de atalhos do exe difere da do brief:`n exe:    $($theirs -join ', ')`n script: $($ours -join ', ')"
    }

    foreach ($hostName in $Hosts) {
        Write-Host "== $hostName"
        $argument = if ($FixtureHosts -contains $hostName) { $fixtureUrl } else { "" }
        $opened = $null
        $watch = [Diagnostics.Stopwatch]::StartNew()
        do {
            $opened = Invoke-Spike "open" $hostName $argument 30000
            if ($opened.ok) { break }
            Start-Sleep -Milliseconds 500
        } while ($watch.ElapsedMilliseconds -lt 45000)
        if (-not $opened.ok) {
            Write-Host "  $hostName nao abriu: $($opened.detail)"
        }

        # A pagina acabou de carregar (e, nos web, e a fixture): so entao a
        # sonda vai para o documento que vai receber as teclas.
        $armed = $null
        $watch.Restart()
        while ($opened.ok -and $watch.ElapsedMilliseconds -lt 45000) {
            $armed = Invoke-Probe "arm" $hostName 0
            $ready = $null -ne $armed -and [string]$armed.ready -eq "complete"
            if ($ready -and $FixtureHosts -contains $hostName) {
                $ready = ([string]$armed.href).StartsWith($fixtureUrl)
            }
            if ($ready) { break }
            $armed = $null
            Start-Sleep -Milliseconds 500
        }
        if ($null -eq $armed) { Write-Host "  sonda nao ficou armada em $hostName" }
        else { Write-Host "  sonda em $($armed.href)" }

        $front = $opened.ok -and (Set-Foreground $hostName)
        if (-not $front) { Write-Host "  AVISO: a janela do NeuralIA nao ficou em primeiro plano para $hostName" }

        foreach ($chord in $Chords) {
            foreach ($repeat in @($false, $true)) {
                $trial++
                $trialInfo[$trial] = [pscustomobject]@{ Host = $hostName; Chord = $chord; Repeat = $repeat; Token = ""; Page = $null; InputError = "" }
                if (-not $opened.ok) { continue }
                # O begin volta a armar a sonda (se a pagina navegou entre
                # tentativas) e zera-a; o token dele e o que o pull tem de ver.
                $begin = Invoke-Probe "begin" $hostName $trial
                $trialInfo[$trial].Token = if ($null -ne $begin) { [string]$begin.token } else { "" }
                if ($null -ne $begin -and -not [bool]$begin.focus) { $null = Set-Foreground $hostName }
                $downs = if ($repeat) { $RepeatDowns } else { 1 }
                try {
                    [AccelSpikeInput]::Chord([uint16]$chord.Vk, $chord.Ctrl, $chord.Shift, $downs, 40)
                }
                catch {
                    # A tecla pode nao ter saido: a tentativa fica invalida
                    # (nunca fallback-2 por um nativo que nao a recebeu).
                    $trialInfo[$trial].InputError = $_.Exception.Message
                    Write-Host "  SendInput: $($_.Exception.Message)"
                }
                Start-Sleep -Milliseconds $SettleMs
                $trialInfo[$trial].Page = Invoke-Probe "pull" $hostName $trial
            }
        }
    }
}
catch {
    $failedHard = $_.Exception.Message
    Write-Host "ERRO: $failedHard"
}
finally {
    foreach ($name in "NEURALIA_ACCEL_SPIKE_DIR", "NEURALIA_DATA_DIR", "NEURALIA_DEBUG_LOG", "NEURALIA_NO_GMAIL", "NEURALIA_STARTUP_INPUT") {
        Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    }
    if ($script:Process -and -not $script:Process.HasExited) {
        Stop-Process -Id $script:Process.Id -Force -ErrorAction SilentlyContinue
    }
    if ($fixture -and -not $fixture.HasExited) {
        Stop-Process -Id $fixture.Id -Force -ErrorAction SilentlyContinue
    }
}

Update-Records
$hooked = @($script:Records | Where-Object { $_.t -eq "hooked" -and $_.ok } | ForEach-Object { [string]$_.host }) | Sort-Object -Unique
$hookFailures = @($script:Records | Where-Object { $_.t -eq "hooked" -and -not $_.ok })
foreach ($failure in $hookFailures) { Write-Host "handler nao instalado em $($failure.host): $($failure.error)" }

# Antes dos vereditos: uma corrida em que nenhuma tecla chegou ao nativo marca
# todas as tentativas como invalidas, e a tabela (e o resumo do job) di-lo.
$nativeSeen = @($script:Records | Where-Object { $_.t -eq "native" }).Count
$runInvalid = Get-RunInvalidReason -NativeSeen $nativeSeen -Trials $trialInfo.Count
if ($runInvalid) { Write-Host "EXECUCAO INVALIDA: $runInvalid" }

foreach ($key in ($trialInfo.Keys | Sort-Object)) {
    $info = $trialInfo[$key]
    $native = @($script:Records | Where-Object { $_.t -eq "native" -and [int]$_.trial -eq $key })
    $acts = @($script:Records | Where-Object { $_.t -eq "act" -and [int]$_.trial -eq $key })
    $verdicts.Add((Get-TrialVerdict -Chord $info.Chord -HostName $info.Host -Repeat $info.Repeat -ArmToken $info.Token -Page $info.Page -Native $native -Acts $acts -Hooked ($hooked -contains $info.Host) -InputError $info.InputError -RunInvalid $runInvalid))
}

$table = Format-SpikeTable @($verdicts) $Hosts $runInvalid
$table | ForEach-Object { Write-Host $_ }
[IO.File]::WriteAllLines((Join-Path (Get-Location) $TablePath), [string[]]$table, [Text.UTF8Encoding]::new($false))
if ($env:GITHUB_STEP_SUMMARY) {
    $title = if ($ExpectLeak) { "(sabotagem Handled=false)" } else { "" }
    Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value (@("# accel-spike $title") + $table) -Encoding utf8
}

$failed = @($verdicts | Where-Object { -not $_.Pass })
$leaks = @($verdicts | Where-Object { $_.PageDown -gt 0 })

if ($failed.Count -gt 0 -or $failedHard) {
    Write-Host "--- ultimas linhas do registo do spike"
    Get-Content -LiteralPath $logPath -Tail 60 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host $_ }
    Write-Host "--- ultimas linhas do NEURALIA_DEBUG_LOG"
    Get-Content -LiteralPath $debugLog -Tail 40 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host $_ }
}

if ($ExpectLeak) {
    if ($leaks.Count -eq 0) {
        throw "Sabotagem Handled=false ficou verde: a tabela nao mostrou a pagina a ver nenhuma tecla ($($verdicts.Count) tentativas)."
    }
    Write-Host "Prova de sabotagem: com Handled=false a pagina viu a tecla em $($leaks.Count) de $($verdicts.Count) tentativas ($((@($leaks | ForEach-Object { $_.Host + ' ' + $_.Chord }) | Select-Object -First 5) -join '; '))."
    return
}
if ($failedHard) { throw "O spike nao correu ate ao fim: $failedHard" }
if ($runInvalid) { throw "Execucao invalida (nada a decidir sobre esta tabela): $runInvalid" }
if ($verdicts.Count -ne $Hosts.Count * $Chords.Count * 2) {
    throw "Tentativas: $($verdicts.Count), esperadas $($Hosts.Count * $Chords.Count * 2)."
}
if ($failed.Count -gt 0) {
    throw "$($failed.Count) de $($verdicts.Count) tentativas falharam a regra do spike (tabela acima)."
}
Write-Host "Spike: os $($Chords.Count) atalhos passam em $($Hosts.Count) hospedeiros, uma vez e com a tecla presa."
