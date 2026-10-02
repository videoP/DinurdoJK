@echo off
rem Run a bench plan:  bench.cmd bench\plans\deluxe.bench [--launches 3] [--only a,b] [--dry-run]
rem Writes bench\results\<plan>-<timestamp>.txt (and a -logs folder) for Claude to read.
where python >nul 2>nul
if %errorlevel%==0 (
    python "%~dp0scripts\bench.py" %*
) else (
    py -3 "%~dp0scripts\bench.py" %*
)
