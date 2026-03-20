#Requires -Version 5.1
<#
.SYNOPSIS
    Start Anthropic Bridge Service (Windows)

.DESCRIPTION
    Starts the bridge service that enables OAuth token support
    for NexiBot by using the official Anthropic TypeScript SDK.
#>

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $ScriptDir

# Check if Node.js is installed
$nodePath = Get-Command node -ErrorAction SilentlyContinue
if (-not $nodePath) {
    Write-Host "Error: Node.js is not installed" -ForegroundColor Red
    Write-Host "Please install Node.js from https://nodejs.org/"
    exit 1
}

# Check Node.js version (need >=18.0.0)
$nodeVersionRaw = & node -v
$nodeVersion = $nodeVersionRaw.TrimStart('v')
$nodeMajor = [int]($nodeVersion.Split('.')[0])

if ($nodeMajor -lt 18) {
    Write-Host "Error: Node.js version $nodeVersion is too old" -ForegroundColor Red
    Write-Host "Please upgrade to Node.js 18.0.0 or later"
    exit 1
}

# Install dependencies if needed
if (-not (Test-Path "node_modules")) {
    Write-Host "Installing dependencies..."
    npm install
}

# Check if bridge is already running on port 18790
$portInUse = Get-NetTCPConnection -LocalPort 18790 -State Listen -ErrorAction SilentlyContinue
if ($portInUse) {
    Write-Host "Warning: Bridge is already running on port 18790" -ForegroundColor Yellow
    Write-Host ""
    $reply = Read-Host "Kill existing process and restart? (y/N)"
    if ($reply -match '^[Yy]$') {
        Write-Host "Killing existing process..."
        foreach ($conn in $portInUse) {
            Stop-Process -Id $conn.OwningProcess -Force -ErrorAction SilentlyContinue
        }
        Start-Sleep -Seconds 1
    } else {
        Write-Host "Exiting"
        exit 0
    }
}

# Start the bridge
Write-Host "Starting Anthropic Bridge Service..." -ForegroundColor Cyan
Write-Host ""

npm start
