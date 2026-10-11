r"""每日备份：bot.db（sqlite3 backup API 一致快照，bot 运行中也能跑）+
磁力登记表 xlsx + 近 3 天的消息 md。保留 RETAIN_DAYS 天，按日期目录轮换。

手动跑：python backup.py
计划任务（每天 03:30，已注册）：
  schtasks /create /tn QQBot_Backup /tr "D:\Python311\python.exe D:\opencword\viewman\qqbot\backup.py" /sc daily /st 03:30 /f
换目的地改 BACKUP_DIR；运行日志在 <BACKUP_DIR>/backup.log。
"""
import shutil
import sqlite3
import time
from datetime import date, timedelta
from pathlib import Path

DATA_DIR = Path(r"D:\QQBot_Data")
BACKUP_DIR = Path(r"C:\QQBot_Backup")   # 备份目的地，换盘/换目录改这里
RETAIN_DAYS = 14
MD_DAYS = 3


def log(msg: str) -> None:
    line = f"{time.strftime('%Y-%m-%d %H:%M:%S')} {msg}"
    print(line)
    try:
        with open(BACKUP_DIR / "backup.log", "a", encoding="utf-8") as f:
            f.write(line + "\n")
    except OSError:
        pass


def main() -> int:
    try:
        BACKUP_DIR.mkdir(parents=True, exist_ok=True)
        dest = BACKUP_DIR / date.today().isoformat()
        dest.mkdir(exist_ok=True)

        # SQLite backup API：比复制 db 文件稳，运行中也能取一致快照
        src = sqlite3.connect(str(DATA_DIR / "bot.db"))
        dst = sqlite3.connect(str(dest / "bot.db"))
        try:
            with dst:
                src.backup(dst)
        finally:
            dst.close()
            src.close()

        xlsx = DATA_DIR / "磁力链接汇总.xlsx"
        if xlsx.is_file():
            shutil.copy2(xlsx, dest / xlsx.name)
        copied = 0
        for i in range(MD_DAYS):
            f = DATA_DIR / "messages" / f"{date.today() - timedelta(days=i)}.md"
            if f.is_file():
                shutil.copy2(f, dest / f.name)
                copied += 1

        cutoff = time.time() - RETAIN_DAYS * 86400
        removed = 0
        for d in BACKUP_DIR.iterdir():
            if d.is_dir() and d != dest and d.stat().st_mtime < cutoff:
                shutil.rmtree(d, ignore_errors=True)
                removed += 1
        log(f"OK -> {dest} (md x{copied}, 轮换删除 {removed} 个旧目录)")
        return 0
    except Exception as e:
        log(f"FAIL: {type(e).__name__}: {e}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
