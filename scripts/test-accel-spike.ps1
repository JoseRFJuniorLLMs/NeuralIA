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
# Uma tentativa cujo SendInput falhou, uma tentativa que nao chegou a ser
# medida (o hospedeiro nao abriu, nao ficou com o foco ou deixou de responder;
# o exe saiu) ou uma corrida em que nenhuma tecla chegou a um
# AcceleratorKeyPressed (linha EXECUCAO INVALIDA na tabela) e invalida: nao
# sugere fallback nenhum.
#
# Um hospedeiro que nao abre, nao fica com o foco ou deixa de responder (sem
# ack, sem resposta da sonda) fica "invalido" nessa linha da tabela, com o
# motivo, e a corrida CONTINUA no hospedeiro seguinte: um prazo esgotado num
# hospedeiro nunca aborta a corrida, e a tabela sai sempre, com uma linha por
# atalho, hospedeiro e modo.
#
# POLITICA DE SAIDA (decisao do lead, 25/09/2026). O entregavel do spike e a
# TABELA. O script sai com 0 quando a tabela esta completa e a corrida e
# valida, MESMO QUE atalhos falhem a regra acima: essas falhas (fallback-1,
# fallback-2, falha, e os hospedeiros invalidos) ficam na tabela e no resumo
# do job para a decisao do dono (AGENTS.md sec. 7: despacho nativo / guarda
# de uma linha no mapa de teclas / numero IPC reservado). Sai com erro so
# por um erro do condutor (a fixture ou o exe nao arrancaram, a tabela de
# atalhos do exe difere, o exe saiu a meio, a tabela nao se pode escrever)
# ou por uma execucao INVALIDA (nenhuma tecla chegou a um
# AcceleratorKeyPressed). A sabotagem Handled=false (-ExpectLeak) tem a sua
# propria regra: tem de ver a fuga (a pagina ve a tecla), senao falha.

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
# Os que so abrem com a fixture. A coluna tambem a pede (a excecao de
# navegacao do exe do spike) mas, se ela nao carregar, mede-se na pagina ao
# vivo e a nota da tabela di-lo.
$FixtureHosts = @("Split", "PrivateSplit", "External", "Service")
# Quantas descidas tem a tentativa da tecla presa (a primeira e a real).
$RepeatDowns = 5

# Os elementos de uma lista, pelo `foreach` -- nunca `@($value)`: o `@()`
# sobre uma List[object] embrulhada num PSObject (e o que o `New-Object`
# devolve) lanca "Argument types do not match" no PowerShell 5.1 e 7 (o
# binder PSToObjectArrayBinder). Foi o que partiu a tabela da primeira
# corrida do CI.
function Get-List($value) {
    if ($null -eq $value) { return }
    foreach ($item in $value) { $item }
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
        [string]$RunInvalid = "",
        # A tentativa nao chegou a ser medida: o hospedeiro nao abriu, nao
        # ficou com o foco ou deixou de responder, ou o exe saiu antes.
        [string]$NotMeasured = ""
    )
    $reasons = [System.Collections.Generic.List[string]]::new()
    $pageDown = 0
    $pagePress = 0
    $find = $false
    $actNames = @()
    $fired = 0
    $repeats = 0
    if ($RunInvalid) { $reasons.Add("execucao invalida: $RunInvalid") }
    if ($NotMeasured) {
        # Nada do que o registo tenha desta tentativa conta: so o motivo.
        $reasons.Add("nao medida: $NotMeasured")
    }
    else {
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
    }

    return [pscustomobject]@{
        Chord = $Chord.Name
        Host = $HostName
        Repeat = $Repeat
        Pass = $reasons.Count -eq 0
        Reasons = $reasons.ToArray()
        PageDown = $pageDown
        PagePress = $pagePress
        Acts = $actNames
        Fired = $fired
        Repeats = $repeats
    }
}

# O que uma falha sugere, pelas regras do brief. So uma sugestao: a decisao
# fica PENDENTE para o dono sobre a tabela. Uma tentativa em que a tecla pode
# nao ter chegado (SendInput falhou, corrida sem input, tentativa nao medida)
# e invalida, nunca fallback-2: senao a tabela pedia um numero IPC para um
# atalho que o WebView2 nunca recebeu.
function Get-FailureClass($verdict) {
    if ($verdict.Pass) { return "ok" }
    $invalid = @($verdict.Reasons | Where-Object {
            $_ -like "sonda*" -or $_ -like "sem leitura*" -or $_ -like "documento de topo sem foco*" -or $_ -like "handler nativo*" -or
            $_ -like "SendInput*" -or $_ -like "execucao invalida*" -or $_ -like "nao medida*"
        })
    if ($invalid.Count -gt 0) { return "invalido" }
    if ($verdict.Fired -ne 1) { return "fallback-2" }
    if ($verdict.PageDown -gt 0 -or $verdict.Acts.Count -gt 0) { return "fallback-1" }
    return "falha"
}

# Uma corrida em que nenhuma tecla da tabela chegou a um AcceleratorKeyPressed
# nao mediu o WebView2: o runner nao entregou o input, a janela nao estava em
# primeiro plano ou nenhum hospedeiro chegou a receber teclas. Devolve o
# motivo (todas as tentativas ficam invalidas) ou "".
function Get-RunInvalidReason([int]$NativeSeen, [int]$Trials, [int]$Sent = -1) {
    if ($Trials -gt 0 -and $NativeSeen -eq 0) {
        $sentText = if ($Sent -ge 0) { ", $Sent com teclas enviadas" } else { "" }
        return "nenhuma tecla da tabela chegou a um AcceleratorKeyPressed em $Trials tentativa(s)$sentText (o runner nao entregou o input, a janela nao estava em primeiro plano ou nenhum hospedeiro recebeu teclas); a tabela nao mede o WebView2"
    }
    return ""
}

