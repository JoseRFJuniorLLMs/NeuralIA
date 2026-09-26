param(
    [Parameter(Mandatory = $true)]
    [string]$ExePath,
    # Absent: o exe publicado (job `windows`, sobre o ci-tested/NeuralIA.exe).
    # Present: o exe do job `accel-spike` -- prova que a procura ve o marcador
    # quando ele existe, senao o "Absent" nao provava nada.
    [Parameter(Mandatory = $true)]
    [ValidateSet("Absent", "Present")]
    [string]$Expect
)

# Contrato de release do spike de aceleradores (infra-accel-spike, plano 2.3):
# o codigo do spike so entra com `--features accel-spike`, e o exe que o CI
# testa e publica e compilado sem ela. O modulo do spike escreve este marcador
# no registo dele, por isso o texto fica nos bytes de qualquer exe que o tenha
# (crates/neural-app/src/accel_spike.rs, SPIKE_BUILD_MARKER).
$ErrorActionPreference = "Stop"
$marker = "NEURALIA-ACCEL-SPIKE-BUILD-v1"

$resolved = (Resolve-Path -LiteralPath $ExePath).Path
$bytes = [IO.File]::ReadAllBytes($resolved)
if ($bytes.Length -lt 1024) {
    throw "$resolved tem $($bytes.Length) bytes: nao e o exe."
}
# Latin-1 mapeia cada byte num caracter: o marcador ASCII aparece tal e qual.
$text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
$found = $text.Contains($marker)
Write-Host "accel-spike marker in ${resolved}: $found (expected $Expect; $($bytes.Length) bytes)"

if ($Expect -eq "Absent" -and $found) {
    throw "O exe $resolved tem o spike de aceleradores (feature accel-spike): o exe publicado tem de ser compilado sem ela."
}
if ($Expect -eq "Present" -and -not $found) {
    throw "O exe $resolved nao tem o marcador do spike: ou nao foi compilado com --features accel-spike, ou o marcador mudou e o gate de release deixou de o ver."
}
