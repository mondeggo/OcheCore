@echo off
setlocal
cd /d "%~dp0.." || exit /b 1

if /i "%~1"=="-h" goto usage
if /i "%~1"=="--help" goto usage

where docker >nul 2>&1
if errorlevel 1 (
    echo Install Docker Desktop, start it, and make sure docker is on PATH.
    exit /b 1
)

rem compose.yaml publishes these; the development ones keep production's alone.
if defined OCHECORE_DEV_PORT (set "OCHECORE_PORT=%OCHECORE_DEV_PORT%") else (set "OCHECORE_PORT=5180")
if defined OCHECORE_DEV_BIND (set "OCHECORE_BIND=%OCHECORE_DEV_BIND%") else (set "OCHECORE_BIND=127.0.0.1")

set COMPOSE=docker compose --project-name ochecore-dev --project-directory . -f compose.yaml -f compose.dev.yaml
if "%~1"=="" (
    %COMPOSE% up --build
) else (
    %COMPOSE% %*
)
exit /b %errorlevel%

:usage
echo Usage: scripts\dev.bat [COMPOSE_ARGS...]
echo Builds and runs OcheCore in Docker, in demo mode, at http://localhost:5180.
echo Without arguments: up --build. OCHECORE_DEV_PORT changes the port.
exit /b 0
