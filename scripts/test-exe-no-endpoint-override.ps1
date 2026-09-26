param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath
)

# Contrato de release do transporte de IA (infra-llm-transport, plano 2.3):
# o `neural_core::llm` so fala com os hosts fixados (`Endpoint::pinned`); o
# loopback so compila nos testes e nenhuma variavel de ambiente troca o host,
# a origem ou um fixture. Este gate procura, nos bytes do exe publicado
# (ci-tested/NeuralIA.exe), nomes de variaveis que fariam isso: um
# `NEURALIA_*BASE_URL*`, `*ENDPOINT*`, `*FIXTURE*`, `*LOOPBACK*`..., e os
# nomes que os SDKs de IA usam para desviar o host (`OPENAI_BASE_URL`,
# `ANTHROPIC_BASE_URL`, `GEMINI_BASE_URL`, `OLLAMA_HOST`...). Procura em
# ASCII e em UTF-16LE (as duas paridades).
#
# Antes de olhar para o exe, prova que ve o que procura (cada canario, nas
# tres codificacoes, e achado) e que nao acusa as variaveis legitimas que o
# exe ja le: sem isto um "nada achado" nao provava nada.
$ErrorActionPreference = "Stop"

$pattern = [regex]::new(
    '(NEURALIA_[A-Z0-9_]*(BASE_URL|BASE_URI|API_BASE|API_URL|API_HOST|ENDPOINT|FIXTURE|LOOPBACK|MOCK)[A-Z0-9_]*)' +
    '|((GEMINI|GOOGLE_GEMINI|GOOGLE_AI|GOOGLE_GENAI|OPENAI|ANTHROPIC|OLLAMA|LLM)_(BASE_URL|BASE_URI|API_BASE|API_BASE_URL|API_URL|API_HOST|ENDPOINT|HOST))',
    [Text.RegularExpressions.RegexOptions]::IgnoreCase
)

function Find-Overrides([byte[]]$bytes) {
    $latin1 = [Text.Encoding]::GetEncoding(28591)
    $texts = @($latin1.GetString($bytes), [Text.Encoding]::Unicode.GetString($bytes))
    if ($bytes.Length -gt 1) {
        $texts += [Text.Encoding]::Unicode.GetString($bytes, 1, $bytes.Length - 1)
    }
    $hits = foreach ($text in $texts) {
        foreach ($match in $pattern.Matches($text)) { $match.Value }
    }
    return @($hits | Sort-Object -Unique)
}

function New-Sample([string]$name, [string]$encoding, [int]$pad) {
    $filler = [byte[]]::new(64 + $pad)
    for ($i = 0; $i -lt $filler.Length; $i++) { $filler[$i] = [byte](($i * 7 + 3) % 251) }
    $payload = switch ($encoding) {
        "ascii" { [Text.Encoding]::ASCII.GetBytes($name) }
        "utf16" { [Text.Encoding]::Unicode.GetBytes($name) }
    }
    return [byte[]]($filler + [byte[]](0) + $payload + [byte[]](0, 0) + $filler)
}

$canaries = @(
    "NEURALIA_GEMINI_BASE_URL",
    "NEURALIA_LLM_ENDPOINT",
    "NEURALIA_LLM_FIXTURE_DIR",
    "NEURALIA_AI_LOOPBACK",
    "GEMINI_BASE_URL",
    "OPENAI_BASE_URL",
    "ANTHROPIC_BASE_URL",
    "OLLAMA_HOST"
)
foreach ($canary in $canaries) {
    foreach ($encoding in @("ascii", "utf16")) {
        foreach ($pad in @(0, 1)) {
            $found = Find-Overrides (New-Sample $canary $encoding $pad)
            if ($found -notcontains $canary) {
                throw "O gate nao ve '$canary' ($encoding, desvio $pad): achou '$($found -join ', ')'."
            }
        }
    }
}
$legitimate = @(
    "NEURALIA_DATA_DIR",
    "NEURALIA_STARTUP_INPUT",
    "NEURALIA_NO_STARTUP",
    "NEURALIA_LIFECYCLE_PROBE",
    "NEURALIA_DEBUG_LOG",
    "NEURALIA_NO_GMAIL",
    "NEURALIA_REDUCE_MOTION",
    "WEBVIEW2_USER_DATA_FOLDER",
    "HTTPS_PROXY",
    "LOCALAPPDATA"
)
foreach ($name in $legitimate) {
    foreach ($encoding in @("ascii", "utf16")) {
        $found = Find-Overrides (New-Sample $name $encoding 0)
        if ($found.Count -gt 0) {
            throw "O gate acusa a variavel legitima '$name' ($encoding): '$($found -join ', ')'."
        }
    }
}
Write-Host "endpoint-override gate: sees $($canaries.Count) canaries in ASCII and UTF-16LE, ignores $($legitimate.Count) legitimate names."

$resolved = (Resolve-Path -LiteralPath $ExePath).Path
$bytes = [IO.File]::ReadAllBytes($resolved)
if ($bytes.Length -lt 1024) {
    throw "$resolved tem $($bytes.Length) bytes: nao e o exe."
}
$found = Find-Overrides $bytes
Write-Host "endpoint overrides in ${resolved}: $($found.Count) ($($bytes.Length) bytes)"
if ($found.Count -gt 0) {
    throw "O exe $resolved tem nomes de variaveis que desviam o transporte de IA: $($found -join ', ')."
}
