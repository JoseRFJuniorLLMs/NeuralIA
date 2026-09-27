param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    # Quanto se espera pelo exe pedir a pagina da fixture.
    [int]$StartupSec = 60,
    # Quanto se deixa a sessao assentar depois da pagina (os workers do
    # historico e da memoria gravam fora do event loop).
    [int]$SettleSec = 8
)

# Fase 0 do E2E do modo privado (infra-privacy-guard, plano 2.4; SPEC-0006
# «Persistence chokepoint»). SO CI: abre o exe testado (ci-tested/NeuralIA.exe)
# numa pasta de dados temporaria e nunca sai da maquina.
#
# O que faz:
# - imprime a lista das lojas que uma sessao Normal pode criar ou mudar em
#   <data_dir> (a tabela da fase 0 da SPEC-0006 -- e a lista que os briefs
#   seguintes estendem com o que o modo privado NAO pode tocar);
# - faz o hash de cada ficheiro debaixo de uma NEURALIA_DATA_DIR temporaria
#   antes e depois de uma sessao Normal guiada: o exe abre a pagina da
#   fixture de 127.0.0.1 (scripts/private-mode-fixture.mjs) no Reader (`read:`),
#   assenta uns segundos e fecha;
# - lista o que a sessao criou ou mudou, agrupado pela linha da lista que o
#   cobre, e falha se algum caminho nao tiver linha nenhuma;
# - falha tambem se a sessao nao chegou ao historico e a memoria: uma
#   sessao que nao escreve nada nao prova nada sobre o que ela pode escrever.
$ErrorActionPreference = "Stop"
$resolved = (Resolve-Path -LiteralPath $ExePath).Path
$node = (Get-Command node -ErrorAction Stop).Source
$utf8 = [Text.UTF8Encoding]::new($false)

# A lista (SPEC-0006, tabela da fase 0). Um caminho que acaba em "/" cobre a
# pasta inteira; os outros sao um ficheiro exato (os trincos `.lock` ao lado
# de uma loja tem a sua linha). Kind: Automatic = efeito lateral do uso (o
# que o modo privado vai deixar de escrever); Setting = escolha num menu;
# Explicit = o utilizador pediu para guardar. O gate
# `the_e2e_allowlist_covers_a_normal_guard_session` (windows_app/tests.rs)
# le esta lista e o Find-AllowlistRow abaixo, conduz uma sessao Normal do
# PrivacyGuard e falha se ela deixar um ficheiro sem linha.
$allowlist = @(
    @{ Path = 'history.jsonl'; Kind = 'Automatic'; Why = 'o historico cronologico, so pelo PrivacyGuard::record' }
    @{ Path = 'history.jsonl.lock'; Kind = 'Automatic'; Why = 'o trinco dos escritores do historico (HistoryStore): nasce na primeira gravacao e fica' }
    @{ Path = 'memory/'; Kind = 'Automatic'; Why = 'a memoria semantica (documentos, wiki, indice SQLite derivado, sessions/), so pelo PrivacyGuard::capture/save_session' }
    @{ Path = 'tabs.json'; Kind = 'Automatic'; Why = 'as abas e os grupos do comparador, so pelo PrivacyGuard::save_tabs' }
    @{ Path = 'tabs.lock'; Kind = 'Automatic'; Why = 'o trinco da primeira janela (aberto pelo PrivacyGuard)' }
    @{ Path = 'tabs.cleared'; Kind = 'Automatic'; Why = 'a geracao do Apagar historico' }
    @{ Path = 'WebView2/'; Kind = 'Automatic'; Why = 'o perfil do WebView2 (cookies, cache, inicios de sessao): o runtime escreve-o; o modo privado do perfil e do private-mode-core' }
    @{ Path = 'panel-width.json'; Kind = 'Automatic'; Why = 'a largura dos paineis, ao arrastar a pega (grant = privacy-guard-retrofit)' }
    @{ Path = 'agent/'; Kind = 'Automatic'; Why = 'o trace e a auditoria legados do agente (int-agents-finish leva-os)' }
    @{ Path = 'theme'; Kind = 'Setting'; Why = 'a escolha do tema' }
    @{ Path = 'gmail'; Kind = 'Setting'; Why = 'os avisos do Gmail' }
    @{ Path = 'pomodoro'; Kind = 'Setting'; Why = 'as duracoes do Pomodoro' }
    @{ Path = 'adblock-settings.json'; Kind = 'Setting'; Why = 'o bloqueio ligado e os sites permitidos' }
    @{ Path = 'adblock-list.json'; Kind = 'Automatic'; Why = 'a lista baixada (renovacao semanal)' }
    @{ Path = 'downloads.json'; Kind = 'Automatic'; Why = 'o registo dos downloads acabados' }
    @{ Path = 'downloads-settings.json'; Kind = 'Setting'; Why = 'a pasta dos downloads e Permitir baixar programas' }
    @{ Path = 'ai/'; Kind = 'Setting'; Why = 'ai/settings.json (finalidades e limite) e ai/usage.json (o consumo do mes, Setting para o limite sobreviver ao modo privado)' }
    @{ Path = 'translate.json'; Kind = 'Setting'; Why = 'Sempre neste site da Traducao' }
    @{ Path = 'bookmarks.json'; Kind = 'Explicit'; Why = 'os favoritos' }
    @{ Path = 'bookmarks.json.lock'; Kind = 'Explicit'; Why = 'o trinco dos favoritos entre janelas' }
    @{ Path = 'keys/'; Kind = 'Explicit'; Why = 'as chaves de API (DPAPI)' }
    @{ Path = 'gemini-live.key'; Kind = 'Explicit'; Why = 'a chave do Gemini Live' }
    @{ Path = 'zettel/'; Kind = 'Explicit'; Why = 'as notas' }
    @{ Path = 'library/'; Kind = 'Explicit'; Why = 'os livros (books/, covers/, index.json); state/ e Automatic (privacy-guard-retrofit)' }
    @{ Path = 'research-exports/'; Kind = 'Explicit'; Why = 'Exportar pesquisa' }
)

