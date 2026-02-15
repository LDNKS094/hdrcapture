# build_all.ps1 — Release build + maturin wheel for each Python version + Rust tests.
#
# Usage:
#   .\scripts\build_all.ps1            # full pipeline
#   .\scripts\build_all.ps1 -SkipRust  # skip cargo build/test, only maturin wheels
#
# Requires: cargo, rustfmt, maturin
# Python venvs: .venv (3.12), .venv313 (3.13) — must already exist in project root.
# Output: target/wheels/*.whl

param(
    [switch]$SkipRust
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Failed = @()

# Hardcoded venvs: name → python executable
$Venvs = [ordered]@{
    "py312" = Join-Path $ProjectRoot ".venv\Scripts\python.exe"
    "py313" = Join-Path $ProjectRoot ".venv313\Scripts\python.exe"
}

function Write-Step($msg) {
    Write-Host "`n========== $msg ==========" -ForegroundColor Cyan
}

function Write-Ok($msg) {
    Write-Host "  [OK] $msg" -ForegroundColor Green
}

function Write-Fail($msg) {
    Write-Host "  [FAIL] $msg" -ForegroundColor Red
}

# ------------------------------------------------------------------
# 1. Rust: fmt check + release build + release tests
# ------------------------------------------------------------------
if (-not $SkipRust) {
    Write-Step "cargo fmt --check"
    cargo fmt --check
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "cargo fmt --check failed. Run 'cargo fmt' first."
        exit 1
    }
    Write-Ok "formatting"

    Write-Step "cargo build --release"
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "cargo build --release"
        exit 1
    }
    Write-Ok "release build"

    Write-Step "cargo test --release"
    cargo test --release -- --test-threads=1
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "cargo test --release"
        $Failed += "cargo test"
    } else {
        Write-Ok "rust tests"
    }
}

# ------------------------------------------------------------------
# 2. maturin build: one wheel per Python version
# ------------------------------------------------------------------
foreach ($entry in $Venvs.GetEnumerator()) {
    $tag = $entry.Key
    $pyExe = $entry.Value
    $venvDir = Split-Path -Parent (Split-Path -Parent $pyExe)

    Write-Step "maturin build ($tag)"

    if (-not (Test-Path $pyExe)) {
        Write-Fail "python not found: $pyExe"
        $Failed += "$tag missing"
        continue
    }

    $pyVer = & $pyExe --version 2>&1
    Write-Host "  $pyVer"

    $env:VIRTUAL_ENV = $venvDir
    & maturin build --release -i $pyExe
    $env:VIRTUAL_ENV = $null
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "maturin build ($tag)"
        $Failed += "$tag maturin"
        continue
    }
    Write-Ok "maturin build ($tag)"
}

# ------------------------------------------------------------------
# 3. Summary
# ------------------------------------------------------------------
Write-Step "Summary"
if ($Failed.Count -eq 0) {
    Write-Host "  All steps passed." -ForegroundColor Green
} else {
    Write-Host "  Failures:" -ForegroundColor Red
    foreach ($f in $Failed) {
        Write-Host "    - $f" -ForegroundColor Red
    }
    exit 1
}
