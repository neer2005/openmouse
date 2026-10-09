@echo off
setlocal
title OpenMouse Server
cd /d "%~dp0"

where py >nul 2>&1
if %errorlevel%==0 (set "PY=py -3") else (set "PY=python")

%PY% --version >nul 2>&1
if %errorlevel% neq 0 (
  echo [OpenMouse] Python 3.9 or newer was not found.
  echo [OpenMouse] Install it from https://www.python.org/downloads/
  echo [OpenMouse] Tip: tick "Add python.exe to PATH" during setup.
  pause
  exit /b 1
)

if not exist ".venv\Scripts\python.exe" (
  echo [OpenMouse] First run: creating an isolated environment...
  %PY% -m venv .venv
  if %errorlevel% neq 0 (
    echo [OpenMouse] Could not create the virtual environment.
    pause
    exit /b 1
  )
)

echo [OpenMouse] Installing dependencies (first run takes a minute)...
.venv\Scripts\python.exe -m pip install -q -r requirements.txt
if %errorlevel% neq 0 (
  echo [OpenMouse] Dependency install failed. Check your internet connection.
  pause
  exit /b 1
)

echo [OpenMouse] Starting server - your browser will open with the pairing QR.
echo [OpenMouse] If Windows Firewall asks, allow it on private networks.
.venv\Scripts\python.exe -m openmouse --open-browser

echo.
echo [OpenMouse] Server stopped.
pause