# Uma linha do registo JSON Lines do exe; $null se nao for JSON.
function ConvertFrom-SpikeLogLine([string]$line) {
    $trimmed = $line.Trim()
    if ($trimmed -eq "") { return $null }
    try { return ($trimmed | ConvertFrom-Json) }
    catch {
        Write-Host "linha do registo ilegivel: $trimmed"
        return $null
    }
}

# O que a sonda devolveu numa linha `page` (o JSON do ExecuteScript, que o
# exe escreve como string): o objeto, ou $null se a pagina nao respondeu.
function ConvertFrom-PageResult($pageRecord) {
    if ($null -eq $pageRecord) { return $null }
    $result = [string]$pageRecord.result
    if ($result -eq "" -or $result -eq "null") { return $null }
    try { return ($result | ConvertFrom-Json) } catch { return $null }
}

# O plano da corrida, feito antes de se carregar numa tecla: uma tentativa
# por hospedeiro, atalho e modo, pela ordem da corrida (o numero e o do
# `begin`). Assim a tabela tem sempre todas as linhas, e o que nao chegou a
# ser medido fica invalido com o motivo.
function New-TrialPlan([string[]]$HostList) {
    $plan = @{}
    $trial = 0
    foreach ($hostName in $HostList) {
        foreach ($chord in $Chords) {
            foreach ($repeat in @($false, $true)) {
                $trial++
                $plan[$trial] = [pscustomobject]@{
                    Host = $hostName; Chord = $chord; Repeat = $repeat; Token = ""; Page = $null
                    InputError = ""; NotMeasured = ""; KeysSent = $false; Done = $false
                }
            }
        }
    }
    return $plan
}

function Format-Cell([string]$text) {
    return (($text -replace "`r?`n", " ") -replace "\|", "\|")
}

function Format-SpikeTable([object[]]$verdicts, [string[]]$hostOrder, [string]$RunInvalid = "", [string[]]$Notes = @()) {
    $out = [System.Collections.Generic.List[string]]::new()
    $out.Add("## Spike do AcceleratorKeyPressed (infra-accel-spike)")
    $out.Add("")
    if ($RunInvalid) {
        $out.Add("**EXECUCAO INVALIDA (INVALID RUN):** $(Format-Cell $RunInvalid). Todas as tentativas contam como invalido; nada nesta tabela sugere fallback.")
        $out.Add("")
    }
    $classes = @($verdicts | Where-Object { -not $_.Pass } | ForEach-Object { Get-FailureClass $_ })
    $okCount = @($verdicts | Where-Object { $_.Pass }).Count
    $invalidCount = @($classes | Where-Object { $_ -eq "invalido" }).Count
    $ruleCount = $classes.Count - $invalidCount
    $out.Add("**Resumo:** $($verdicts.Count) tentativas: $okCount ok, $ruleCount falham a regra do spike, $invalidCount invalidas (nao medidas).")
    $out.Add("")
    $out.Add("**Politica de saida:** o entregavel e esta tabela. O job so falha por um erro do condutor ou por uma execucao invalida (nenhuma tecla chegou a um AcceleratorKeyPressed); os atalhos que falham a regra ficam aqui para a decisao do dono (AGENTS.md sec. 7).")
    $out.Add("")
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
                $pairClasses = @($pair | Where-Object { -not $_.Pass } | ForEach-Object { Get-FailureClass $_ }) | Sort-Object -Unique
                "FALHA (" + ($pairClasses -join ",") + ")"
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
            $out.Add("| $($row.Chord) | $mode | $($row.PageDown) | $($row.PagePress) | $(Format-Cell $acts) | $($row.Fired) / $($row.Repeats) | $(Format-Cell $result) |")
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
                "invalido" { "medicao invalida em $hostsHit (sonda/foco/handler/input/hospedeiro) -> repetir antes de decidir" }
                default { "falha em $hostsHit" }
            }
        }
        $out.Add("- $($chord.Name): " + ($parts -join "; "))
    }
    $noteList = @(Get-List $Notes | Where-Object { $_ })
    if ($noteList.Count -gt 0) {
        $out.Add("")
        $out.Add("### Notas da corrida")
        $out.Add("")
        foreach ($note in $noteList) { $out.Add("- $(Format-Cell $note)") }
    }
    return $out.ToArray()
}

# Os vereditos e a tabela de uma corrida, a partir do registo do exe e do
# plano. E o mesmo codigo na corrida real e no -SelfTest (com registos de
# forma real), para a tabela nao poder partir so no CI outra vez.
function Get-SpikeReport {
    param(
        $Records,
        $TrialPlan,
        [string[]]$HostList,
        [string[]]$Notes = @()
    )
    $hooked = @()
    $nativeSeen = 0
    $byTrial = @{}
    foreach ($record in $Records) {
        $kind = [string]$record.t
        if ($kind -eq "hooked") {
            if ([bool]$record.ok) { $hooked += [string]$record.host }
            continue
        }
        if ($kind -ne "native" -and $kind -ne "act") { continue }
        if ($kind -eq "native") { $nativeSeen++ }
        $key = [int]$record.trial
        if (-not $byTrial.ContainsKey($key)) {
            $byTrial[$key] = @{ native = [System.Collections.Generic.List[object]]::new(); act = [System.Collections.Generic.List[object]]::new() }
        }
        $byTrial[$key][$kind].Add($record)
    }
    $sent = @($TrialPlan.Values | Where-Object { $_.KeysSent }).Count
    # Antes dos vereditos: uma corrida em que nenhuma tecla chegou ao nativo
    # marca todas as tentativas como invalidas, e a tabela (e o resumo do job)
    # di-lo.
    $runInvalid = Get-RunInvalidReason -NativeSeen $nativeSeen -Trials $TrialPlan.Count -Sent $sent

    $verdicts = [System.Collections.Generic.List[object]]::new()
    foreach ($key in ($TrialPlan.Keys | Sort-Object)) {
        $info = $TrialPlan[$key]
        $native = @()
        $acts = @()
        if ($byTrial.ContainsKey($key)) {
            $native = $byTrial[$key].native.ToArray()
            $acts = $byTrial[$key].act.ToArray()
        }
        $verdicts.Add((Get-TrialVerdict -Chord $info.Chord -HostName $info.Host -Repeat $info.Repeat -ArmToken $info.Token -Page $info.Page -Native $native -Acts $acts -Hooked ($hooked -contains $info.Host) -InputError $info.InputError -RunInvalid $runInvalid -NotMeasured $info.NotMeasured))
    }
    $all = $verdicts.ToArray()
    $table = @(Format-SpikeTable $all $HostList $runInvalid $Notes)
    return [pscustomobject]@{
        Verdicts = $all
        RunInvalid = $runInvalid
        Table = $table
        Expected = $HostList.Count * $Chords.Count * 2
    }
}

