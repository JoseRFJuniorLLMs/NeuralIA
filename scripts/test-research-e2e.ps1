param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [switch]$ExpectPersistenceFailure,
    [switch]$PrivateMode,
    [int]$TimeoutSec = 45
)

# SPEC-0101 E2E. CI-only: usa o mesmo canal do accel-spike, que existe so
# no exe compilado com --features accel-spike. A pagina e servida em
# 127.0.0.1, mas usa o host oficial www.google.com; o workflow prende o DNS
# ao loopback. Assim o leitor real ainda passa por ProviderId::from_url e
# column_answer_read, em vez de uma excecao de teste no validador.
$ErrorActionPreference = "Stop"
if ($ExpectPersistenceFailure -and $PrivateMode) {
    throw "-ExpectPersistenceFailure e -PrivateMode sao modos de prova separados."
}
$exe = (Resolve-Path -LiteralPath $ExePath).Path
$utf8 = [Text.UTF8Encoding]::new($false)
$answer = "NEURALIA_SPEC0101_ANSWER_7F9C"
$source = "https://example.test/spec0101-source"

foreach ($name in 'www.google.com', 'google.com', 'chatgpt.com', 'chat.openai.com', 'claude.ai') {
    $resolved = @([Net.Dns]::GetHostAddresses($name) | ForEach-Object { $_.ToString() })
    if ($resolved.Count -eq 0 -or ($resolved | Where-Object { $_ -ne '127.0.0.1' }).Count -ne 0) {
        throw "$name nao esta preso exclusivamente a 127.0.0.1: $($resolved -join ', ')."
    }
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-research-e2e-" + [Guid]::NewGuid().ToString("N"))
$dataDir = Join-Path $work "data"
$spikeDir = Join-Path $work "spike"
$portFile = Join-Path $work "fixture.port"
New-Item -ItemType Directory -Force -Path $dataDir, $spikeDir | Out-Null
$logPath = Join-Path $spikeDir "spike.log"
$cmdPath = Join-Path $spikeDir "cmd.txt"

$fixture = $null
$process = $null
$failure = $null
$seq = 0

function Read-Records {
    if (-not (Test-Path -LiteralPath $logPath)) { return @() }
    $records = @()
    $stream = $null
    $reader = $null
    try {
        $stream = [IO.FileStream]::new(
            $logPath,
            [IO.FileMode]::Open,
            [IO.FileAccess]::Read,
            [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete
        )
        $reader = [IO.StreamReader]::new($stream, $utf8, $true)
        while (-not $reader.EndOfStream) {
            $line = $reader.ReadLine()
            if ([string]::IsNullOrWhiteSpace($line)) { continue }
            try { $records += ($line | ConvertFrom-Json) } catch {}
        }
    }
    catch [IO.IOException] {
        return @()
    }
    finally {
        if ($reader) { $reader.Dispose() }
        elseif ($stream) { $stream.Dispose() }
    }
    return @($records)
}

function Wait-Record([scriptblock]$Predicate, [string]$What, [int]$Milliseconds = 30000) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $Milliseconds) {
        if ($process -and $process.HasExited) {
            throw "NeuralIA saiu enquanto esperava $What (codigo $($process.ExitCode))."
        }
        foreach ($record in (Read-Records)) {
            if (& $Predicate $record) { return $record }
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Timeout esperando $What."
}

function Send-Command([string]$Body, [int]$Milliseconds = 30000) {
    $script:seq++
    $line = "$($script:seq) $Body"
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (Test-Path -LiteralPath $cmdPath) {
        if ($watch.ElapsedMilliseconds -gt 5000) { throw "cmd.txt nao foi consumido." }
        Start-Sleep -Milliseconds 25
    }
    $tmp = Join-Path $spikeDir ("cmd-" + [Guid]::NewGuid().ToString("N") + ".tmp")
    [IO.File]::WriteAllText($tmp, $line + [Environment]::NewLine, $utf8)
    Move-Item -LiteralPath $tmp -Destination $cmdPath
    $wanted = $script:seq
    $ack = Wait-Record { param($r) [string]$r.t -eq "ack" -and [int64]$r.seq -eq $wanted } "ack $wanted ($Body)" $Milliseconds
    if (-not [bool]$ack.ok) { throw "Comando '$Body' recusado: $($ack.detail)" }
    return $ack
}

function Find-PersistedAnswer {
    $root = Join-Path $dataDir "memory"
    if (-not (Test-Path -LiteralPath $root)) { return $null }
    foreach ($file in Get-ChildItem -LiteralPath $root -Recurse -Filter *.json -File -ErrorAction SilentlyContinue) {
        $text = [IO.File]::ReadAllText($file.FullName, $utf8)
        if ($text.Contains($answer) -and $text.Contains("Google IA") -and $text.Contains($source)) {
            return $file
        }
    }
    return $null
}

try {
    $fixtureScript = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot 'research-e2e-fixture.mjs')).Path
    $fixture = Start-Process -FilePath "node" -ArgumentList @("`"$fixtureScript`"", "`"$portFile`"") -PassThru -NoNewWindow
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $portFile)) {
        if ($watch.ElapsedMilliseconds -gt 10000 -or $fixture.HasExited) { throw "A fixture de pesquisa nao arrancou." }
        Start-Sleep -Milliseconds 50
    }
    $port = ([IO.File]::ReadAllText($portFile, $utf8)).Trim()
    $fixtureUrl = "http://www.google.com:$port/search?udm=50&q=spec0101"
    Write-Host "research fixture: $fixtureUrl"

    $env:NEURALIA_DATA_DIR = $dataDir
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_REDUCE_MOTION = "1"
    $env:NEURALIA_ACCEL_SPIKE_DIR = $spikeDir
    $env:NEURALIA_STARTUP_INPUT = "SPEC 0101 E2E startup"
    $process = Start-Process -FilePath $exe -PassThru

    Wait-Record { param($r) [string]$r.t -eq "hello" } "hello do harness" 120000 | Out-Null
    $ping = Send-Command "ping Column" 120000
    if ([string]$ping.detail -notmatch 'aberto=true') {
        throw "A primeira coluna nao abriu: $($ping.detail)"
    }

    if ($PrivateMode) {
        $privateAck = Send-Command "research-private Column"
        if ([string]$privateAck.detail -notmatch 'privacy=private') {
            throw "O harness nao confirmou o modo privado: $($privateAck.detail)"
        }
    }

    Send-Command "research-open Column $fixtureUrl" | Out-Null
    Wait-Record {
        param($r)
        [string]$r.t -eq "colnav" -and [string]$r.phase -eq "done" -and [bool]$r.ok
    } "NavigationCompleted da fixture Google" 30000 | Out-Null

    Send-Command "research-start Column" | Out-Null

    if ($PrivateMode) {
        $exportsRoot = Join-Path $dataDir "research-exports"
        $automaticExports = @(Get-ChildItem -LiteralPath $exportsRoot -Filter *.md -File -ErrorAction SilentlyContinue)
        if ($automaticExports.Count -ne 0) {
            throw "Modo privado criou export automatico antes do pedido explicito: $($automaticExports[0].FullName)"
        }

        # O export explicito e permitido em qualquer modo (SPEC-0006). Alem de
        # validar essa regra, ele prova que a resposta chegou a sessao viva; so
        # depois disso a ausencia de JSON persistido e uma prova real do Private.
        $markdown = $null
        $markdownPath = $null
        $watch.Restart()
        while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
            $exportAck = Send-Command "research-export Column"
            if ([string]$exportAck.detail -notmatch 'research-export id=([A-Za-z0-9_-]+)') {
                throw "ACK do export privado nao trouxe id: $($exportAck.detail)"
            }
            $sessionId = $Matches[1]
            $markdownPath = Join-Path $exportsRoot "$sessionId.md"
            if (Test-Path -LiteralPath $markdownPath) {
                $candidate = [IO.File]::ReadAllText($markdownPath, $utf8)
                if ($candidate.Contains($answer) -and $candidate.Contains("Google IA") -and $candidate.Contains($source)) {
                    $markdown = $candidate
                    break
                }
            }
            if ($process.HasExited) { throw "NeuralIA saiu antes de completar a sessao privada." }
            Start-Sleep -Milliseconds 300
        }
        if (-not $markdown) {
            throw "A sessao privada nao recebeu resposta/provedor/proveniencia pelo caminho real."
        }

        # save_session envia ao worker de memoria de forma assincrona. Espera
        # mais um pouco depois de a resposta estar comprovadamente na sessao
        # viva para apanhar qualquer escrita indevida tardia.
        $watch.Restart()
        while ($watch.Elapsed.TotalSeconds -lt 4) {
            $persisted = Find-PersistedAnswer
            if ($persisted) {
                throw "Modo privado persistiu automaticamente a sessao em $($persisted.FullName)."
            }
            Start-Sleep -Milliseconds 100
        }
        Write-Host "SPEC-0101 Private E2E: resposta ficou viva em memoria, sem persistencia/export automaticos; export explicito preservou provedor e proveniencia."
        return
    }

    $persisted = $null
    $watch.Restart()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        $persisted = Find-PersistedAnswer
        if ($persisted) { break }
        if ($process.HasExited) { throw "NeuralIA saiu antes de persistir a sessao." }
        Start-Sleep -Milliseconds 200
    }

    if ($ExpectPersistenceFailure) {
        if ($persisted) {
            throw "Sabotagem ficou verde: a resposta ainda apareceu em $($persisted.FullName)."
        }
        # No JSON alone could mean that the reader never received an answer.
        # Prove the live session contains the answer and provenance through the
        # real explicit export before accepting the missing-write sabotage.
        $liveAnswer = $false
        $watch.Restart()
        while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
            $exportAck = Send-Command "research-export Column"
            if ([string]$exportAck.detail -notmatch 'research-export id=([A-Za-z0-9_-]+)') {
                throw "ACK do export sabotado nao trouxe id: $($exportAck.detail)"
            }
            $markdownPath = Join-Path (Join-Path $dataDir "research-exports") "$($Matches[1]).md"
            if (Test-Path -LiteralPath $markdownPath) {
                $markdown = [IO.File]::ReadAllText($markdownPath, $utf8)
                if ($markdown.Contains($answer) -and $markdown.Contains("Google IA") -and $markdown.Contains($source)) {
                    $liveAnswer = $true
                    break
                }
            }
            Start-Sleep -Milliseconds 200
        }
        if (-not $liveAnswer) { throw "Sabotagem inconclusiva: a resposta nao chegou a sessao viva." }
        # A live export can precede the asynchronous memory-worker write.
        # Keep the app alive while observing the same post-answer window used
        # by the Private gate, so a pending write cannot become a false green.
        $watch.Restart()
        while ($watch.Elapsed.TotalSeconds -lt 4) {
            if ($process.HasExited) { throw "NeuralIA saiu durante a prova de ausencia de persistencia." }
            $persisted = Find-PersistedAnswer
            if ($persisted) { throw "Sabotagem ficou verde: a resposta ainda apareceu em $($persisted.FullName)." }
            Start-Sleep -Milliseconds 100
        }
        Write-Host "Sabotagem provada: resposta e proveniencia chegaram a sessao viva, mas sem save_session nao chegaram ao disco."
        return
    }

    if (-not $persisted) {
        throw "A resposta lida da WebView nao apareceu na sessao persistida."
    }
    Write-Host "session persisted: $($persisted.FullName)"

    $exportAck = Send-Command "research-export Column"
    if ([string]$exportAck.detail -notmatch 'research-export id=([A-Za-z0-9_-]+)') {
        throw "ACK do export nao trouxe id: $($exportAck.detail)"
    }
    $sessionId = $Matches[1]
    $markdownPath = Join-Path (Join-Path $dataDir "research-exports") "$sessionId.md"
    $watch.Restart()
    while (-not (Test-Path -LiteralPath $markdownPath)) {
        if ($watch.Elapsed.TotalSeconds -gt 10) { throw "O exportador real nao criou $markdownPath." }
        Start-Sleep -Milliseconds 100
    }
    $markdown = [IO.File]::ReadAllText($markdownPath, $utf8)
    foreach ($needle in @($answer, "Google IA", $source)) {
        if (-not $markdown.Contains($needle)) {
            throw "Markdown exportado nao contem '$needle'."
        }
    }
    Write-Host "SPEC-0101 E2E: WebView -> page_eval/Consenso -> sessao persistida -> export Markdown, com provedor e proveniencia."
}
catch {
    $failure = $_.Exception.Message
}
finally {
    foreach ($name in "NEURALIA_DATA_DIR", "NEURALIA_NO_GMAIL", "NEURALIA_REDUCE_MOTION", "NEURALIA_ACCEL_SPIKE_DIR", "NEURALIA_STARTUP_INPUT") {
        Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
    }
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
    if ($fixture -and -not $fixture.HasExited) { Stop-Process -Id $fixture.Id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 300
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failure) { throw $failure }
