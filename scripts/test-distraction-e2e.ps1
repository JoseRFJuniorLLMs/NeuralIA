param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    [switch]$ExpectVisible,
    [int]$TimeoutSec = 180
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
$fixtureStdout = $null
$fixtureStderr = $null
$process = $null
$failure = $null
$http = $null
$rows = @()
$environmentNames = @("NEURALIA_DATA_DIR", "NEURALIA_NO_GMAIL", "NEURALIA_REDUCE_MOTION", "NEURALIA_STARTUP_INPUT", "WEBVIEW2_USER_DATA_FOLDER")
$previousEnvironment = @{}
foreach ($name in $environmentNames) { $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process") }
try {
    $fixtureScript = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "distraction-e2e-fixture.mjs")).Path
    # The runner has no interactive console: launch Node directly rather
    # than asking ShellExecute/console-window handling to hide a new window.
    $fixtureStart = [Diagnostics.ProcessStartInfo]::new()
    $fixtureStart.FileName = (Get-Command node -CommandType Application | Select-Object -First 1).Source
    $fixtureStart.UseShellExecute = $false
    $fixtureStart.CreateNoWindow = $true
    $fixtureStart.RedirectStandardOutput = $true
    $fixtureStart.RedirectStandardError = $true
    $fixtureStart.ArgumentList.Add($fixtureScript)
    $fixtureStart.ArgumentList.Add($portFile)
    $fixture = [Diagnostics.Process]::Start($fixtureStart)
    $fixtureStdout = $fixture.StandardOutput.ReadToEndAsync()
    $fixtureStderr = $fixture.StandardError.ReadToEndAsync()
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $portFile)) {
        if ($watch.ElapsedMilliseconds -gt 45000 -or $fixture.HasExited) { throw "A fixture da SPEC-0114 nao arrancou (pid $($fixture.Id); exited=$($fixture.HasExited); node=$($fixtureStart.FileName))." }
        Start-Sleep -Milliseconds 50
    }
    $port = ([IO.File]::ReadAllText($portFile, $utf8)).Trim()
    $localOrigin = "http://127.0.0.1:$port"
    $pageUrl = "http://${hostName}:$port/page.html"

    # Reuse one direct loopback client; readiness is checked before WebView2
    # starts, so a dead fixture cannot be mistaken for failed COM injection.
    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $false
    $http = [Net.Http.HttpClient]::new($handler)
    $http.Timeout = [TimeSpan]::FromSeconds(5)
    function Get-Seen {
        $raw = $http.GetStringAsync("$localOrigin/__log").GetAwaiter().GetResult()
        return @($raw | ConvertFrom-Json)
    }
    $rows = Get-Seen
    Write-Host "SPEC-0114 fixture ready at $localOrigin; cold-start deadline ${TimeoutSec}s."

    $env:NEURALIA_DATA_DIR = $dataDir
    $env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $dataDir "WebView2"
    $env:NEURALIA_NO_GMAIL = "1"
    $env:NEURALIA_REDUCE_MOTION = "1"
    $env:NEURALIA_STARTUP_INPUT = "web:$pageUrl"
    $process = Start-Process -FilePath $exe -PassThru

    $result = $null
    $sawSecond = $false
    $watch.Restart()
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        if ($fixture.HasExited) { throw "A fixture da SPEC-0114 saiu (codigo $($fixture.ExitCode))." }
        if ($process.HasExited) { throw "NeuralIA saiu antes do E2E da SPEC-0114 (codigo $($process.ExitCode))." }
        # A fixture pode atrasar o polling em runners Windows ocupados.
        # Um timeout isolado nao invalida o E2E: o prazo global continua fixo.
        try {
            $rows = Get-Seen
        } catch {
            $message = $_.Exception.Message
            if ($message -notmatch '(?i)timeout|timed out|canceled due to') { throw }
            Write-Warning "SPEC-0114: timeout transitorio ao consultar a fixture local: $message"
            Start-Sleep -Milliseconds 150
            continue
        }
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
catch {
    $failure = $_.Exception.Message
    Write-Warning "SPEC-0114 failed: $failure"
    Write-Host "Last fixture requests: $(@($rows | Where-Object { $_.path -ne '/__log' } | Select-Object -Last 20) | ConvertTo-Json -Compress)"
    foreach ($output in @($fixtureStdout, $fixtureStderr)) {
        if ($output -and $output.IsCompleted) { Write-Host $output.GetAwaiter().GetResult() }
    }
}
finally {
    if ($http) { $http.Dispose() }
    foreach ($name in $environmentNames) { [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], "Process") }
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
    if ($fixture -and -not $fixture.HasExited) { Stop-Process -Id $fixture.Id -Force -ErrorAction SilentlyContinue }
    if ($fixture) {
        $exited = $fixture.WaitForExit(5000)
        if ($failure -and $exited) {
            foreach ($output in @($fixtureStdout, $fixtureStderr)) {
                if ($output) { Write-Host $output.GetAwaiter().GetResult() }
            }
        }
        $fixture.Dispose()
    }
    Start-Sleep -Milliseconds 300
    $resolvedWork = [IO.Path]::GetFullPath($work)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolvedWork.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) { throw "E2E cleanup escaped temp root: $resolvedWork" }
    Remove-Item -LiteralPath $resolvedWork -Recurse -Force -ErrorAction SilentlyContinue
}
if ($failure) { throw $failure }