# A politica de saida (cabecalho). Devolve se o passo falha e a mensagem.
function Get-SpikeExit {
    param(
        [object[]]$Verdicts,
        [string]$RunInvalid,
        [string]$FailedHard,
        [bool]$ExpectLeak,
        [int]$Expected
    )
    $leaks = @($Verdicts | Where-Object { $_.PageDown -gt 0 })
    $failed = @($Verdicts | Where-Object { -not $_.Pass })
    if ($ExpectLeak) {
        if ($leaks.Count -eq 0) {
            return [pscustomobject]@{ Fail = $true; Message = "Sabotagem Handled=false ficou verde: a tabela nao mostrou a pagina a ver nenhuma tecla ($($Verdicts.Count) tentativas)." }
        }
        $sample = (@($leaks | ForEach-Object { $_.Host + " " + $_.Chord }) | Select-Object -First 5) -join "; "
        return [pscustomobject]@{ Fail = $false; Message = "Prova de sabotagem: com Handled=false a pagina viu a tecla em $($leaks.Count) de $($Verdicts.Count) tentativas ($sample)." }
    }
    if ($FailedHard) {
        return [pscustomobject]@{ Fail = $true; Message = "Erro do condutor (o spike nao correu ate ao fim): $FailedHard" }
    }
    if ($RunInvalid) {
        return [pscustomobject]@{ Fail = $true; Message = "Execucao invalida (nada a decidir sobre esta tabela): $RunInvalid" }
    }
    if ($Verdicts.Count -ne $Expected) {
        return [pscustomobject]@{ Fail = $true; Message = "Tabela incompleta: $($Verdicts.Count) tentativas, esperadas $Expected." }
    }
    if ($failed.Count -gt 0) {
        $invalid = @($failed | Where-Object { (Get-FailureClass $_) -eq "invalido" }).Count
        return [pscustomobject]@{ Fail = $false; Message = "Tabela completa e corrida valida: $($failed.Count) de $($Verdicts.Count) tentativas nao passam ($($failed.Count - $invalid) pela regra do spike, $invalid invalidas); ficam na tabela para a decisao do dono (sec. 7), o job nao falha por elas." }
    }
    return [pscustomobject]@{ Fail = $false; Message = "Spike: os $($Chords.Count) atalhos passam em todos os hospedeiros, uma vez e com a tecla presa." }
}

# Uma celula da tabela geral (atalho x hospedeiro), para o -SelfTest.
function Get-TableCell([string[]]$table, [string]$chordName, [string]$hostName) {
    $header = @($table | Where-Object { $_ -like "| Atalho | *" }) | Select-Object -First 1
    if (-not $header) { return $null }
    $columns = @($header.Trim("|", " ") -split "\s*\|\s*")
    $index = [array]::IndexOf($columns, $hostName)
    $row = @($table | Where-Object { $_ -like "| $chordName | *" }) | Select-Object -First 1
    if ($index -lt 0 -or -not $row) { return $null }
    $cells = @($row.Trim("|", " ") -split "\s*\|\s*")
    return $cells[$index]
}

