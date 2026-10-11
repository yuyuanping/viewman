@echo off
title QQ Bot Saver - Stop
:: 停止 NapCat + bot。建议以管理员身份运行，否则可能杀不掉 QQ 进程

:: 只杀跑 bot.py 的 python/pythonw 进程（按命令行精确匹配），不误伤其它 Python 程序
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR Name='pythonw.exe'\" | Where-Object {$_.CommandLine -like '*bot.py*'} | ForEach-Object { Write-Host ('kill bot ' + $_.ProcessId); Stop-Process -Id $_.ProcessId -Force }"

taskkill /F /IM NapCatWinBootMain.exe >nul 2>&1
taskkill /F /IM QQ.exe >nul 2>&1
if errorlevel 1 (
    echo Some processes could not be killed. Run as Administrator.
) else (
    echo Stopped.
)
