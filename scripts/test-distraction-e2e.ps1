param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [switch]$ExpectVisible,
    [int]$TimeoutSec = 45
)

# SPEC-0114 E2E of the real WebView2 COM injection boundary.
# news.distraction.test MUST be pinned to 127.0.0.1 by the CI caller.
# The fixture reloads once before measuring to avoid the known first-document race.
$ErrorActionPreference = "Stop"
$exe = (Resolve-Path -LiteralPath $ExePath).Path
$hostName = "news.distraction.test"
$resolved = @([Net.Dns]::GetHostAddresses($hostName) | ForEach-Object { $_.ToString() })
if ($resolved.Count -eq 0 -or ($resolved | Where-Object { $_ -ne "127.0.0.1" }).Count -ne 0) {
    throw "$hostName nao esta preso exclusivamente a 127.0.0.1: $($resolved -join ', ')."
}

$utf8 = [Text.UTF8Encoding]::new($false)
$work = Join-Path ([IO.Path]::GetTempPath()) ("neuralia-distraction-e2e-" + [guid]::NewGuid().ToString("N"))
$dataDir = Join-Path $work "data"
$portFile = Join-Path $work "fixture.port"
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
$settingsPath = Join-Path $dataDir "adblock-settings.json"
[IO.File]::WriteAllText($settingsPath, '{"version":1,"data":{"enabled":false,"allow_sites":[],"distraction":{}}}', $utf8)

$fixture = $null
$process = $null
$failure = $null
try {
    $fixtureScript = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "distraction-e2e-fixture.mjs")).Path
    $fixture = Start-Process -FilePath "node" -ArgumentList @(('"' + $fixtureScript + '"'), ('"' + $portFile + '"')) -PassThru -NoNewWindow
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $portFile)) {
        if ($watch.ElapsedMilliseconds -gt 10000 -or $fixture.HasExited) { throw "A fixture da SPEC-0114 nao arrancou." }
        Start-Sleep -Milliseconds 50
    }
    $port = ([IO.File]::ReadAllText($portFile, $utf8)).Trim()
    $localOrigin = "http://127.0.0.1:$port"
    $pageUrl = "http://${hostName}:$port/page.html"

    $env:NEURALIA_DATA_DIR = $dataDir
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_REDUCE_MOTION = "1"
    $env:NEURALIA_STARTUP_INPUT = "web:$pageUrl"
    $process = Start-Process -FilePath $exe -PassThru

    function Get-Seen {
        $raw = Invoke-WebRequest -Uri "$localOrigin/__log" -UseBasicParsing -TimeoutSec 5
        return @($raw.Content | ConvertFrom-Json)
    }

    $result = $null
    $sawSecond = $false
    $watch.Restart()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if ($process.HasExited) { throw "NeuralIA saiu antes do E2E da SPEC-0114 (codigo $($process.ExitCode))." }
        $rows = Get-Seen
        $sawSecond = @($rows | Where-Object { $_.host -like "$hostName*" -and $_.path -eq "/page.html" -and $_.query -eq "pass=2" }).Count -gt 0
        if (@($rows | Where-Object { $_.path -eq "/report" -and $_.query -eq "what=hidden" }).Count -gt 0) { $result = "hidden"; break }
        if (@($rows | Where-Object { $_.path -eq "/report" -and $_.query -eq "what=visible" }).Count -gt 0) { $result = "visible"; break }
        Start-Sleep -Milliseconds 150
    }

    if (-not $sawSecond) { throw "A segunda navegacao da fixture nao ocorreu; o teste nao chegou ao documento medido." }
    if (-not $result) { throw "Timeout esperando o veredicto DOM da anti-distracao." }
    if ($ExpectVisible) {
        if ($result -ne "visible") { throw "Sabotagem inconclusiva: o banner ficou oculto mesmo sem a ligacao COM esperada." }
        Write-Host "SPEC-0114 sabotage proof: sem a ligacao COM, o banner permaneceu visivel."
    } else {
        if ($result -ne "hidden") { throw "SPEC-0114: o banner ficou visivel; AddScriptToExecuteOnDocumentCreated nao produziu o efeito embarcado." }
        Write-Host "SPEC-0114 WebView2 E2E: a segunda navegacao recebeu o script COM e ocultou o cookie banner real."
    }
}
catch { $failure = $_.Exception.Message }
finally {
    foreach ($name in "NEURALIA_DATA_DIR", "NEURALIA_NO_GMAIL", "NEURALIA_REDUCE_MOTION", "NEURALIA_STARTUP_INPUT") { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
    if ($fixture -and -not $fixture.HasExited) { Stop-Process -Id $fixture.Id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 300
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failure) { throw $failure }
