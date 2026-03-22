@echo off
REM
REM Start NexiBot Bridge Service (Windows)
REM
REM This script starts the plugin-based bridge service that enables
REM OAuth token support and provider SDK integration for NexiBot.
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
for /f "tokens=1 delims=v." %%a in ('node -v') do set NODE_MAJOR=%%a
REM node -v returns "v18.x.y", strip the leading v
for /f "tokens=1 delims=." %%a in ('node -v') do set NODE_VER=%%a
set NODE_MAJOR=%NODE_VER:~1%

for /f "delims=" %%a in ('node -v') do set NODE_VERSION=%%a

if %NODE_MAJOR% lss 18 (
    echo Error: Node.js version %NODE_VERSION% is too old
    echo Please upgrade to Node.js 18.0.0 or later
    exit /b 1
)

REM Install core dependencies if needed
if not exist "node_modules" (
    echo Installing core dependencies...
    call npm install
    if %errorlevel% neq 0 (
        echo Failed to install core dependencies
        exit /b 1
    )
)

REM Install plugin dependencies
for /d %%d in (plugins\*) do (
    if exist "%%d\package.json" (
        if not exist "%%d\node_modules" (
            echo Installing dependencies for %%~nxd...
            pushd "%%d"
            call npm install
            if %errorlevel% neq 0 (
                echo Failed to install dependencies for %%~nxd
                popd
                exit /b 1
            )
            popd
        )
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
echo Starting NexiBot Bridge Service...
echo.

npm start
