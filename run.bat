@echo off
title Desktop Pet AI

REM === Go to this script's folder (project root) ===
cd /d "%~dp0"

REM === Add Rust (cargo) and Node to PATH ===
set "PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\nodejs;%PATH%"

REM === Guard: if she is already running, do not start a 2nd instance (port 5173 clash) ===
tasklist /FI "IMAGENAME eq desktop-pet-ai.exe" 2>nul | find /I "desktop-pet-ai.exe" >nul
if not errorlevel 1 (
  echo Desktop Pet AI is already running. Check the taskbar / system tray icon.
  ping -n 4 127.0.0.1 >nul
  exit /b 0
)

echo ============================================
echo    Desktop Pet AI - starting (dev mode)
echo ============================================
echo.

REM === Check npm ===
where npm >nul 2>nul
if errorlevel 1 (
  echo [ERROR] npm not found. Install Node.js 20+ : https://nodejs.org
  echo.
  pause
  exit /b 1
)

REM === Check cargo (Rust) ===
where cargo >nul 2>nul
if errorlevel 1 (
  echo [ERROR] cargo not found. Install Rust : https://rustup.rs
  echo.
  pause
  exit /b 1
)

REM === First run / missing deps: install frontend packages ===
if not exist "node_modules" (
  echo [setup] First run: installing dependencies ^(npm install^) ...
  call npm install
  if errorlevel 1 (
    echo [ERROR] npm install failed.
    echo.
    pause
    exit /b 1
  )
)

echo [start] npm run tauri dev
echo         First launch compiles Rust ^(may take minutes^). Wait for her window.
echo.
call npm run tauri dev

echo.
echo Stopped. Press any key to close.
pause >nul
