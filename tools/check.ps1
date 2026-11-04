# Quality gate: TypeScript, the frontend unit tests (vitest), clippy (no warnings) and the Rust tests.
# Run by hand (.\tools\check.ps1) and by tools\release.ps1 before every build.
# Releases are built on Windows, not in GitLab CI (which only builds the weekly data), so this
# script is where the checks run.
# native tools write progress to stderr: judge them by exit code only
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot

function Step($name, [scriptblock]$cmd) {
    Write-Host "== $name" -ForegroundColor Cyan
    & $cmd
    if ($LASTEXITCODE -ne 0) { Write-Host "FAILED: $name" -ForegroundColor Red; exit 1 }
}

Push-Location $root
try {
    Step "tsc" { pnpm tsc --noEmit }
    Step "vitest" { pnpm test --run }
    Push-Location (Join-Path $root "src-tauri")
    try {
        Step "clippy" { cargo clippy --all-targets --features cli -- -D warnings }
        Step "tests" { cargo test --features cli }
    } finally { Pop-Location }
} finally { Pop-Location }
Write-Host "all checks passed" -ForegroundColor Green