function Find-AllowlistRow([string]$Relative) {
    foreach ($row in $allowlist) {
        if ($row.Path.EndsWith('/')) {
            if ($Relative.StartsWith($row.Path, [StringComparison]::OrdinalIgnoreCase)) { return $row }
        } elseif ($Relative -ieq $row.Path) {
            return $row
        }
    }
    return $null
}

# Caminho relativo (com /) -> SHA-256 de cada ficheiro debaixo de $Dir.
function Get-Snapshot([string]$Dir) {
    $out = @{}
    if (-not (Test-Path -LiteralPath $Dir)) { return $out }
    $base = (Resolve-Path -LiteralPath $Dir).Path.TrimEnd('\') + '\'
    foreach ($file in Get-ChildItem -LiteralPath $Dir -Recurse -File -Force -ErrorAction SilentlyContinue) {
        $relative = $file.FullName.Substring($base.Length).Replace('\', '/')
        try {
            $out[$relative] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
        } catch {
            # Um ficheiro preso pelo WebView2 (o perfil): conta como mudado.
            $out[$relative] = "locked:" + [guid]::NewGuid().ToString("N")
        }
    }
    return $out
}

function Wait-Until([scriptblock]$Condition, [int]$Seconds) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $Seconds) {
        $value = & $Condition
        if ($value) { return $value }
        Start-Sleep -Milliseconds 250
    }
    return & $Condition
}

$root = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-private-mode-" + [guid]::NewGuid().ToString("N"))
$fixtureDir = Join-Path $root "fixture"
$data = Join-Path $root "data"
$log = Join-Path $root "debug.log"
New-Item -ItemType Directory -Force -Path $fixtureDir, $data | Out-Null

function Get-FixtureEvents {
    $file = Join-Path $fixtureDir "events.jsonl"
    if (-not (Test-Path -LiteralPath $file)) { return @() }
    try { return @([IO.File]::ReadAllLines($file) | ForEach-Object { $_ | ConvertFrom-Json }) } catch { return @() }
}

Write-Host "Modo privado, fase 0: a lista do que uma sessao Normal pode escrever em <data_dir> (SPEC-0006):"
foreach ($row in $allowlist) {
    Write-Host ("  {0,-24} {1,-9} {2}" -f $row.Path, $row.Kind, $row.Why)
}

