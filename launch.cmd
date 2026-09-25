@echo off
if not exist "%~dp0target\release\DinurdoJK.exe" (
    echo Run build.cmd first. On a fresh checkout use build.cmd -Fetch.
    pause
    exit /b 1
)
"%~dp0target\release\DinurdoJK.exe" %*
set "client_result=%errorlevel%"
if not "%client_result%"=="0" pause
exit /b %client_result%
