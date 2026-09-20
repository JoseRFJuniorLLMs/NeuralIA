# Constroi o instalador da NeuralIA.
#
# A ordem importa: o instalador leva a NeuralIA dentro de si, portanto o
# navegador tem de estar compilado ANTES. O `build.rs` do `neural-setup` le a
# pasta apontada por NEURALIA_PAYLOAD_DIR; sem essa variavel sai um instalador
# vazio, que se recusa a instalar em vez de criar uma pasta oca.
#
#   pwsh packaging/build-installer.ps1
#   pwsh packaging/build-installer.ps1 -OutDir dist
#
# O artefacto fica em <OutDir>/NeuralIA-Setup.exe.

[CmdletBinding()]
param(
    [string]$OutDir = "dist",
    [switch]$SkipBrowser
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    if (-not $SkipBrowser) {
        Write-Host "==> a compilar o navegador (release)" -ForegroundColor Cyan
        cargo build --release -p neural-app
        if ($LASTEXITCODE -ne 0) { throw "cargo build -p neural-app falhou" }
    }

    # O diretorio de saida do cargo pode estar redirecionado por
    # CARGO_TARGET_DIR; perguntar em vez de adivinhar "target/".
    $meta = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    $releaseDir = Join-Path $meta.target_directory "release"
    $browser = Join-Path $releaseDir "NeuralIA.exe"
    if (-not (Test-Path $browser)) { throw "nao encontrei $browser" }

    $stage = Join-Path ([System.IO.Path]::GetTempPath()) ("neuralia-payload-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    try {
        Copy-Item $browser (Join-Path $stage "NeuralIA.exe")
        $mb = [math]::Round((Get-Item $browser).Length / 1MB, 1)
        Write-Host "==> carga util: NeuralIA.exe ($mb MiB)" -ForegroundColor Cyan

        $env:NEURALIA_PAYLOAD_DIR = $stage
        Write-Host "==> a compilar o instalador (release)" -ForegroundColor Cyan
        cargo build --release -p neural-setup
        if ($LASTEXITCODE -ne 0) { throw "cargo build -p neural-setup falhou" }
    }
    finally {
        Remove-Item Env:\NEURALIA_PAYLOAD_DIR -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    }

    $setup = Join-Path $releaseDir "NeuralIA-Setup.exe"
    if (-not (Test-Path $setup)) { throw "nao encontrei $setup" }
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $final = Join-Path $OutDir "NeuralIA-Setup.exe"
    Copy-Item $setup $final -Force

    $size = [math]::Round((Get-Item $final).Length / 1MB, 1)
    $hash = (Get-FileHash $final -Algorithm SHA256).Hash.ToLower()
    Write-Host ""
    Write-Host "==> $final ($size MiB)" -ForegroundColor Green
    Write-Host "    sha256 $hash"
}
finally {
    Pop-Location
}
