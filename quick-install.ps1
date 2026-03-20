#Requires -Version 5.1
<#
.SYNOPSIS
    Quick Install — Fast incremental release build + install for Windows.

.DESCRIPTION
    Faster than `cargo tauri build` for dev iterations because:
      - Skips NSIS/MSI packaging
      - Incremental Rust compilation (only changed crates recompile)

    Usage: cd nexibot; .\quick-install.ps1 [-UI]
      -UI  Also rebuild the UI (needed if you changed .tsx/.css files)
#>
param(
    [switch]$UI
)

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$ConfigDir = "$env:APPDATA\ai.nexibot.desktop"
$ConfigPath = "$ConfigDir\config.yaml"
$InstallDir = "$env:LOCALAPPDATA\Programs\NexiBot"
$BinaryName = "nexibot-tauri.exe"

Write-Host "=== NexiBot Quick Install (Windows) ===" -ForegroundColor Cyan
Write-Host ""

# --- Step 1: Backup config ---
Write-Host "[1/5] Backing up config..."
if (Test-Path $ConfigPath) {
    Copy-Item $ConfigPath "$ConfigPath.pre-build" -Force
    Write-Host "      Backed up to config.yaml.pre-build"
} else {
    Write-Host "      WARNING: No config found at $ConfigPath" -ForegroundColor Yellow
}

# --- Step 2: Build UI (optional) ---
if ($UI) {
    Write-Host "[2/5] Building UI..."
    Push-Location "$ScriptDir\ui"
    npm run build
    Pop-Location
} else {
    Write-Host "[2/5] Skipping UI build (pass -UI to rebuild)"
}

# --- Step 3: Build release binary ---
Write-Host "[3/5] Building release binary (incremental)..."
Push-Location $ScriptDir
$env:CMAKE_POLICY_VERSION_MINIMUM = "3.5"
cargo build --release --bin nexibot-tauri
if ($LASTEXITCODE -ne 0) {
    Write-Host "=== BUILD FAILED ===" -ForegroundColor Red
    Pop-Location
    exit 1
}
Write-Host "      Built: src-tauri\target\release\$BinaryName"
Pop-Location

# --- Step 4: Kill running instances ---
Write-Host "[4/5] Killing running instances..."
$procs = Get-Process -Name "nexibot-tauri" -ErrorAction SilentlyContinue
if ($procs) {
    $procs | Stop-Process -Force
    Start-Sleep -Seconds 2
    Write-Host "      Killed $($procs.Count) running instance(s)"
} else {
    Write-Host "      No running instances found"
}

# --- Step 5: Copy binary to install location ---
Write-Host "[5/5] Installing..."
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$Source = "$ScriptDir\src-tauri\target\release\$BinaryName"
$Dest = "$InstallDir\$BinaryName"

Copy-Item $Source $Dest -Force
Write-Host "      Installed: $Dest"

# Copy DLLs if present (ONNX Runtime, sherpa, etc.)
$DllDir = "$ScriptDir\src-tauri\target\release"
$Dlls = @(
    "onnxruntime.dll",
    "onnxruntime_providers_shared.dll",
    "sherpa-onnx-c-api.dll",
    "sherpa-onnx-cxx-api.dll",
    "DirectML.dll",
    "cargs.dll"
)
foreach ($dll in $Dlls) {
    $DllPath = "$DllDir\$dll"
    if (Test-Path $DllPath) {
        Copy-Item $DllPath "$InstallDir\$dll" -Force
    }
}

# --- Launch and verify ---
Write-Host ""
Write-Host "Launching..."
Start-Process $Dest
Start-Sleep -Seconds 3

$running = Get-Process -Name "nexibot-tauri" -ErrorAction SilentlyContinue
if ($running) {
    Write-Host ""
    Write-Host "=== SUCCESS ===" -ForegroundColor Green
    Write-Host "NexiBot running (PID $($running.Id))"
    Write-Host ""
    Write-Host "Verify manually:"
    Write-Host "  [x] System tray icon visible"
    Write-Host "  [x] Clicking icon opens UI with content (not blank)"
} else {
    Write-Host ""
    Write-Host "=== FAILED - Process not running after launch ===" -ForegroundColor Red
    Write-Host "Check Event Viewer or run the binary directly for error output."
    exit 1
}
