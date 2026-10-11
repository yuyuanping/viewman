@echo off
title QQ Bot Saver
rem Single NapCat install. Everything (core + QQ + config) lives in NapCat.44498.Shell.
rem It used to cross-wire NAPCAT_DIR=4.18.28 with NAPCAT_QQ=44498, so NapCat read
rem config from 44498 but loaded the core from 4.18.28 -> mismatch showed as
rem "adapter ready but 9501/9502 not listening". Now one self-contained shell.
rem 44498 bundles the full core under versions\9.9.26-44498\resources\app\napcat\
set NAPCAT_DIR=D:\tool\NapCat.Shell.Windows.OneKey\NapCat.44498.Shell
set NAPCAT_CORE=%NAPCAT_DIR%\versions\9.9.26-44498\resources\app\napcat
set NAPCAT_QQ=%NAPCAT_DIR%\QQ.exe
set NAPCAT_EXE=NapCatWinBootMain.exe
set PY=D:\Python311\python.exe
set PYW=D:\Python311\pythonw.exe
set LOG=D:\QQBot_Data\startup.log

echo [%time%] start.bat begin >> %LOG%

tasklist.exe | findstr /I "%NAPCAT_EXE%" >nul
if errorlevel 1 (
    echo [%time%] NapCat not running, starting hidden >> %LOG%
    rem Hidden by default; run %NAPCAT_DIR%\napcat.bat manually to watch QR / logs.
    rem All core files come from the same shell's versions dir to avoid cross-version config reads.
    set "NAPCAT_PATCH_PACKAGE=%NAPCAT_CORE%\qqnt.json"
    set "NAPCAT_LOAD_PATH=%NAPCAT_CORE%\loadNapCat.js"
    set "NAPCAT_INJECT_PATH=%NAPCAT_CORE%\NapCatWinBootHook.dll"
    set "NAPCAT_LAUNCHER_PATH=%NAPCAT_DIR%\%NAPCAT_EXE%"
    set "NAPCAT_MAIN_PATH=%NAPCAT_CORE:\=/%/napcat.mjs"
    powershell -NoProfile -Command "$env:NAPCAT_PATCH_PACKAGE='%NAPCAT_PATCH_PACKAGE%'; $env:NAPCAT_LOAD_PATH='%NAPCAT_LOAD_PATH%'; $env:NAPCAT_INJECT_PATH='%NAPCAT_INJECT_PATH%'; $env:NAPCAT_LAUNCHER_PATH='%NAPCAT_LAUNCHER_PATH%'; $env:NAPCAT_MAIN_PATH='%NAPCAT_MAIN_PATH%'; Start-Process -WindowStyle Hidden -WorkingDirectory '%NAPCAT_DIR%' -FilePath '%NAPCAT_DIR%\%NAPCAT_EXE%' -ArgumentList '%NAPCAT_QQ%','%NAPCAT_DIR%\NapCatWinBootHook.dll' -RedirectStandardOutput 'D:\QQBot_Data\napcat_console.log' -RedirectStandardError 'D:\QQBot_Data\napcat_console_err.log'"
    echo [%time%] waiting for NapCat WS port 9501 to listen >> %LOG%
    rem Poll 9501 for up to 90s: proceed as soon as it is up, faster and steadier than a fixed sleep.
    for /l %%i in (1,1,45) do (
        netstat -ano | findstr ":9501" | findstr "LISTENING" >nul && goto :ws_up
        ping -n 3 127.0.0.1 >nul
    )
    echo [%time%] WARN port 9501 not listening after 90s, starting bot anyway >> %LOG%
) else (
    echo [%time%] NapCat already running, skip >> %LOG%
)
:ws_up
ping -n 3 127.0.0.1 >nul

"%PY%" -c "import aiohttp, openpyxl" 2>nul
if errorlevel 1 (
    echo [%time%] installing deps aiohttp/openpyxl >> %LOG%
    "%PY%" -m pip install --disable-pip-version-check aiohttp openpyxl >nul 2>&1
)

rem Kill stale bot: tasklist /FO LIST hides CommandLine so old findstr missed them and
rem processes stacked up. Match by CommandLine via PowerShell instead. Covers both
rem python.exe and pythonw.exe (no-console runs use pythonw).
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR Name='pythonw.exe'\" | Where-Object {$_.CommandLine -like '*bot.py*'} | ForEach-Object { Write-Host ('kill old bot ' + $_.ProcessId); Stop-Process -Id $_.ProcessId -Force }" >> %LOG% 2>&1

echo [%time%] starting bot.py (pythonw, no window) >> %LOG%
start "" "%PYW%" "%~dp0bot.py"

rem Risk watchdog: separate from bot, watches NapCat logs, pushes to Lark on risk signals.
powershell -NoProfile -Command "if (-not (Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR Name='pythonw.exe'\" | Where-Object {$_.CommandLine -like '*riskwatch.py*'})) { Start-Process -WindowStyle Hidden -FilePath '%PYW%' -ArgumentList '%~dp0riskwatch.py' }" >> %LOG% 2>&1

rem No console means no crash output; after 3s confirm bot.py is alive. Other pythonw
rem processes are running too, so match by CommandLine.
ping -n 4 127.0.0.1 >nul
powershell -NoProfile -Command "if (-not (Get-CimInstance Win32_Process -Filter \"Name='pythonw.exe'\" | Where-Object {$_.CommandLine -like '*bot.py*'})) { Write-Host 'WARN no bot.py alive after launch' }" >> %LOG% 2>&1
echo [%time%] done >> %LOG%
