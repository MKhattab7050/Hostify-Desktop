param([switch]$SkipFrontend)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
Push-Location $repo
try {
    cargo build -p hostify-engine --release --offline
    if ($LASTEXITCODE -ne 0) { throw 'Engine release build failed' }
    $targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
    New-Item -ItemType Directory -Force -Path src-tauri/resources | Out-Null
    Copy-Item -LiteralPath (Join-Path $targetRoot 'release/hostify-engine.exe') -Destination src-tauri/resources/hostify-engine.exe -Force
    & (Join-Path $PSScriptRoot 'generate-icons.ps1')
    if (!$SkipFrontend) {
        pnpm build
        if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed' }
    }
} finally { Pop-Location }
