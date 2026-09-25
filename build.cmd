@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build.ps1" %*
set "build_result=%errorlevel%"
if not "%build_result%"=="0" pause
exit /b %build_result%
