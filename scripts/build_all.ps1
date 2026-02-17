# build_all.ps1 — Release build + Python tests + wheel build for each discovered venv + Rust tests.
#
# Usage:
#   .\scripts\build_all.ps1            # full pipeline
#   .\scripts\build_all.ps1 -SkipRust  # skip cargo build/test, only maturin wheels
#
# Requires: cargo, rustfmt, uv
# Python venvs: auto-discovered from .venv* directories in project root.
# Output: target/wheels/*.whl

param(
    [switch]$SkipRust
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Failed = @()

function Get-VenvPythonExes($root) {
    $venvDirs = Get-ChildItem -Path $root -Directory -Filter ".venv*" | Sort-Object Name
    $pyExes = @()

    foreach ($dir in $venvDirs) {
        $pyExe = Join-Path $dir.FullName "Scripts\python.exe"
        if (Test-Path $pyExe) {
            $pyExes += $pyExe
        }
    }

    return $pyExes
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

$Uv = Get-Command uv -ErrorAction SilentlyContinue
if (-not $Uv) {
    Write-Fail "uv not found in PATH"
    exit 1
}

$VenvPythonExes = Get-VenvPythonExes $ProjectRoot
if ($VenvPythonExes.Count -eq 0) {
    Write-Fail "No usable venv found. Expected .venv*\\Scripts\\python.exe in project root."
    exit 1
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
# 2. Python tests + maturin build: run pytest and build one wheel per venv
# ------------------------------------------------------------------
foreach ($pyExe in $VenvPythonExes) {
    $venvDir = Split-Path -Parent (Split-Path -Parent $pyExe)
    $tag = Split-Path -Leaf $venvDir
    $venvScripts = Join-Path $venvDir "Scripts"

    Write-Step "python tests ($tag)"

    $pyVer = & $pyExe --version 2>&1
    Write-Host "  $pyVer"

    $oldVirtualEnv = $env:VIRTUAL_ENV
    $oldPath = $env:PATH
    try {
        # Activate target venv context for uv --active.
        $env:VIRTUAL_ENV = $venvDir
        $env:PATH = "$venvScripts;$oldPath"

        # Install the extension into this venv before pytest.
        & uv run --active maturin develop
        if ($LASTEXITCODE -ne 0) {
            Write-Fail "maturin develop ($tag)"
            $Failed += "$tag maturin develop"
            continue
        }

        & uv run --active pytest
        if ($LASTEXITCODE -ne 0) {
            Write-Fail "pytest ($tag)"
            $Failed += "$tag pytest"
            continue
        }
        Write-Ok "pytest ($tag)"

        Write-Step "maturin build ($tag)"
        & uv run --active maturin build --release -i $pyExe
    }
    finally {
        $env:VIRTUAL_ENV = $oldVirtualEnv
        $env:PATH = $oldPath
    }

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
