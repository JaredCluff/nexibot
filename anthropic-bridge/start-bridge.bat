@echo off
REM
REM Start Anthropic Bridge Service (Windows)
REM
REM This script starts the bridge service that enables OAuth token support
REM for NexiBot by using the official Anthropic TypeScript SDK.
REM

setlocal enabledelayedexpansion

REM Change to the script's directory
cd /d "%~dp0"

REM Check if Node.js is installed
where node >nul 2>&1
if %errorlevel% neq 0 (
    echo Error: Node.js is not installed
    echo Please install Node.js from https://nodejs.org/
    exit /b 1
)

REM Check Node.js version (need >=18.0.0)
for /f "tokens=1 delims=." %%a in ('node -v') do set NODE_VER=%%a
set NODE_MAJOR=%NODE_VER:~1%

for /f "delims=" %%a in ('node -v') do set NODE_VERSION=%%a

if %NODE_MAJOR% lss 18 (
    echo Error: Node.js version %NODE_VERSION% is too old
    echo Please upgrade to Node.js 18.0.0 or later
    exit /b 1
)

REM Install dependencies if needed
if not exist "node_modules" (
    echo Installing dependencies...
    call npm install
    if %errorlevel% neq 0 (
        echo Failed to install dependencies
        exit /b 1
    )
)

REM Check if bridge is already running on port 18790
set PORT_IN_USE=0
for /f "tokens=5" %%p in ('netstat -ano ^| findstr "LISTENING" ^| findstr ":18790 "') do (
    set PORT_IN_USE=1
    set EXISTING_PID=%%p
)

if %PORT_IN_USE%==1 (
    echo Warning: Bridge is already running on port 18790 (PID: %EXISTING_PID%)
    echo.
    set /p REPLY="Kill existing process and restart? (y/N) "
    if /i "!REPLY!"=="y" (
        echo Killing existing process...
        taskkill /PID %EXISTING_PID% /F >nul 2>&1
        timeout /t 1 /nobreak >nul
    ) else (
        echo Exiting
        exit /b 0
    )
)

REM Start the bridge
echo Starting Anthropic Bridge Service...
echo.

npm start
