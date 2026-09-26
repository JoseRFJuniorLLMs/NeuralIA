param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [int]$TimeoutSec = 40
)

# E2E do bloqueio de anuncios (adblock, plano 2.3). SO CI: abre o exe
# testado (ci-tested/NeuralIA.exe) numa pasta de dados temporaria e nunca
# sai da maquina.
#
# - A pasta de dados comeca com o bloqueio LIGADO e uma lista guardada
#   AGORA (adblock-settings.json, adblock-list.json): a renovacao semanal
#   nao fica devida, por isso o exe nunca fala com pgl.yoyo.org. No fim
#   confere-se que a lista guardada ficou byte a byte igual (uma renovacao
#   te-la-ia reescrito).
# - O WebView2 corre com --host-resolver-rules="MAP * 127.0.0.1": qualquer
#   nome resolve para o loopback (o `*.test` da fixture chega a ela; um
#   nome da internet nunca sai daqui). Os argumentos que o wry poe por
#   omissao seguem com ele (a variavel substitui-os).
# - NEURALIA_STARTUP_INPUT abre a pagina da fixture na Web completa.
#
# Prova, pelo que chegou a fixture (scripts/adblock-fixture.mjs):
# - o script de ads.blocked.test (dominio da lista) NUNCA chegou e a pagina
#   viu-o falhar (o 403 do CreateWebResourceResponse);
# - a moldura de ads.blocked.test chegou: um DOCUMENTO nunca e bloqueado;
# - a imagem de allowed.test (fora da lista) chegou: os nomes resolvem para
#   a fixture, por isso o que falta foi o bloqueio que tirou;
# - a imagem do proprio 127.0.0.1 chegou.
$ErrorActionPreference = "Stop"
$exe = (Resolve-Path -LiteralPath $ExePath).Path
$utf8 = [Text.UTF8Encoding]::new($false)

$work = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-adblock-" + [guid]::NewGuid().ToString("N"))
$dataDir = Join-Path $work "data"
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
$portFile = Join-Path $work "fixture.port"

$settingsPath = Join-Path $dataDir "adblock-settings.json"
[IO.File]::WriteAllText($settingsPath, '{"version":1,"data":{"enabled":true,"allow_sites":[],"distraction":{}}}', $utf8)
$domains = @("blocked.test") + @(1..1200 | ForEach-Object { "filler$_.example-ads.test" })
$stored = [ordered]@{
    version = 1
    data    = [ordered]@{
        fetched_ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        domains    = ($domains -join "`n")
    }
}
$listPath = Join-Path $dataDir "adblock-list.json"
[IO.File]::WriteAllText($listPath, ($stored | ConvertTo-Json -Compress -Depth 4), $utf8)
$listHash = (Get-FileHash -LiteralPath $listPath -Algorithm SHA256).Hash

$fixture = $null
$process = $null
$failure = $null
$log = @()
try {
    $fixture = Start-Process -FilePath "node" -ArgumentList @("`"$(Join-Path $PSScriptRoot 'adblock-fixture.mjs')`"", "`"$portFile`"") -PassThru -NoNewWindow
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $portFile)) {
        if ($watch.ElapsedMilliseconds -gt 10000 -or $fixture.HasExited) { throw "A fixture de 127.0.0.1 nao arrancou." }
        Start-Sleep -Milliseconds 50
    }
    $port = ([IO.File]::ReadAllText($portFile)).Trim()
    $origin = "http://127.0.0.1:$port"
    Write-Host "fixture: $origin/page.html"

    $env:NEURALIA_DATA_DIR = $dataDir
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_STARTUP_INPUT = "web:$origin/page.html"
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --host-resolver-rules="MAP * 127.0.0.1"'
    $process = Start-Process -FilePath $exe -PassThru

    function Get-Seen {
        $raw = Invoke-WebRequest -Uri "$origin/__log" -UseBasicParsing -TimeoutSec 5
        return @($raw.Content | ConvertFrom-Json)
    }
    function Test-Seen($rows, [string]$hostPrefix, [string]$path, $query = $null) {
        return @($rows | Where-Object {
                $_.host -like "$hostPrefix*" -and $_.path -eq $path -and ($null -eq $query -or $_.query -eq $query)
            }).Count -gt 0
    }

    $watch.Restart()
    $done = $false
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if ($process.HasExited) { throw "O NeuralIA saiu antes de a pagina carregar (codigo $($process.ExitCode))." }
        $log = Get-Seen
        $reported = (Test-Seen $log "127.0.0.1" "/report" "what=ad-error") -or (Test-Seen $log "127.0.0.1" "/report" "what=ad-load")
        if ($reported -and (Test-Seen $log "allowed.test" "/ok.png") -and (Test-Seen $log "ads.blocked.test" "/frame.html")) {
            $done = $true
            break
        }
        Start-Sleep -Milliseconds 250
    }
    # Um pedido atrasado do anuncio ainda contaria: mais um instante.
    Start-Sleep -Milliseconds 1500
    $log = Get-Seen

    $checks = [ordered]@{
        page_loaded             = Test-Seen $log "127.0.0.1" "/page.html"
        first_party_loaded      = Test-Seen $log "127.0.0.1" "/first.png"
        unlisted_loaded         = Test-Seen $log "allowed.test" "/ok.png"
        listed_document_loaded  = Test-Seen $log "ads.blocked.test" "/frame.html"
        page_saw_ad_fail        = Test-Seen $log "127.0.0.1" "/report" "what=ad-error"
        ad_script_never_arrived = -not (@($log | Where-Object { $_.path -eq "/ad.js" }).Count -gt 0)
        page_never_ran_the_ad   = -not (Test-Seen $log "127.0.0.1" "/report" "what=ad-load")
        list_not_refreshed      = (Get-FileHash -LiteralPath $listPath -Algorithm SHA256).Hash -eq $listHash
    }
    [ordered]@{ settled = $done; checks = $checks; requests = @($log | Where-Object { $_.path -ne "/__log" }) } |
        ConvertTo-Json -Depth 5 | Write-Host
    if (-not $checks.page_loaded) { throw "A pagina da fixture nunca foi pedida: o NEURALIA_STARTUP_INPUT nao abriu a Web completa." }
    if (-not $checks.unlisted_loaded) { throw "A imagem de allowed.test nunca chegou: o --host-resolver-rules nao foi aplicado, e sem ele o E2E nao prova nada." }
    $bad = @($checks.GetEnumerator() | Where-Object { -not $_.Value } | ForEach-Object { $_.Key })
    if ($bad.Count -gt 0) { throw "Bloqueio de anuncios: falhou $($bad -join ', ')." }
    Write-Host "Adblock E2E: o script da lista foi bloqueado (403), a moldura do mesmo dominio, o que nao esta na lista e o proprio site passaram, e a lista nao foi renovada."
}
catch {
    $failure = $_.Exception.Message
}
finally {
    foreach ($name in "NEURALIA_DATA_DIR", "NEURALIA_NO_GMAIL", "NEURALIA_STARTUP_INPUT", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS") {
        Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    }
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    if ($fixture -and -not $fixture.HasExited) {
        Stop-Process -Id $fixture.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failure) {
    throw $failure
}