$fixture = $null
$process = $null
$failures = New-Object System.Collections.Generic.List[string]
$summary = New-Object System.Collections.Generic.List[string]
try {
    $before = Get-Snapshot $data
    if ($before.Count -ne 0) { throw "a pasta de dados temporaria nao esta vazia." }

    $fixture = Start-Process -FilePath $node -ArgumentList @((Join-Path $PSScriptRoot "private-mode-fixture.mjs"), $fixtureDir) -PassThru -WindowStyle Hidden
    $portFile = Join-Path $fixtureDir "port"
    $up = Wait-Until { (Test-Path -LiteralPath $portFile) -and -not $fixture.HasExited } 10
    if (-not $up) { throw "A fixture de 127.0.0.1 nao arrancou." }
    $port = ([IO.File]::ReadAllText($portFile)).Trim()
    $url = "http://127.0.0.1:$port/page.html"
    Write-Host "fixture: $url"

    $env:NEURALIA_DATA_DIR = $data
    $env:NEURALIA_DEBUG_LOG = $log
    # `read:` abre a pagina no Reader, que a grava no historico
    # (PrivacyGuard::record) e a captura na memoria (PrivacyGuard::capture,
    # capture_reader_memory). A Web completa (`web:`) nao serve aqui: o
    # open_external nao captura na memoria uma origem local (a fixture de
    # 127.0.0.1), por isso a primeira corrida no CI so chegou ao historico.
    $env:NEURALIA_STARTUP_INPUT = "read:$url"
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_REDUCE_MOTION = "1"
    try {
        $process = Start-Process -FilePath $resolved -PassThru
    } finally {
        foreach ($name in "NEURALIA_DATA_DIR", "NEURALIA_DEBUG_LOG", "NEURALIA_STARTUP_INPUT", "NEURALIA_NO_GMAIL", "NEURALIA_REDUCE_MOTION") {
            Remove-Item -Path "Env:$name" -ErrorAction SilentlyContinue
        }
    }
    Write-Host "exe $($process.Id) abre $url numa sessao Normal sobre $data"

    $seen = Wait-Until {
        if ($process.HasExited) { throw "o exe saiu (codigo $($process.ExitCode)) antes de pedir a pagina." }
        @(Get-FixtureEvents | Where-Object { $_.kind -eq 'request' -and $_.path -eq '/page.html' }).Count -gt 0
    } $StartupSec
    if (-not $seen) { throw "o exe nao pediu /page.html em $StartupSec s." }
    Start-Sleep -Seconds $SettleSec

    # Fechar como o dono fecha (WM_CLOSE: o `exiting` grava as abas); se nao
    # sair, o processo cai.
    $null = $process.CloseMainWindow()
    if (-not $process.WaitForExit(15000)) {
        Write-Host "o exe nao fechou com WM_CLOSE em 15 s; a terminar."
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $null = $process.WaitForExit(15000)
    }
    Start-Sleep -Seconds 2
    $after = Get-Snapshot $data

    $changed = New-Object System.Collections.Generic.List[string]
    foreach ($relative in $after.Keys) {
        if (-not $before.ContainsKey($relative) -or $before[$relative] -ne $after[$relative]) { $changed.Add($relative) }
    }
    foreach ($relative in $before.Keys) {
        if (-not $after.ContainsKey($relative)) { $changed.Add($relative) }
    }
    $changed = @($changed | Sort-Object)

    $byRow = [ordered]@{}
    $unknown = New-Object System.Collections.Generic.List[string]
    foreach ($relative in $changed) {
        $row = Find-AllowlistRow $relative
        if ($null -eq $row) {
            $unknown.Add($relative)
            continue
        }
        if (-not $byRow.Contains($row.Path)) { $byRow[$row.Path] = New-Object System.Collections.Generic.List[string] }
        $byRow[$row.Path].Add($relative)
    }
    Write-Host "A sessao criou ou mudou $($changed.Count) ficheiro(s) em <data_dir>:"
    foreach ($key in $byRow.Keys) {
        $row = $allowlist | Where-Object { $_.Path -eq $key } | Select-Object -First 1
        $files = $byRow[$key]
        Write-Host ("  {0,-24} {1,-9} {2} ficheiro(s)" -f $key, $row.Kind, $files.Count)
        foreach ($file in ($files | Select-Object -First 8)) { Write-Host "      $file" }
        if ($files.Count -gt 8) { Write-Host "      ... e mais $($files.Count - 8)" }
        $summary.Add("$key ($($row.Kind)): $($files.Count) ficheiro(s)")
    }
    foreach ($relative in $unknown) {
        $failures.Add("$relative`: fora da lista (SPEC-0006, tabela da fase 0): uma loja nova sem tipo declarado")
    }
    if (-not ($changed | Where-Object { $_ -ieq 'history.jsonl' })) {
        $failures.Add("history.jsonl: a sessao nao chegou ao historico (o E2E nao exercitou o PrivacyGuard::record)")
    }
    if (-not ($changed | Where-Object { $_.StartsWith('memory/', [StringComparison]::OrdinalIgnoreCase) })) {
        $failures.Add("memory/: a sessao nao chegou a memoria (o E2E nao exercitou o PrivacyGuard::capture)")
    }
    if ($failures.Count -gt 0 -and (Test-Path -LiteralPath $log)) {
        Write-Host "log de depuracao (fim):"
        Get-Content -LiteralPath $log -Tail 40 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "  $_" }
    }
} finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $null = $process.WaitForExit(15000)
    }
    if ($fixture -and -not $fixture.HasExited) {
        Stop-Process -Id $fixture.Id -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}

if ($env:GITHUB_STEP_SUMMARY) {
    $lines = @("### Modo privado, fase 0 (sessao Normal)") + @($summary | ForEach-Object { "- $_" }) + @($failures | ForEach-Object { "- FALHA: $_" })
    [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, ($lines -join "`n") + "`n", $utf8)
}
foreach ($line in $summary) { Write-Host $line }
if ($failures.Count -gt 0) {
    foreach ($line in $failures) { Write-Host "FALHA: $line" }
    throw "Modo privado, fase 0: $($failures.Count) problema(s)."
}
Write-Host "Modo privado, fase 0: tudo o que a sessao Normal escreveu tem uma linha na lista."