# Uma corrida sintetica com a forma real: linhas JSON do registo como o exe as
# escreve (lidas por ConvertFrom-SpikeLogLine, como o Update-Records), as
# respostas da sonda como string JSON dentro da linha `page` (lidas por
# ConvertFrom-PageResult, como o Invoke-Probe), o plano do New-TrialPlan e os
# nove hospedeiros, com todas as classes misturadas: ok, fallback-1 (fixture e
# act()), fallback-2, um hospedeiro que deixou de responder a meio, um sem
# foco, um SendInput falhado e o exe a sair antes do ultimo hospedeiro.
function New-SelfTestRun {
    $hostList = @("Column", "Split", "PrivateSplit", "SidePanel", "Service", "External", "Reader", "Pdf", "Epub")
    $plan = New-TrialPlan $hostList
    $lines = [System.Collections.Generic.List[string]]::new()
    foreach ($hostName in $hostList) { $lines.Add('{"t":"hooked","host":"' + $hostName + '","ok":true}') }
    $perHost = @{}
    $seq = 0
    foreach ($trial in ($plan.Keys | Sort-Object)) {
        $info = $plan[$trial]
        $hostName = $info.Host
        $chord = $info.Chord
        if (-not $perHost.ContainsKey($hostName)) { $perHost[$hostName] = 0 }
        $perHost[$hostName]++
        $index = $perHost[$hostName]
        if ($hostName -eq "Reader") {
            $info.NotMeasured = "sem foco: focus: WebView2 error: WindowsError(Error { code: HRESULT(0x80070057), message: `"The parameter is incorrect.`" })"
            $info.Done = $true
            continue
        }
        if ($hostName -eq "Epub") {
            $info.NotMeasured = "O NeuralIA saiu (codigo -1073741819) a espera de: ack de 'open Epub'"
            $info.Done = $true
            continue
        }
        if ($hostName -eq "External" -and $index -gt 4) {
            $info.NotMeasured = "o hospedeiro deixou de responder: Sem resposta em 15000 ms: resposta da pagina a 'pull External'"
            $info.Done = $true
            continue
        }
        $token = "tok-$hostName"
        $info.Token = $token
        $info.KeysSent = $true
        $noNative = ($hostName -eq "PrivateSplit" -and $chord.Name -eq "Ctrl+O") -or ($hostName -eq "Pdf" -and $chord.Name -eq "F1" -and -not $info.Repeat)
        if ($hostName -eq "Pdf" -and $chord.Name -eq "F1" -and -not $info.Repeat) {
            $info.InputError = "SendInput recusou a tecla 112 (erro 5)"
        }
        if (-not $noNative) {
            $downs = if ($info.Repeat) { $RepeatDowns } else { 1 }
            for ($i = 0; $i -lt $downs; $i++) {
                $fired = if ($i -eq 0) { "true" } else { "false" }
                $again = if ($i -gt 0) { "true" } else { "false" }
                $lines.Add('{"t":"native","trial":' + $trial + ',"host":"' + $hostName + '","chord":"' + $chord.Name + '","kind":"down","handled":true,"fired":' + $fired + ',"repeat":' + $again + '}')
            }
            $lines.Add('{"t":"native","trial":' + $trial + ',"host":"' + $hostName + '","chord":"' + $chord.Name + '","kind":"up","handled":true,"fired":false,"repeat":false}')
        }
        if ($hostName -eq "Service" -and $chord.Name -eq "Ctrl+Shift+N" -and -not $info.Repeat) {
            $lines.Add('{"t":"act","trial":' + $trial + ',"act":"newtab"}')
        }
        $fixtureDown = "null"
        $fixturePress = "null"
        if ($FixtureHosts -contains $hostName) {
            $fixtureDown = "[]"
            $fixturePress = "[]"
            if ($hostName -eq "Split" -and $chord.Name -eq "Ctrl+Shift+F") {
                $fixtureDown = '[{"key":"F","code":"KeyF","ctrl":true,"shift":true,"repeat":false,"trusted":true}]'
            }
        }
        $raw = '{"token":"' + $token + '","down":[],"press":[],"fixtureDown":' + $fixtureDown + ',"fixturePress":' + $fixturePress + ',"find":false,"focus":true,"active":"BODY","href":"http://127.0.0.1:5123/fixture.html"}'
        $seq++
        $pageLine = '{"t":"page","seq":' + $seq + ',"trial":' + $trial + ',"host":"' + $hostName + '","phase":"pull","result":' + (ConvertTo-Json -InputObject $raw -Compress) + '}'
        $info.Page = ConvertFrom-PageResult (ConvertFrom-SpikeLogLine $pageLine)
        $info.Done = $true
    }
    # Como na corrida real: a lista do registo e uma List[object] ...
    $records = [System.Collections.Generic.List[object]]::new()
    foreach ($line in $lines) {
        $record = ConvertFrom-SpikeLogLine $line
        if ($null -ne $record) { $records.Add($record) }
    }
    # ... e a mesma lista embrulhada num PSObject (o que o New-Object da): o
    # relatorio tem de a ler das duas formas.
    $wrapped = New-Object -TypeName "System.Collections.Generic.List[object]"
    foreach ($record in $records) { $wrapped.Add($record) }
    $notes = @(
        "Column: a fixture de 127.0.0.1 nao carregou na coluna (a sonda viu https://www.google.com/search); medida na pagina ao vivo, que depende da rede",
        "Reader: invalido (20 tentativa(s)): sem foco: focus: WebView2 error",
        "External: invalido (16 tentativa(s)): o hospedeiro deixou de responder | com barra"
    )
    return [pscustomobject]@{ Hosts = $hostList; Plan = $plan; Records = $records; Wrapped = $wrapped; Notes = $notes }
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
        @{ Name = "corrida sem input"; Repeat = $true; Page = $clean; Native = @(); Acts = @(); Hooked = $true; RunInvalid = (Get-RunInvalidReason -NativeSeen 0 -Trials 2); Pass = $false; Class = "invalido" },
        # O hospedeiro nao ficou com o foco (ou deixou de responder): a
        # tentativa nao foi medida, mesmo que o registo tenha uma fuga dela.
        @{ Name = "nao medida"; Repeat = $false; Page = $leak; Native = @(); Acts = $newtab; Hooked = $true; NotMeasured = "sem foco: focus: WebView2 error"; Pass = $false; Class = "invalido" }
    )
    $failures = 0
    $verdicts = @()
    foreach ($case in $cases) {
        $verdict = Get-TrialVerdict -Chord $chord -HostName "Split" -Repeat $case.Repeat -ArmToken "t1" -Page $case.Page -Native $case.Native -Acts $case.Acts -Hooked $case.Hooked -InputError ([string]$case.InputError) -RunInvalid ([string]$case.RunInvalid) -NotMeasured ([string]$case.NotMeasured)
        $class = Get-FailureClass $verdict
        $ok = ($verdict.Pass -eq $case.Pass) -and ($class -eq $case.Class)
        if (-not $ok) { $failures++ }
        Write-Host ("self-test {0,-20} pass={1,-5} class={2,-10} {3} {4}" -f $case.Name, $verdict.Pass, $class, $(if ($ok) { "ok" } else { "ERRADO" }), ($verdict.Reasons -join "; "))
        $verdicts += $verdict
    }
    $table = @(Format-SpikeTable $verdicts @("Split"))
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

    # A corrida real em ponto pequeno: nove hospedeiros, 180 tentativas, o
    # registo e as paginas na forma que o exe as escreve, pelo mesmo
    # Get-SpikeReport da corrida real. Foi aqui que a primeira corrida do CI
    # partiu ("Argument types do not match"), com o -SelfTest verde.
    $run = New-SelfTestRun
    $realChecks = @()
    $reports = @()
    foreach ($source in @(@{ Name = "List[object]"; Records = $run.Records }, @{ Name = "List[object] do New-Object"; Records = $run.Wrapped })) {
        try {
            $reports += Get-SpikeReport -Records $source.Records -TrialPlan $run.Plan -HostList $run.Hosts -Notes $run.Notes
            $realChecks += @{ Name = "relatorio sobre $($source.Name)"; Ok = $true; Detail = "" }
        }
        catch {
            $realChecks += @{ Name = "relatorio sobre $($source.Name)"; Ok = $false; Detail = $_.Exception.Message }
        }
    }
    if ($reports.Count -eq 2) {
        $report = $reports[0]
        $realTable = $report.Table
        $expectedClasses = [ordered]@{
            Column = "ok"; Split = "fallback-1,ok"; PrivateSplit = "fallback-2,ok"; SidePanel = "ok"; Service = "fallback-1,ok"
            External = "invalido,ok"; Reader = "invalido"; Pdf = "invalido,ok"; Epub = "invalido"
        }
        foreach ($hostName in $expectedClasses.Keys) {
            $seen = (@($report.Verdicts | Where-Object { $_.Host -eq $hostName } | ForEach-Object { Get-FailureClass $_ }) | Sort-Object -Unique) -join ","
            $realChecks += @{ Name = "classes de $hostName"; Ok = ($seen -eq $expectedClasses[$hostName]); Detail = "viu $seen, esperava $($expectedClasses[$hostName])" }
        }
        $expectedCells = @(
            @("Ctrl+D", "Column", "ok"), @("Ctrl+Shift+F", "Split", "FALHA (fallback-1)"), @("Ctrl+O", "PrivateSplit", "FALHA (fallback-2)"),
            @("Ctrl+Shift+N", "Service", "FALHA (fallback-1)"), @("Ctrl+D", "External", "ok"), @("Ctrl+O", "External", "FALHA (invalido)"),
            @("Ctrl+D", "Reader", "FALHA (invalido)"), @("F1", "Pdf", "FALHA (invalido)"), @("Ctrl+O", "Epub", "FALHA (invalido)")
        )
        foreach ($cell in $expectedCells) {
            $got = Get-TableCell $realTable $cell[0] $cell[1]
            $realChecks += @{ Name = "celula $($cell[0]) x $($cell[1])"; Ok = ($got -eq $cell[2]); Detail = "viu '$got'" }
        }
        $realChecks += @{ Name = "180 tentativas"; Ok = ($report.Verdicts.Count -eq 180 -and $report.Expected -eq 180); Detail = "$($report.Verdicts.Count)/$($report.Expected)" }
        $realChecks += @{ Name = "corrida valida"; Ok = ($report.RunInvalid -eq "" -and -not ($realTable -cmatch "INVALID RUN")); Detail = $report.RunInvalid }
        $realChecks += @{ Name = "hospedeiros na tabela"; Ok = (@($run.Hosts | Where-Object { $realTable -contains "### $_" }).Count -eq 9); Detail = "" }
        $realChecks += @{ Name = "notas e barra escapada"; Ok = (@($realTable -like "- Column: a fixture de 127.0.0.1*").Count -eq 1 -and @($realTable -like "*deixou de responder \| com barra*").Count -eq 1); Detail = "" }
        $realChecks += @{ Name = "resumo"; Ok = (@($realTable -like "**Resumo:** 180 tentativas: *").Count -eq 1); Detail = "" }
        $realChecks += @{ Name = "mesma tabela das duas listas"; Ok = (($reports[0].Table -join "`n") -eq ($reports[1].Table -join "`n")); Detail = "" }

        # A politica de saida sobre a mesma tabela.
        $all = $report.Verdicts
        $exitCases = @(
            @{ Name = "atalhos falham, corrida valida -> 0"; Exit = (Get-SpikeExit -Verdicts $all -RunInvalid "" -FailedHard "" -ExpectLeak $false -Expected 180); Fail = $false },
            @{ Name = "erro do condutor -> erro"; Exit = (Get-SpikeExit -Verdicts $all -RunInvalid "" -FailedHard "O NeuralIA saiu" -ExpectLeak $false -Expected 180); Fail = $true },
            @{ Name = "corrida invalida -> erro"; Exit = (Get-SpikeExit -Verdicts $all -RunInvalid $runReason -FailedHard "" -ExpectLeak $false -Expected 180); Fail = $true },
            @{ Name = "tabela incompleta -> erro"; Exit = (Get-SpikeExit -Verdicts $all -RunInvalid "" -FailedHard "" -ExpectLeak $false -Expected 181); Fail = $true },
            @{ Name = "sabotagem com fuga -> 0"; Exit = (Get-SpikeExit -Verdicts $all -RunInvalid "" -FailedHard "O NeuralIA saiu" -ExpectLeak $true -Expected 180); Fail = $false },
            @{ Name = "sabotagem sem fuga -> erro"; Exit = (Get-SpikeExit -Verdicts @($all | Where-Object { $_.PageDown -eq 0 }) -RunInvalid "" -FailedHard "" -ExpectLeak $true -Expected 180); Fail = $true }
        )
        foreach ($case in $exitCases) {
            $realChecks += @{ Name = "saida: $($case.Name)"; Ok = ($case.Exit.Fail -eq $case.Fail); Detail = $case.Exit.Message }
        }
    }
    foreach ($check in $realChecks) {
        if (-not $check.Ok) { $failures++ }
        Write-Host ("self-test {0,-44} {1} {2}" -f $check.Name, $(if ($check.Ok) { "ok" } else { "ERRADO" }), $check.Detail)
    }
    if ($failures -gt 0 -and $reports.Count -gt 0) { $reports[0].Table | ForEach-Object { Write-Host $_ } }

    if ($failures -gt 0) { throw "self-test do avaliador: $failures caso(s) errado(s)." }
    Write-Host "self-test do avaliador: $($cases.Count + $runChecks + $realChecks.Count) casos certos."
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
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
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

    delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lparam);

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
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lparam);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int max);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr wparam, IntPtr lparam);

    const uint INPUT_KEYBOARD = 1;
    const uint KEYEVENTF_KEYUP = 0x2;
    const ushort VK_SHIFT = 0x10;
    const ushort VK_CONTROL = 0x11;
    const uint GA_ROOTOWNER = 3;
    const uint WM_CLOSE = 0x10;

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

    // Os dialogos modais (classe #32770) cujo dono e a janela do NeuralIA --
    // de qualquer processo: o de impressao de uma tecla que escapou e do
    // processo do WebView2 -- recebem WM_CLOSE (= Cancelar). Devolve os
    // titulos, para a nota da tabela.
    public static string[] CloseOwnedDialogs(IntPtr owner) {
        var closed = new List<string>();
        if (owner == IntPtr.Zero) { return closed.ToArray(); }
        EnumWindows(delegate (IntPtr hwnd, IntPtr lparam) {
            if (hwnd != owner && IsWindowVisible(hwnd) && GetAncestor(hwnd, GA_ROOTOWNER) == owner) {
                var name = new StringBuilder(64);
                GetClassName(hwnd, name, name.Capacity);
                if (name.ToString() == "#32770") {
                    var title = new StringBuilder(256);
                    GetWindowText(hwnd, title, title.Capacity);
                    PostMessage(hwnd, WM_CLOSE, IntPtr.Zero, IntPtr.Zero);
                    closed.Add(title.ToString());
                }
            }
            return true;
        }, IntPtr.Zero);
        return closed.ToArray();
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
$utf8 = [Text.UTF8Encoding]::new($false)

# A lista do registo: `::new()`, nunca `New-Object` (ver Get-List).
$script:Records = [System.Collections.Generic.List[object]]::new()
$script:Notes = [System.Collections.Generic.List[string]]::new()
$script:LogOffset = 0
$script:LogTail = ""
$script:Seq = 0
$script:ExeGone = ""
$script:FixtureUrl = ""

function Add-SpikeNote([string]$text) {
    $script:Notes.Add($text)
    Write-Host "  nota: $text"
}

# Um endereco para o registo e a nota: sem a query (a pagina ao vivo da coluna
# leva tokens de sessao nela).
function Format-Href([string]$href) {
    $uri = $null
    if ([Uri]::TryCreate($href, [UriKind]::Absolute, [ref]$uri) -and $uri.Scheme -like "http*") {
        return $uri.GetLeftPart([UriPartial]::Path)
    }
    return $href
}

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
        $record = ConvertFrom-SpikeLogLine $line
        if ($null -ne $record) { $script:Records.Add($record) }
    }
}

function Wait-Record([scriptblock]$match, [int]$timeoutMs, [string]$what) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $timeoutMs) {
        Update-Records
        foreach ($record in $script:Records) {
            if (& $match $record) { return $record }
        }
        if ($script:Process.HasExited) {
            $script:ExeGone = "O NeuralIA saiu (codigo $($script:Process.ExitCode)) a espera de: $what"
            throw $script:ExeGone
        }
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
    [IO.File]::WriteAllText($temp, $line + "`n", $utf8)
    Move-Item -LiteralPath $temp -Destination $commandPath
    return $script:Seq
}

function Invoke-Spike([string]$verb, [string]$hostName, [string]$argument, [int]$timeoutMs = 15000) {
    $seq = Send-SpikeCommand $verb $hostName $argument
    $ack = Wait-Record { param($r) $r.t -eq "ack" -and [int64]$r.seq -eq $seq } $timeoutMs "ack de '$verb $hostName'"
    return $ack
}

# Corre um script da sonda e devolve o que a pagina respondeu (objeto), ou
# $null se o exe recusou o comando ou a pagina nao devolveu nada. Um prazo
# esgotado (sem ack, ou sem a resposta da pagina) LANCA: o hospedeiro deixou
# de responder e a corrida passa ao seguinte. So o `arm` (-Tolerant), que
# espera a pagina acabar de carregar, tolera uma resposta que nao chega.
function Invoke-Probe([string]$phase, [string]$hostName, [int]$trial, [switch]$Tolerant) {
    $argument = if ($trial -gt 0) { [string]$trial } else { "" }
    $seq = Send-SpikeCommand $phase $hostName $argument
    $ack = Wait-Record { param($r) $r.t -eq "ack" -and [int64]$r.seq -eq $seq } 15000 "ack de '$phase $hostName'"
    if (-not $ack.ok) { Write-Host "  $phase ${hostName}: $($ack.detail)"; return $null }
    try {
        $page = Wait-Record { param($r) $r.t -eq "page" -and [int64]$r.seq -eq $seq } 15000 "resposta da pagina a '$phase $hostName'"
    }
    catch {
        if ($script:ExeGone -or -not $Tolerant) { throw }
        Write-Host "  $phase ${hostName}: $($_.Exception.Message)"
        return $null
    }
    return ConvertFrom-PageResult $page
}

# Janela em primeiro plano e o teclado na WebView do hospedeiro. Devolve
# Ok e, se falhou, o ultimo motivo (o ack do exe e o primeiro plano).
function Set-Foreground([string]$hostName) {
    $detail = ""
    for ($attempt = 1; $attempt -le 4; $attempt++) {
        $ack = Invoke-Spike "focus" $hostName ""
        Start-Sleep -Milliseconds 150
        $foreground = [AccelSpikeInput]::GetForegroundWindow()
        $ours = [AccelSpikeInput]::WindowPid($foreground) -eq [uint32]$script:Process.Id
        if ($ack.ok -and $ours) { return [pscustomobject]@{ Ok = $true; Detail = "" } }
        $detail = "ack=$($ack.ok) '$($ack.detail)' primeiro-plano-nosso=$ours"
        Write-Host "  foco ${hostName} (tentativa $attempt): $detail"
        $script:Process.Refresh()
        if ($script:Process.MainWindowHandle -ne [IntPtr]::Zero) {
            $null = [AccelSpikeInput]::BringToFront($script:Process.MainWindowHandle)
        }
        Start-Sleep -Milliseconds 250
    }
    return [pscustomobject]@{ Ok = $false; Detail = $detail }
}

# A sonda no documento que vai receber as teclas, depois de ele acabar de
# carregar (e, com `$requireHref`, so nessa pagina). Devolve a resposta da
# sonda armada ($null se nao armou no prazo) e a ultima que se viu.
function Wait-Armed([string]$hostName, [string]$requireHref, [int]$budgetMs) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $last = $null
    while ($watch.ElapsedMilliseconds -lt $budgetMs) {
        $armed = Invoke-Probe "arm" $hostName 0 -Tolerant
        if ($null -ne $armed) { $last = $armed }
        $ready = $null -ne $armed -and [string]$armed.ready -eq "complete"
        if ($ready -and $requireHref) { $ready = ([string]$armed.href).StartsWith($requireHref) }
        if ($ready) { return [pscustomobject]@{ Armed = $armed; Last = $armed } }
        Start-Sleep -Milliseconds 500
    }
    return [pscustomobject]@{ Armed = $null; Last = $last }
}

# No fim de cada hospedeiro: um dialogo modal que uma tecla abriu (o de
# impressao do Ctrl+Shift+P quando o Handled nao o trava) deixa a janela
# desativada e os hospedeiros seguintes sem foco. Fecha-o e anota-o.
function Close-StrayDialogs([string]$hostName) {
    if (-not $script:Process -or $script:Process.HasExited) { return }
    try {
        $script:Process.Refresh()
        $closed = @([AccelSpikeInput]::CloseOwnedDialogs($script:Process.MainWindowHandle))
        if ($closed.Count -gt 0) {
            Add-SpikeNote "${hostName}: $($closed.Count) dialogo(s) modal(is) com dono no NeuralIA fechado(s) no fim do hospedeiro ('$($closed -join "', '")')"
            Start-Sleep -Milliseconds 500
        }
    }
    catch {
        Write-Host "  dialogos de ${hostName}: $($_.Exception.Message)"
    }
}

# Um hospedeiro inteiro: abre, arma a sonda, poe-lhe o foco e corre as
# tentativas dele. Um problema que impede medir o hospedeiro LANCA; quem
# chama marca as tentativas que faltam como nao medidas e segue.
function Invoke-HostTrials([string]$hostName, [int[]]$trials) {
    $isFixtureHost = $FixtureHosts -contains $hostName
    $argument = ""
    if ($isFixtureHost -or $hostName -eq "Column") { $argument = $script:FixtureUrl }
    $opened = $null
    $watch = [Diagnostics.Stopwatch]::StartNew()
    do {
        $opened = Invoke-Spike "open" $hostName $argument 30000
        if ($opened.ok) { break }
        Start-Sleep -Milliseconds 500
    } while ($watch.ElapsedMilliseconds -lt 45000)
    if (-not $opened.ok) { throw "nao abriu: $($opened.detail)" }

    # A pagina acabou de carregar (e, nos web, e a fixture): so entao a
    # sonda vai para o documento que vai receber as teclas.
    if ($hostName -eq "Column") {
        $arm = Wait-Armed $hostName $script:FixtureUrl 20000
        if ($null -ne $arm.Armed) {
            Add-SpikeNote "Column: medida na fixture de 127.0.0.1 (a excecao de navegacao da coluna so existe no exe do spike; o gate que embarca nao muda)"
        }
        else {
            $seen = if ($null -ne $arm.Last) { Format-Href ([string]$arm.Last.href) } else { "sem resposta da sonda" }
            Add-SpikeNote "Column: a fixture de 127.0.0.1 nao carregou na coluna (a sonda viu $seen); medida na pagina ao vivo que o NEURALIA_STARTUP_INPUT abriu, que depende da rede"
            $arm = Wait-Armed $hostName "" 45000
        }
    }
    else {
        $requireHref = if ($isFixtureHost) { $script:FixtureUrl } else { "" }
        $arm = Wait-Armed $hostName $requireHref 45000
    }
    if ($null -eq $arm.Armed) {
        $seen = if ($null -ne $arm.Last) { "$(Format-Href ([string]$arm.Last.href)) ($($arm.Last.ready))" } else { "sem resposta da sonda" }
        throw "a sonda nao ficou armada (ultima: $seen)"
    }
    Write-Host "  sonda em $(Format-Href ([string]$arm.Armed.href))"

    $focus = Set-Foreground $hostName
    if (-not $focus.Ok) { throw "sem foco: $($focus.Detail)" }

    $strikes = 0
    foreach ($trial in $trials) {
        $info = $script:Plan[$trial]
        # O begin volta a armar a sonda (se a pagina navegou entre
        # tentativas) e zera-a; o token dele e o que o pull tem de ver.
        $begin = Invoke-Probe "begin" $hostName $trial
        $skip = ""
        if ($null -eq $begin) {
            $skip = "a sonda nao respondeu ao begin"
        }
        else {
            $info.Token = [string]$begin.token
            if (-not [bool]$begin.focus) {
                $refocus = Set-Foreground $hostName
                if (-not $refocus.Ok) { $skip = "sem foco nesta tentativa: $($refocus.Detail)" }
            }
        }
        if ($skip) {
            # Sem teclas: sem foco, a tecla cairia noutro sitio.
            $info.NotMeasured = $skip
            $info.Done = $true
            $strikes++
            if ($strikes -ge 3) { throw "3 tentativas seguidas sem sonda ou sem foco ($skip)" }
            continue
        }
        $strikes = 0
        $downs = if ($info.Repeat) { $RepeatDowns } else { 1 }
        $info.KeysSent = $true
        try {
            [AccelSpikeInput]::Chord([uint16]$info.Chord.Vk, $info.Chord.Ctrl, $info.Chord.Shift, $downs, 40)
        }
        catch {
            # A tecla pode nao ter saido: a tentativa fica invalida
            # (nunca fallback-2 por um nativo que nao a recebeu).
            $info.InputError = $_.Exception.Message
            Write-Host "  SendInput: $($_.Exception.Message)"
        }
        Start-Sleep -Milliseconds $SettleMs
        $info.Page = Invoke-Probe "pull" $hostName $trial
        $info.Done = $true
    }
}

$fixture = $null
$script:Process = $null
$script:Plan = New-TrialPlan $Hosts
$failedHard = ""

try {
    $fixture = Start-Process -FilePath "node" -ArgumentList @("`"$(Join-Path $PSScriptRoot 'accel-spike-fixture.mjs')`"", "`"$portFile`"") -PassThru -NoNewWindow
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $portFile)) {
        if ($watch.ElapsedMilliseconds -gt 10000 -or $fixture.HasExited) { throw "A fixture de 127.0.0.1 nao arrancou." }
        Start-Sleep -Milliseconds 50
    }
    $port = ([IO.File]::ReadAllText($portFile)).Trim()
    $script:FixtureUrl = "http://127.0.0.1:$port/fixture.html"
    Write-Host "fixture: $($script:FixtureUrl)"

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
        $trials = @($script:Plan.Keys | Where-Object { $script:Plan[$_].Host -eq $hostName } | Sort-Object)
        try {
            Invoke-HostTrials $hostName $trials
        }
        catch {
            # O exe saiu: nada mais se mede (o catch de fora marca o resto).
            if ($script:ExeGone) { throw }
            $reason = $_.Exception.Message
            $left = @($trials | Where-Object { -not $script:Plan[$_].Done })
            foreach ($trial in $left) {
                $script:Plan[$trial].NotMeasured = $reason
                $script:Plan[$trial].Done = $true
            }
            Write-Host "  $hostName invalido: $reason"
            Add-SpikeNote "${hostName}: invalido em $($left.Count) de $($trials.Count) tentativa(s): $reason"
        }
        Close-StrayDialogs $hostName
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

# O que nao chegou a ser medido (o condutor parou antes) fica invalido com o
# motivo: a tabela tem sempre todas as linhas.
if ($failedHard) {
    $left = @($script:Plan.Keys | Where-Object { -not $script:Plan[$_].Done })
    foreach ($trial in $left) {
        $script:Plan[$trial].NotMeasured = "nao chegou a ser medida: $failedHard"
        $script:Plan[$trial].Done = $true
    }
    Add-SpikeNote "Erro do condutor ($($left.Count) tentativa(s) nao medidas): $failedHard"
}

try { Update-Records } catch { Write-Host "registo: $($_.Exception.Message)" }
foreach ($failure in @($script:Records | Where-Object { $_.t -eq "hooked" -and -not $_.ok })) {
    Add-SpikeNote "handler nao instalado em $($failure.host): $($failure.error)"
}

$tableFile = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($TablePath)
$title = if ($ExpectLeak) { "# accel-spike (sabotagem Handled=false)" } else { "# accel-spike" }
try {
    $report = Get-SpikeReport -Records $script:Records -TrialPlan $script:Plan -HostList $Hosts -Notes $script:Notes.ToArray()
}
catch {
    # Um erro do proprio condutor: mesmo assim sai uma tabela, a dize-lo.
    $message = "ERRO DO CONDUTOR a montar a tabela: $($_.Exception.Message)"
    $fallback = @("## Spike do AcceleratorKeyPressed (infra-accel-spike)", "", "**$message**")
    [IO.File]::WriteAllLines($tableFile, [string[]]$fallback, $utf8)
    if ($env:GITHUB_STEP_SUMMARY) { [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, (((@($title) + $fallback) -join "`n") + "`n"), $utf8) }
    throw
}
$table = $report.Table
$table | ForEach-Object { Write-Host $_ }
[IO.File]::WriteAllLines($tableFile, [string[]]$table, $utf8)
if ($env:GITHUB_STEP_SUMMARY) {
    [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, (((@($title) + $table) -join "`n") + "`n"), $utf8)
}

$failed = @($report.Verdicts | Where-Object { -not $_.Pass })
if ($failed.Count -gt 0 -or $failedHard) {
    Write-Host "--- ultimas linhas do registo do spike"
    Get-Content -LiteralPath $logPath -Tail 60 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host $_ }
    Write-Host "--- ultimas linhas do NEURALIA_DEBUG_LOG"
    Get-Content -LiteralPath $debugLog -Tail 40 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host $_ }
}

$exit = Get-SpikeExit -Verdicts $report.Verdicts -RunInvalid $report.RunInvalid -FailedHard $failedHard -ExpectLeak ([bool]$ExpectLeak) -Expected $report.Expected
if ($env:GITHUB_STEP_SUMMARY) {
    [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, "`n**Saida do passo:** $(if ($exit.Fail) { 'FALHA' } else { 'ok' }) -- $($exit.Message)`n", $utf8)
}
if ($exit.Fail) { throw $exit.Message }
Write-Host $exit.Message
