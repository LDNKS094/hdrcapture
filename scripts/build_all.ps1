# build_all.ps1 — Release build, maturin develop for each venv, run all tests.
#
# Usage:
#   .\scripts\build_all.ps1            # full pipeline
#   .\scripts\build_all.ps1 -SkipRust  # skip cargo build/test, only Python
#
# Requires: cargo, rustfmt
# Python venvs: .venv (3.12), .venv313 (3.13) — must already exist in project root.

param(
    [switch]$SkipRust
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Failed = @()

# Hardcoded venvs: name → directory
$Venvs = [ordered]@{
    "py312" = Join-Path $ProjectRoot ".venv"
    "py313" = Join-Path $ProjectRoot ".venv313"
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
# 2. Python: maturin develop + python tests per venv
# ------------------------------------------------------------------
foreach ($entry in $Venvs.GetEnumerator()) {
    $tag = $entry.Key
    $venvDir = $entry.Value
    $venvPython = Join-Path $venvDir "Scripts\python.exe"
    $maturin = Join-Path $venvDir "Scripts\maturin.exe"

    Write-Step "$tag"

    if (-not (Test-Path $venvPython)) {
        Write-Fail "venv not found: $venvDir"
        $Failed += "$tag venv missing"
        continue
    }

    $pyVer = & $venvPython --version 2>&1
    Write-Host "  $pyVer"

    # maturin develop --release
    Write-Host "  maturin develop --release ..."
    & $maturin develop --release
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "maturin develop ($tag)"
        $Failed += "$tag maturin"
        continue
    }
    Write-Ok "maturin develop ($tag)"

    # Run Python tests
    $testScript = Join-Path $ProjectRoot "tests\test_capture.py"
    if (Test-Path $testScript) {
        Write-Host "  Running tests/test_capture.py ..."
        & $venvPython $testScript
        if ($LASTEXITCODE -ne 0) {
            Write-Fail "python tests ($tag)"
            $Failed += "$tag python tests"
        } else {
            Write-Ok "python tests ($tag)"
        }
    }
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
