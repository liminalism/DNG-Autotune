@echo off
setlocal
cd /d "%~dp0"

where powershell.exe >nul 2>&1
if errorlevel 1 (
  echo PowerShell was not found.
  exit /b 1
)

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-release.ps1"
exit /b %errorlevel%
