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

rem Quick login: read qq_account from config.json (gitignored). OneKey's launcher
rem takes the uin as the ONLY arg (napcat.quick.bat style) and forwards it as
rem `-q <uin>` to QQ. Passing QQ.exe/hook paths alongside makes the launcher
rem mangle the uin (`-q <QQ.exe path>` reaches the core, which can never match).
rem The launcher auto-detects QQ.exe + hook in its own dir (WorkingDirectory).
rem Falls back to QR by itself when the cached session expires.
set QQ_UIN=
for /f "usebackq delims=" %%a in (`powershell -NoProfile -Command "try{(Get-Content -Raw '%~dp0config.json'|ConvertFrom-Json).qq_account}catch{}"`) do set QQ_UIN=%%a
set "NAPCAT_ARGS="
if not "%QQ_UIN%"=="" (set "NAPCAT_ARGS=%QQ_UIN%") else set "NAPCAT_ARGS='%NAPCAT_QQ%','%NAPCAT_DIR%\NapCatWinBootHook.dll'"

echo [%time%] start.bat begin >> %LOG%

rem Kill first, then start. The old "process alive => skip" check was the zombie
rem trap: BootMain stays in tasklist while 9501/9502 are dead, so a relaunch
rem skipped NapCat forever. Now every run wipes the whole owned stack (bot.py,
rem riskwatch.py, NapCat launcher, QQ) and starts clean. A non-admin taskkill
rem can fail on QQ when NapCat launched it elevated (see stop.bat); we log and
rem continue -- the 9501 wait below will show whether the fresh start worked.
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR Name='pythonw.exe'\" | Where-Object {$_.CommandLine -like '*bot.py*' -or $_.CommandLine -like '*riskwatch.py*'} | ForEach-Object { Write-Host ('kill old ' + $_.ProcessId); Stop-Process -Id $_.ProcessId -Force }" >> %LOG% 2>&1
taskkill /F /IM %NAPCAT_EXE% >> %LOG% 2>&1
taskkill /F /IM QQ.exe >> %LOG% 2>&1
rem Let handles and listening sockets release before relaunching.
ping -n 3 127.0.0.1 >nul

rem Hidden by default; run %NAPCAT_DIR%\napcat.bat manually to watch QR / logs.
rem All core files come from the same shell's versions dir to avoid cross-version config reads.
echo [%time%] starting NapCat hidden >> %LOG%
set "NAPCAT_PATCH_PACKAGE=%NAPCAT_CORE%\qqnt.json"
set "NAPCAT_LOAD_PATH=%NAPCAT_CORE%\loadNapCat.js"
set "NAPCAT_INJECT_PATH=%NAPCAT_CORE%\NapCatWinBootHook.dll"
set "NAPCAT_LAUNCHER_PATH=%NAPCAT_DIR%\%NAPCAT_EXE%"
set "NAPCAT_MAIN_PATH=%NAPCAT_CORE:\=/%/napcat.mjs"
powershell -NoProfile -Command "$env:NAPCAT_PATCH_PACKAGE='%NAPCAT_PATCH_PACKAGE%'; $env:NAPCAT_LOAD_PATH='%NAPCAT_LOAD_PATH%'; $env:NAPCAT_INJECT_PATH='%NAPCAT_INJECT_PATH%'; $env:NAPCAT_LAUNCHER_PATH='%NAPCAT_LAUNCHER_PATH%'; $env:NAPCAT_MAIN_PATH='%NAPCAT_MAIN_PATH%'; Start-Process -WindowStyle Hidden -WorkingDirectory '%NAPCAT_DIR%' -FilePath '%NAPCAT_DIR%\%NAPCAT_EXE%' -ArgumentList %NAPCAT_ARGS% -RedirectStandardOutput 'D:\QQBot_Data\napcat_console.log' -RedirectStandardError 'D:\QQBot_Data\napcat_console_err.log'"
echo [%time%] waiting for NapCat WS port 9501 to listen >> %LOG%
rem Poll 9501 for up to 90s: proceed as soon as it is up, faster and steadier than a fixed sleep.
for /l %%i in (1,1,45) do (
    netstat -ano | findstr ":9501" | findstr "LISTENING" >nul && goto :ws_up
    ping -n 3 127.0.0.1 >nul
)
echo [%time%] WARN port 9501 not listening after 90s, starting bot anyway >> %LOG%
:ws_up
ping -n 3 127.0.0.1 >nul

"%PY%" -c "import aiohttp, openpyxl" 2>nul
if errorlevel 1 (
    echo [%time%] installing deps aiohttp/openpyxl >> %LOG%
    "%PY%" -m pip install --disable-pip-version-check aiohttp openpyxl >nul 2>&1
)

rem Both workers were killed at the top of this script (matched by CommandLine
rem via PowerShell -- tasklist /FO LIST hides CommandLine so findstr missed them
rem and processes stacked up). Launch fresh, no "already running" branch.
echo [%time%] starting bot.py (pythonw, no window) >> %LOG%
start "" "%PYW%" "%~dp0bot.py"

rem Risk watchdog: separate from bot, watches NapCat logs, records local risk alerts.
echo [%time%] starting riskwatch.py (pythonw, hidden) >> %LOG%
powershell -NoProfile -Command "Start-Process -WindowStyle Hidden -FilePath '%PYW%' -ArgumentList '%~dp0riskwatch.py'" >> %LOG% 2>&1

rem No console means no crash output; after 3s confirm both workers are alive.
rem Other pythonw processes run on this box, so match by CommandLine.
ping -n 4 127.0.0.1 >nul
powershell -NoProfile -Command "foreach ($p in @('bot.py','riskwatch.py')) { if (-not (Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR Name='pythonw.exe'\" | Where-Object {$_.CommandLine -like ('*'+$p+'*')})) { Write-Host ('WARN no ' + $p + ' alive after launch') } }" >> %LOG% 2>&1
echo [%time%] done >> %LOG%
