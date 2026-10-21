"""风控预警看门狗：监控 NapCat 控制台日志与 bot 日志，命中风控信号记本地告警日志。

独立于 bot.py 运行（被 start.bat 拉起），不碰数据库与消息管道。只做一件事：
扫日志尾部，发现以下任一信号就在 riskwatch.log 里记一条 [ALERT]，
让用户翻日志即可在封号前 24h 看到信号（曾经推飞书，已按用户要求去掉推送）。

触发信号（命中即告警，每条信号有冷却避免刷屏）：
  - Resource type ... is currently disabled   腾讯熔断资源拉取（封号前置降级）
  - Login Error, ErrCode: 3                   登录被拒（非版本过低）
  - serverErrorCode 45                        版本过低（降级提醒，非致命）
  - 登录态已失效 / Process exited              会话被掐
  - 账号已被冻结 / 涉嫌违规 / 被限制登录        封号定性

冷却：同一条信号 30 分钟内只告警一次（按信号名做 key）。
日志源与日志文件路径都跟随 config.json 的 data_dir。
"""
import asyncio
import logging
import logging.handlers
import os
import re
import time

from config import load_config

_CFG = load_config()
DATA_DIR = _CFG["data_dir"].replace("\\", "/").rstrip("/") + "/"
os.makedirs(DATA_DIR, exist_ok=True)   # 日志目录不存在会让 RotatingFileHandler 首条日志即崩

# PID 落盘：面板的停止/状态探测优先读这个文件，PowerShell 按命令行匹配只是兜底
# （GUI 拉起的进程命令行可能读不到，见 qqbot.rs resolve_pids 的注释）
with open(DATA_DIR + "riskwatch.pid", "w") as _f:
    _f.write(str(os.getpid()))

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(message)s",
    handlers=[logging.StreamHandler(),
              logging.handlers.RotatingFileHandler(
                  DATA_DIR + "riskwatch.log", maxBytes=2*1024*1024,
                  backupCount=2, encoding="utf-8", delay=True)])
logger = logging.getLogger("riskwatch")

# 日志源（NapCat 控制台重定向目标 + bot 自身日志）
WATCH_FILES = [
    DATA_DIR + "napcat_console.log",
    DATA_DIR + "bot.log",
]

# 风控信号正则：每条 (name, pattern, severity)
SIGNALS = [
    ("资源熔断", re.compile(r"Resource type .* is currently disabled.*?failure:\s*\d+", re.I), "CRIT"),
    ("登录拒绝", re.compile(r"Login Error.*ErrCode:\s*3", re.I), "CRIT"),
    ("版本过低", re.compile(r"serverErrorCode\s*45|QQ版本过低", re.I), "WARN"),
    ("登录态失效", re.compile(r"登录态已失效|登录失败|Process exited\.?", re.I), "WARN"),
    ("封号定性", re.compile(r"账号.*(?:冻结|涉嫌|违规|被限制)|被限制登录", re.I), "CRIT"),
    ("频繁下线", re.compile(r"被踢下线|强制下线", re.I), "WARN"),
]

COOLDOWN = 30 * 60   # 同一信号 30 分钟内不重复告警
SCAN_INTERVAL = 60   # 每 60 秒扫一次日志尾部
TAIL_BYTES = 200_000  # 每次只扫每个文件最后 ~200KB，避免全文遍历

_last_alert: dict[str, float] = {}   # signal_key -> last alert unix ts
_last_pos: dict[str, int] = {}       # file path -> last read offset (0 = start)


def _tail_new(path: str) -> str:
    """返回文件自上次读取以来的新增内容。首次只取尾部 TAIL_BYTES。"""
    try:
        size = os.path.getsize(path)
    except OSError:
        return ""
    last = _last_pos.get(path, max(0, size - TAIL_BYTES))
    if size < last:   # 日志轮换/截断：从头读
        last = 0
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            f.seek(last)
            chunk = f.read()
            _last_pos[path] = f.tell()
        return chunk
    except OSError:
        return ""


def _scan() -> list[tuple[str, str, str]]:
    """扫所有日志的新增内容，返回 [(name, matched_text, severity)]。"""
    hits = []
    for path in WATCH_FILES:
        chunk = _tail_new(path)
        if not chunk:
            continue
        for name, pat, sev in SIGNALS:
            m = pat.search(chunk)
            if m:
                # 取命中行（往前找到换行，往后找到换行）
                i = chunk.rfind("\n", 0, m.start()) + 1
                j = chunk.find("\n", m.end())
                line = chunk[i if j < 0 else i:j].strip()
                hits.append((name, line[:200], sev))
    return hits


def _alert(name: str, line: str, sev: str) -> None:
    # key 只取信号名：日志行自带时间戳（"10-10 15:43:45 ..."），若把 line 算进 key，
    # 每次 NapCat 重试登录产生新行就绕过冷却 → 每 2 分钟刷一条。按信号类型冷却。
    now = time.time()
    if now - _last_alert.get(name, 0) < COOLDOWN:
        return
    _last_alert[name] = now
    logger.warning("[ALERT] [%s] %s | %s", sev, name, line)


async def main() -> None:
    logger.info("[START] riskwatch 监听 %d 个日志源，每 %ds 扫一次",
                len(WATCH_FILES), SCAN_INTERVAL)
    # 启动时把 pos 推到当前末尾，避免把历史错误一次性告警出去
    for path in WATCH_FILES:
        try:
            _last_pos[path] = os.path.getsize(path)
        except OSError:
            pass
    while True:
        try:
            for name, line, sev in _scan():
                _alert(name, line, sev)
        except Exception as e:
            logger.error("[SCAN ERR] %s: %s", type(e).__name__, e)
        await asyncio.sleep(SCAN_INTERVAL)


_PID_FILE = DATA_DIR + "riskwatch.pid"

if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        logger.info("[EXIT] interrupted")
    finally:
        # 正常退出自己删；被 taskkill / 崩溃时删不了，留给 viewman 的状态探测
        # 按「进程身份对不上」清掉（见 qqbot.rs probe_pids）。只删自己的
        try:
            with open(_PID_FILE, encoding="utf-8") as f:
                mine = f.read().strip() == str(os.getpid())
            if mine:
                os.remove(_PID_FILE)
        except OSError:
            pass
