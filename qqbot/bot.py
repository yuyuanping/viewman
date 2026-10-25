"""QQ Bot Saver 主逻辑（asyncio + SQLite 队列版）。

管道：WS 收到事件 -> events 表落库（唯一持久化点，之后崩溃/失败都不丢）
      -> 消费队列按序处理：文本即时写 md 视图 + messages 行
      -> 媒体/转发展开/引用回查登记为 dl=pending，fast-path 立即尝试一次，
         失败由 sweeper 按指数退避自动重做（约 4 小时快重试跑满后转 dead，
         之后每天复检一次、共约一周；replay.py 可立即重置）。
      -> sweeper 每轮扫 messages 里的磁力链接，与 xlsx 登记表按 btih 比对后补行；
         收到含 magnet 的消息会立即唤醒这一轮，不必等定时班车。
"""
import asyncio
import json
import logging
import logging.handlers
import os
import random
import sys
import time
from pathlib import Path
from typing import Any

import aiohttp

from config import load_config
from db import DB
from forward import card_text, render_node, seg_text
from links import LinkRegistry
from magnets import MagnetRegistry
from storage import Storage, md5_hint

logger = logging.getLogger("saver.bot")

MAX_CONCURRENT_DL = 4
SWEEP_INTERVAL = 20
REPLY_TEXT_LIMIT = 200   # 引用原文在 md 里的截断长度
# 打 QQ 侧消息类 API（历史、单条消息）的最小间隔。QQ 对 SsoGetGroupMsg 这类
# 请求有频率风控，批量回捞被打成 retcode 1200 / video-url-get disabled 就是
# 这么来的；所有调用点共用一个闸门，串行成 0.3s 一次。
API_MIN_GAP_S = 0.3
# 看门重连：NapCat 没起来时别按固定 5s 硬撞端口——实测停机期间 7s 一次
# 刷出 4.2 万行相同 ERROR，把 5MB×5 的滚动预算全吃掉，真正的诊断线索留不住
RECONNECT_BASE = 5       # 连上过之后断线的重连间隔（秒）
RECONNECT_MAX = 120      # 一直连不上时的指数退避上限
CONNECT_LOG_EVERY = 30   # 连续失败期间每 30 次才记一条，其余只计数

# 停机补拉：连接建立/重连后拉漫游历史，把 NapCat 无订阅期间的缺口补进同一管道
BACKFILL_ACTIVE_DAYS = 14    # 只补最近活跃过的会话
BACKFILL_PAGE = 100          # 单页历史条数
BACKFILL_MAX_PAGES = 3       # 每会话最多往前翻几页
BACKFILL_KNOWN_STREAK = 30   # 从新往旧连续见到这么多条已知消息即停
BACKFILL_INTERVAL = 6 * 3600 # 进程不重启也每 6 小时补一轮（覆盖运行中断线漏的）

# 断线缺口补录（默认开启）：只补「这段掉线期间」的消息——每个最近活跃过的会话
# 打一次历史 API、单页，范围 = 掉线时刻 - 余量。与 backfill_enabled 的 14 天
# 全量扫描不同：那个每会话 3 页 ×100 条会踩限流，这个每会话 1 页 20 条（热态实测
# 10.2s/会话，2026-10-13 最近 3 天活跃会话只有 1 个），且所有请求被 API_MIN_GAP_S
# 串行，掉线后自动补齐实时流漏掉的消息。
GAP_CHAT_DAYS = 3          # 只回看最近 3 天有过消息的会话（掉线前活跃的必在其中）
# 页大小实测（2026-10-13，同一私聊会话，get_friend_msg_history 走 HTTP）：
#   count=1 →1.0s  count=5 →1.1s  count=10 →7.0s(覆盖 70 分钟)
#   count=20 →10.2s(覆盖 22 小时)  count=50 →34.3s(覆盖 38 小时)
# 耗时几乎线性于 count，50 条要 34s 会撞上超时；20 条已够覆盖一整天的消息量。
GAP_PAGE = 20              # 单会话单页条数
GAP_TIMEOUT = 45           # 单次历史请求超时：冷启时 NapCat 明显慢于热态实测
GAP_MAX_DOWN_S = 7 * 86400 # 缺口长过一周不自动拉，交给 replay.py 手工按天补
GAP_MAX_FAILS = 3          # 连续这么多轮问不到 QQ（超时/无返回）就销账并提示
GAP_LEADIN_S = 120         # 缺口往前多取 2 分钟：断线信号本身有延迟，段也可能先到


def md_prefix(scope: str, peer_id, sender_id, nickname: str) -> str:
    """md 视图里的发言人前缀：群 [群号/昵称]，私聊 昵称(id)。"""
    if scope == "group":
        return f"[{peer_id}/{nickname}] "
    return f"[{nickname}({sender_id})] "


def row_prefix(row: dict) -> str:
    """从 messages 行重建 md_prefix——延迟/重放的下载没有原始事件在手。"""
    return md_prefix(row.get("scope") or "private", row.get("peer_id"),
                     row.get("sender_id"), row.get("sender_nick") or "?")


def reply_line(src: dict) -> str:
    """把被引用的原消息压成一行：[回复←昵称：原文]。媒体只留标签。"""
    sender = src.get("sender", {}) or {}
    who = (sender.get("card") or sender.get("nickname")
           or str(sender.get("user_id", "?")))
    segs = src.get("message") or []
    body = "".join(seg_text(s) for s in segs if isinstance(s, dict)).strip()
    if not body:
        body = str(src.get("raw_message") or "").strip()
    if not body:
        body = "(空消息)"
    return f"[回复←{who}：{body[:REPLY_TEXT_LIMIT]}]"


def setup_logging(data_dir: str) -> None:
    """输出到 <data_dir>\\bot.log（按大小滚动，保留 5 份），有控制台时同时打印。"""
    log_dir = Path(data_dir)
    log_dir.mkdir(parents=True, exist_ok=True)
    log_file = log_dir / "bot.log"

    fmt = logging.Formatter("%(asctime)s %(levelname)s %(name)s: %(message)s")
    root = logging.getLogger()
    root.setLevel(logging.INFO)
    # 避免重复添加 handler（重复 start 时）
    if root.handlers:
        return

    # pythonw.exe 下 sys.stderr 为 None，挂 StreamHandler 会让每条日志都报错
    if sys.stderr is not None:
        sh = logging.StreamHandler()
        sh.setFormatter(fmt)
        root.addHandler(sh)

    fh = logging.handlers.RotatingFileHandler(
        log_file, maxBytes=5 * 1024 * 1024, backupCount=5,
        encoding="utf-8", delay=True)
    fh.setFormatter(fmt)
    root.addHandler(fh)

    # 无窗口运行时唯一的崩溃线索就是这份日志，未捕获异常必须落盘
    def _excepthook(exc_type, exc, tb):
        if issubclass(exc_type, KeyboardInterrupt):
            return
        logging.getLogger("saver").critical("[FATAL] 未捕获异常",
                                            exc_info=(exc_type, exc, tb))
    sys.excepthook = _excepthook

    logger.info("[LOG] logging to %s", log_file)


class Bot:
    def __init__(self, config: dict, session: aiohttp.ClientSession, db: DB,
                 magnets: MagnetRegistry | None = None,
                 links: LinkRegistry | None = None):
        self.config = config
        self.session = session
        self.db = db
        self.magnets = magnets
        self.links = links
        # db 交给 storage：媒体落盘（含合并转发路径）当场登记 media 表
        self.storage = Storage(config["data_dir"], session, MAX_CONCURRENT_DL, db)
        # 全局下载限速（每秒下载数）：None/<=0 不限。配进 storage 的令牌桶
        self.storage.dl_limit = config.get("dl_limit")
        self._ws: aiohttp.ClientWebSocketResponse | None = None
        self._pending: dict[str, asyncio.Future] = {}
        self._seq = 0
        self.connect_fails = 0   # 连续「连不上」次数，连上即清零
        self._tasks: set[asyncio.Task] = set()
        # 事件消费队列：单消费者保证 md 文本时序
        self.q: asyncio.Queue = asyncio.Queue()
        self.registry_kick = asyncio.Event()
        self.backfill_kick = asyncio.Event()
        self.gap_kick = asyncio.Event()
        # 掉线起点（Unix 秒）：连接结束时打点，补完缺口才清空。None = 没欠缺口
        self.down_since: int | None = None
        self._gap_fails = 0    # 连续问不到 QQ 的缺口轮数（超时/无返回）
        self._api_next = 0.0   # 下一次允许打消息类 API 的最早时刻（monotonic）
        # 转发展开短缓存 {fid: (单调时刻, data 或 None)}：一条转发的十几个明细行
        # 常在同一班车里重试，逐行 expand_forward 会把同一转发向 QQ 连打十几次。
        # 60 秒窗口够一班车复用；None（失败/过期）也缓存，免得过期的旧转发被
        # 每行反复重试。rkey 只按分钟级轮换，缓存里的 url 在窗口内依然有效。
        self._fwd_cache: dict[str, tuple[float, Any | None]] = {}

    def _spawn(self, coro) -> asyncio.Task:
        task = asyncio.create_task(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)
        return task

    def _kick_magnets(self, text: str, mid: int | None = None) -> None:
        # 磁力/网址落库即唤醒 sweeper 去登记，不等 20 秒班车：remove_magnets.py
        # 按行删 messages，晚登记的那条链接就只剩 Excel 里没人知道了。
        low = (text or "").lower()
        if self.magnets and "magnet:" in low:
            if mid is not None:
                # 回填走的是 UPDATE 老行，id 可能已低于 sweeper 游标，必须点名重扫
                self.magnets.dirty.add(mid)
            self.registry_kick.set()
        if self.links and ("http://" in low or "https://" in low):
            if mid is not None:
                self.links.dirty.add(mid)
            self.registry_kick.set()

    # ---- OneBot API（请求/响应同步） ----
    async def _api_gate(self) -> None:
        """消息类 API 串行闸门：所有调用点共用，见 API_MIN_GAP_S。"""
        now = time.monotonic()
        if now < self._api_next:
            await asyncio.sleep(self._api_next - now)
        self._api_next = time.monotonic() + API_MIN_GAP_S

    async def call_api(self, action: str, params: dict, timeout: float = 15) -> Any | None:
        ws = self._ws
        if ws is None or ws.closed:
            logger.error("[API SKIP] %s - websocket not connected", action)
            return None
        self._seq += 1
        echo = f"api_{self._seq}_{int(time.time() * 1_000_000)}"
        fut: asyncio.Future = asyncio.get_running_loop().create_future()
        self._pending[echo] = fut
        try:
            await ws.send_json({"action": action, "params": params, "echo": echo})
            full = await asyncio.wait_for(fut, timeout)
        except TimeoutError:
            logger.error("[API TIMEOUT] %s", action)
            return None
        except Exception as e:
            logger.error("[API ERROR] %s: %s", action, e)
            return None
        finally:
            self._pending.pop(echo, None)
        if full.get("status") == "ok":
            return full.get("data")
        logger.error("[API ERROR] %s: %s", action,
                     json.dumps(full, ensure_ascii=False)[:300])
        return None

    async def get_file_url(self, file_id: str) -> str | None:
        """通过 NapCat HTTP API (get_file) 取文件本地路径。

        NapCat 把本地绝对路径放在 data.file 还是 data.url 并不统一（另一字段留空
        字符串），两个都要认；storage 侧本地路径和 http url 都能处理。
        """
        api_url = self.config.get("api_url", "http://127.0.0.1:9502").rstrip("/") + "/get_file"
        headers = {"Authorization": f"Bearer {self.config.get('ws_token', '')}"}
        # 私聊文件常需 NapCat 现场从 QQ 服务器拉取，旧 15s 曾大面积超时
        # （[FILE SKIP] cannot resolve file source）。改为 90s + 3 次尝试。
        for attempt in (1, 2, 3):
            try:
                async with self.session.post(
                        api_url, headers=headers, json={"file_id": file_id},
                        timeout=aiohttp.ClientTimeout(total=90)) as resp:
                    d = await resp.json()
                data = d.get("data") or {}
                loc = data.get("file") or data.get("url")
                if d.get("status") == "ok" and loc:
                    if attempt > 1:
                        logger.info("[get_file_url] resolved on attempt %d", attempt)
                    return loc
                logger.error("[get_file_url] bad response: %s",
                             json.dumps(d, ensure_ascii=False)[:300])
            except Exception as e:
                logger.error("[get_file_url] attempt %d/3 failed: %s", attempt, e)
            if attempt < 3:
                await asyncio.sleep(2)
        return None

    async def expand_forward(self, fid: str) -> Any | None:
        return await self.call_api("get_forward_msg", {"id": fid}, timeout=60)

    async def get_message(self, message_id: str) -> Any | None:
        """取单条消息原文（用于还原 [回复] 引用的那条消息）。"""
        try:
            mid: Any = int(message_id)
        except (TypeError, ValueError):
            mid = message_id
        await self._api_gate()
        return await self.call_api("get_msg", {"message_id": mid}, timeout=20)

    # ---- 事件流水线 ----
    async def _ingest(self, raw: str, data: dict) -> None:
        # 落库是唯一持久化点：此后任何环节失败都可从 events 表重做
        try:
            eid = await self.db.add_event(raw)
        except Exception as e:
            logger.error("[INGEST FAIL] %s - 降级为直接处理", e)
            await self._run_event_safe(0, data)
            return
        await self.q.put((eid, data))

    async def worker(self) -> None:
        while True:
            eid, data = await self.q.get()
            # 唯一消费者不许死：claim/finish 的 DB 异常只跳过这一条，
            # 事件仍是 new，下一轮 sweeper（20s 内）会重新捞起回放
            try:
                await self._run_event_safe(eid, data)
            except Exception as e:
                logger.error("[WORKER] id=%s: %s", eid, e)

    async def _run_event_safe(self, eid: int, data: dict) -> None:
        if eid and not await self.db.claim_event(eid):
            return  # 已被 sweeper 认领
        try:
            await self._process_event(eid, data)
        except Exception as e:
            logger.error("[EVENT FAIL] id=%s: %s", eid, e)
            if eid:
                await self.db.fail_event(eid, str(e))
            return
        if eid:
            await self.db.finish_event(eid)

    async def _process_event(self, eid: int, data: dict) -> None:
        if data.get("post_type") != "message":
            return
        msg_type = data.get("message_type")
        if msg_type not in ("private", "group"):
            return
        # 事件重放幂等：清掉上次尝试已写的 messages 行（md 由下方缓冲天然幂等）
        if eid:
            await self.db.clear_event_messages(eid)
        # 只存私聊（2026-10-11 用户要求）：群聊消息直接跳过，不入库不落盘。
        # 想恢复群聊存档，config.json 里 save_group=true 重启即可
        if msg_type == "group" and not self.config.get("save_group", False):
            return
        is_group = msg_type == "group"
        scope = "group" if is_group else "private"
        sender = data.get("sender", {})
        sender_id = sender.get("user_id")
        nickname = str(sender.get("nickname", sender_id if sender_id is not None else "?"))
        peer_id = data.get("group_id") if is_group else sender_id
        ts = int(data.get("time") or time.time())
        # 群消息文本带 [群号/昵称] 前缀写统一日记；私聊前缀带 user_id（昵称可改）
        prefix = md_prefix(scope, peer_id, sender_id, nickname)
        logger.info("[RECV] %s from %s(%s) segs=%d",
                    msg_type, peer_id, nickname, len(data.get("message", [])))
        kinds = ("text", "image", "video", "file", "forward", "reply")
        msg_id = str(data.get("message_id") or "")
        # md 写入统一缓冲：事件全部段处理成功才落盘。逐段直写的历史行为在
        # 事件重放时会把已写过的段再写一遍（媒体有 md5 去重，纯文本会重复成行）
        md_pending: list[tuple[str, str, int | None]] = []
        for i, seg in enumerate(data.get("message", [])):
            t = seg.get("type")
            d = seg.get("data", {})
            common = dict(event_id=eid or None, msg_id=msg_id, scope=scope,
                          peer_id=peer_id, sender_id=sender_id,
                          sender_nick=nickname, ts=ts,
                          seg_index=i, kind=t if t in kinds else "other")
            if t == "text":
                md_pending.append((d.get("text", ""), prefix, ts))
                mid = await self.db.add_message(**common, text=d.get("text", ""))
                self._kick_magnets(d.get("text", ""), mid)
            elif t in ("image", "video"):
                # NapCat 的图片/视频段把 file id 放在 data.file（形如 HASH.jpg），
                # 没有 file_id 键。不存下来就没有 CDN 失败后的本地原件兜底。
                row = dict(common, src=d.get("url", ""),
                           ref=d.get("file_id") or d.get("file") or "",
                           dl="pending")
                mid = await self.db.add_message(**row)
                self._spawn(self._attempt_row(dict(row, id=mid)))
            elif t == "file":
                row = dict(common, src=d.get("url", ""),
                           ref=d.get("file_id", ""), name=d.get("file", "file"),
                           dl="pending")
                mid = await self.db.add_message(**row)
                self._spawn(self._attempt_row(dict(row, id=mid)))
            elif t == "record":
                # 语音：NapCat 给的常是 mp3（也可能 silk），按文件落盘
                name = d.get("file") or f"voice_{msg_id or ts}.mp3"
                row = dict(common, src=d.get("url", ""),
                           ref=d.get("file_id") or d.get("file") or "",
                           name=name, dl="pending")
                mid = await self.db.add_message(**row)
                self._spawn(self._attempt_row(dict(row, id=mid)))
            elif t == "mface":
                # 商城表情：有直链，当图片存
                row = dict(common, kind="image", src=d.get("url", ""),
                           ref=d.get("file_id") or d.get("emoji_id") or "",
                           name=d.get("file") or "", dl="pending")
                mid = await self.db.add_message(**row)
                self._spawn(self._attempt_row(dict(row, id=mid)))
            elif t == "forward":
                md_pending.append(("[合并转发] 正在展开...", "", ts))
                mid = await self.db.add_message(**common, ref=d.get("id", ""),
                                                dl="pending")
                self._spawn(self._attempt_row(dict(common, id=mid,
                                                   ref=d.get("id", ""))))
            elif t == "reply":
                # 事件里只给被引用消息的 id，原文须回查 get_msg。不阻塞 worker：
                # 取回后补写一行，失败由 sweeper 退避重试，穷尽才退回裸标签。
                rid = str(d.get("id", "") or "")
                if not rid:
                    md_pending.append(("[回复]", prefix, ts))
                    await self.db.add_message(**common, text="[回复]")
                else:
                    row = dict(common, ref=rid, dl="pending")
                    mid = await self.db.add_message(**row)
                    self._spawn(self._attempt_row(dict(row, id=mid)))
            elif t in ("json", "xml", "share", "music", "location"):
                # 卡片/分享类：提取载体里的网址/磁力一起存档，纯占位会把链接丢掉
                text = card_text(seg)
                md_pending.append((text, prefix, ts))
                mid = await self.db.add_message(**common, text=text)
                self._kick_magnets(text, mid)
            else:
                label = {"face": "[表情]", "at": "[@]"} .get(t, f"[{t}]")
                md_pending.append((label, prefix, ts))
                await self.db.add_message(**common, text=label)

        # 事件全部段处理成功，md 才一次性落盘；中途失败走 fail_event 重放，
        # 缓冲直接丢弃，不会把半条事件写进日记
        for text, pfx, t in md_pending:
            self.storage.append_text(text, pfx, ts=t)

    # ---- dl 重试队列 ----
    async def sweeper(self) -> None:
        while True:
            try:
                await self._sweep_once()
            except Exception as e:
                logger.error("[SWEEP] %s", e)
            # 定时班车之外，收到磁力/网址链接会被 _kick_magnets 提前叫醒。
            # clear 放在 wait 之后：唤醒期间到达的链接不会丢，最坏是多跑一轮。
            try:
                await asyncio.wait_for(self.registry_kick.wait(), SWEEP_INTERVAL)
            except TimeoutError:
                pass
            self.registry_kick.clear()

    async def _sweep_once(self) -> None:
        for ev in await self.db.fetch_due_events(limit=10):
            try:
                data = json.loads(ev["payload"])
            except Exception as e:
                await self.db.fail_event(ev["id"], f"bad payload: {e}")
                continue
            self._spawn(self._run_event_safe(ev["id"], data))
        for row in await self.db.claim_pending_dl(limit=8):
            self._spawn(self._attempt_row(row, claimed=True))
        for reg, tag in ((self.magnets, "[MAGNET]"), (self.links, "[LINK]")):
            if reg:
                try:
                    await reg.sync(self.db)
                except Exception as e:
                    logger.error("%s sync 异常: %s: %s", tag, type(e).__name__, e)

    # ---- 停机补拉 ----
    async def backfill_task(self) -> None:
        """WS 每次连上（含重连）补一轮 + 定期补一轮，去重让它多跑无害。"""
        while True:
            try:
                await asyncio.wait_for(self.backfill_kick.wait(), BACKFILL_INTERVAL)
            except TimeoutError:
                pass
            self.backfill_kick.clear()
            if self._ws is None or self._ws.closed:
                continue   # 断线中：等重连唤醒，别空打 API
            try:
                await self._backfill_all()
            except Exception as e:
                logger.error("[BACKFILL] %s: %s", type(e).__name__, e)

    async def _backfill_all(self) -> None:
        chats = await self.db.active_chats(BACKFILL_ACTIVE_DAYS)
        if not chats:
            return
        t0 = time.time()
        total = none_cnt = 0
        for ch in chats:
            try:
                n = await self._backfill_chat(ch["scope"], ch["peer_id"])
            except Exception as e:
                logger.warning("[BACKFILL] %s %s: %s: %s", ch["scope"],
                               ch["peer_id"], type(e).__name__, e)
                continue
            if n is None:      # API 未返回数据（版本不支持/无漫游）
                none_cnt += 1
            else:
                total += n
        if total:
            logger.info("[BACKFILL] 补录 %d 条历史消息（%d 个会话，%.1fs）",
                        total, len(chats), time.time() - t0)
        elif none_cnt >= len(chats):
            logger.warning("[BACKFILL] NapCat 未返回任何历史消息"
                           "（API 不可用或无漫游记录），本轮跳过")

    async def _backfill_chat(self, scope: str, peer_id) -> int | None:
        """拉一个会话的漫游历史，没见过的走正常管道补录。返回补录条数；
        None 表示 API 压根没给数据。"""
        action = ("get_group_msg_history" if scope == "group"
                  else "get_friend_msg_history")
        param = "group_id" if scope == "group" else "user_id"
        known_streak = 0
        added = 0
        cursor = None
        got_any = False
        for _ in range(BACKFILL_MAX_PAGES):
            params: dict[str, Any] = {param: peer_id, "count": BACKFILL_PAGE}
            if cursor is not None:
                params["message_seq"] = cursor
            await self._api_gate()
            data = await self.call_api(action, params, timeout=30)
            if not data:
                return added if got_any else None
            msgs = data.get("messages") or []
            if not msgs:
                break
            got_any = True
            msgs.sort(key=lambda m: m.get("time") or 0)
            seqs = [m.get("message_seq") for m in msgs
                    if m.get("message_seq") is not None]
            oldest_seq = min(seqs) if seqs else None
            fresh: list[dict] = []
            for m in reversed(msgs):          # 从新往旧判重
                if await self._msg_known(m):
                    known_streak += 1
                    if known_streak >= BACKFILL_KNOWN_STREAK:
                        break
                else:
                    known_streak = 0
                    fresh.append(m)
            for x in reversed(fresh):         # 旧 -> 新补录，日记顺序才对
                await self._ingest(json.dumps(x, ensure_ascii=False), x)
                added += 1
            if known_streak >= BACKFILL_KNOWN_STREAK:
                break
            if oldest_seq is None or oldest_seq == cursor:
                break   # 拿不到翻页游标 / 翻页无进展（message_seq 语义怪是常态）
            cursor = oldest_seq
        return added

    async def _msg_known(self, m: dict) -> bool:
        scope = "group" if m.get("message_type") == "group" else "private"
        peer = m.get("group_id") if scope == "group" else m.get("user_id")
        sender = (m.get("sender") or {}).get("user_id")
        return await self.db.msg_known(str(m.get("message_id") or ""),
                                       scope, peer, sender,
                                       int(m.get("time") or 0))

    # ---- 断线缺口补录 ----
    async def gap_task(self) -> None:
        """每次重连后补一次掉线期间的缺口。只有真问到 QQ 才销账：历史请求很慢
        （实测热态 10.2s/会话，冷启能到 34s），一问不到就欠着等下次重连，
        连续 GAP_MAX_FAILS 轮问不到才放弃并提示改用 replay.py。
        补的过程中再次断线时 down_since 已被 run() 改成新的掉线时刻，
        那一轮的账不销，留给下一轮从更早的时间点接着补。"""
        while True:
            await self.gap_kick.wait()
            self.gap_kick.clear()
            since = self.down_since
            if since is None:
                continue
            if self._ws is None or self._ws.closed:
                continue   # 又断了：run() 下次连上会重新 kick
            try:
                answered = await self._gap_backfill(int(since))
            except Exception as e:
                logger.error("[GAP] %s: %s", type(e).__name__, e)
                answered = False
            self._gap_settle(answered, int(since))

    def _gap_settle(self, answered: bool, since: int) -> None:
        """一轮缺口跑完后的销账/欠账判定。"""
        if self.down_since != since:
            return   # 补的过程中又断线，新缺口自己会 kick 下一轮
        if answered:
            self._gap_fails = 0
            self.down_since = None
            return
        self._gap_fails += 1
        if self._gap_fails < GAP_MAX_FAILS:
            logger.warning("[GAP] 第 %d 轮没问到 QQ，缺口继续欠着", self._gap_fails)
            return
        logger.warning("[GAP] 连续 %d 轮问不到 QQ，放弃这段 %.1f 小时的缺口 —— "
                       "要补历史请开 backfill_enabled 或跑 python replay.py",
                       self._gap_fails, (time.time() - since) / 3600)
        self._gap_fails = 0
        self.down_since = None

    async def _gap_backfill(self, since: int) -> bool:
        """补这段缺口；返回是否至少问到了一个会话（一个都没问到就别销账）。"""
        span = int(time.time()) - since
        if span > GAP_MAX_DOWN_S:
            logger.warning("[GAP] 缺口 %.1f 天，超出自动补录范围（%d 天），跳过 —— "
                           "要这段历史请跑 python replay.py <起> <止>",
                           span / 86400, GAP_MAX_DOWN_S // 86400)
            return True
        floor_ts = since - GAP_LEADIN_S
        chats = await self.db.active_chats(GAP_CHAT_DAYS)
        if not chats:
            logger.info("[GAP] 缺口 %d 分钟，最近 %d 天无活跃会话，无需补录",
                        span // 60, GAP_CHAT_DAYS)
            return True
        added = answered = 0
        for ch in chats:
            try:
                n = await self._gap_backfill_chat(ch["scope"], ch["peer_id"], floor_ts)
            except Exception as e:
                logger.warning("[GAP] %s %s: %s: %s", ch["scope"], ch["peer_id"],
                               type(e).__name__, e)
                continue
            if n is None:
                logger.warning("[GAP] %s %s 历史请求无返回（超时或 API 不可用）",
                               ch["scope"], ch["peer_id"])
                continue
            answered += 1
            added += n
        logger.info("[GAP] 缺口 %d 分钟：扫 %d 个会话，问到 %d 个，补录 %d 条",
                    span // 60, len(chats), answered, added)
        return answered > 0

    async def _gap_backfill_chat(self, scope: str, peer_id,
                                 floor_ts: int) -> int | None:
        """一个会话一页历史，只取缺口起点之后的、且库里没有的，走正常管道入库。
        返回 None 表示这一页压根没拿到——调用方据此决定缺口欠不欠着。"""
        action = ("get_group_msg_history" if scope == "group"
                  else "get_friend_msg_history")
        param = "group_id" if scope == "group" else "user_id"
        await self._api_gate()
        data = await self.call_api(action, {param: peer_id, "count": GAP_PAGE},
                                   timeout=GAP_TIMEOUT)
        if not data:
            return None
        msgs = [m for m in (data.get("messages") or [])
                if int(m.get("time") or 0) >= floor_ts]
        msgs.sort(key=lambda m: m.get("time") or 0)
        added = 0
        for m in msgs:                 # 旧 -> 新补录，日记顺序才对
            if await self._msg_known(m):
                continue
            await self._ingest(json.dumps(m, ensure_ascii=False), m)
            added += 1
        return added

    @staticmethod
    def _fresh_url(msg: dict, kind: str, seg_index) -> str | None:
        """从 get_msg 回来的消息里挑出本段的新鲜 url。
        先认段号 + 类型都对得上的，其次同类型的，最后任何带 url 的段（单段消息常见）。"""
        if not isinstance(msg, dict):
            return None
        segs = [(i, s) for i, s in enumerate(msg.get("message") or [])
                if isinstance(s, dict)]
        for same in (lambda i, s: i == seg_index and s.get("type") == kind,
                     lambda i, s: s.get("type") == kind,
                     lambda i, s: True):
            for i, s in segs:
                url = (s.get("data") or {}).get("url")
                if url and same(i, s):
                    return url
        return None

    async def _refresh_src(self, row: dict, kind: str, src: str) -> str:
        """重试前先换个新地址。存下来的 CDN url 带 rkey（QQ 给的签名参数），
        放几天就必然失效——拿死链重试要白跑 8 次快重试再加一周复检。

        两条刷新路径，按行有没有 msg_id 分流：
        - 顶层消息行：有 msg_id，调 get_msg 拿当前有效地址。
        - 转发明细行：无 msg_id（parent 非空指向父转发行），父行 ref 存的是转发
          id——重新 get_forward_msg 展开父转发，按文件名匹配本段的新 url。NapCat
          的转发展开段只有 file（哈希文件名）+ url（带 rkey 直链），没有 file_id
          键，所以 ref 当初只能存文件名；重展开按 name 匹配是唯一能刷新的路径。

        首次尝试（tries=0）不折腾：那时地址刚拿到，本来就是活的。
        WS 没连着、消息已翻不到或没给出 url 都沿用旧 src。"""
        if int(row.get("tries") or 0) == 0:
            return src
        if row.get("msg_id"):
            return await self._refresh_src_by_msg(row, kind, src)
        if row.get("parent"):
            return await self._refresh_src_by_forward(row, kind, src)
        return src

    async def _refresh_src_by_msg(self, row: dict, kind: str, src: str) -> str:
        """顶层消息行：get_msg 回来的段里就是当前有效地址。"""
        ws = self._ws
        if ws is None or ws.closed:
            return src   # 断线中打不出 get_msg，别让重试管线刷一屏 API SKIP
        data = await self.get_message(row["msg_id"])
        fresh = self._fresh_url(data or {}, kind, row.get("seg_index"))
        if not fresh or fresh == src:
            return src
        await self.db.set_message_src(row["id"], fresh)
        logger.info("[DL REURL] id=%d 旧地址已换签，用 get_msg 的新 url 重试",
                    row["id"])
        return fresh

    async def _refresh_src_by_forward(self, row: dict, kind: str, src: str) -> str:
        """转发明细行：用父行 ref 里的转发 id 重新展开，按文件名匹配本段新 url。

        NapCat 展开转发时对同一文件名给的是确定 url（同转发内同名媒体是同一内容，
        见当初排查），所以按 name 匹配不会因撞车拿错地址。转发 id 在 QQ 端也会过期
        （retcode 1200 消息已过期），过期则拿不到新 url、沿用旧 src 走正常退避。"""
        ws = self._ws
        if ws is None or ws.closed:
            return src
        parent_id = row["parent"]
        # 父转发行的 ref 存着 forward id；取不到（父行已删/异常）就沿用旧 src
        fid = await self.db.get_message_ref(parent_id)
        if not fid:
            return src
        now = time.monotonic()
        hit = self._fwd_cache.get(fid)
        if hit and now - hit[0] < 60:
            data = hit[1]
        else:
            data = await self.expand_forward(fid)
            self._fwd_cache[fid] = (now, data)
            if len(self._fwd_cache) > 32:    # 防泄漏：只留最近展开的几条
                oldest = min(self._fwd_cache, key=lambda k: self._fwd_cache[k][0])
                self._fwd_cache.pop(oldest, None)
        if not data:
            return src   # 转发已过期 / WS 打不通——沿用旧 src 走退避
        fresh = self._fresh_forward_url(data, kind, row.get("name"))
        if not fresh or fresh == src:
            return src
        await self.db.set_message_src(row["id"], fresh)
        logger.info("[DL REURL] id=%d 转发子行按文件名匹配到新 url，父转发 %s",
                    row["id"], fid[:16])
        return fresh

    @staticmethod
    def _fresh_forward_url(data: dict, kind: str, name: str | None) -> str | None:
        """从 get_forward_msg 的返回里按文件名挑出本段的新鲜 url。

        转发体内层消息是平铺的 message 列表，每个 media 段的 data.file 是
        哈希文件名、data.url 是带 rkey 的直链。同一转发内同名段给同一 url
        （同内容复用），所以按 name 精确匹配即可；匹配不到就返回 None 走旧
        src 退避——绝不"取第一个同类型段"兜底，猜中别的文件会把错内容下进这行。"""
        if not isinstance(data, dict) or not name:
            return None
        for m in data.get("messages", []):
            if not isinstance(m, dict):
                continue
            for seg in m.get("message", []):
                if not isinstance(seg, dict) or seg.get("type") != kind:
                    continue
                d = seg.get("data") or {}
                if d.get("file") == name and d.get("url"):
                    return d["url"]
        return None

    async def _attempt_row(self, row: dict, claimed: bool = False) -> None:
        mid = row["id"]
        # 认领失败 = sweeper 已接手这一行；两条路径同时跑会往 md 重复写一行
        if not claimed and not await self.db.claim_dl_row(mid):
            return
        kind = row.get("kind", "")
        scope = row.get("scope") or "private"
        had_text = bool((row.get("text") or "").strip())

        async def _progress_self(done: int, total: int | None) -> None:
            """顶层媒体/文件行下载中把进度报在自己行上；转发行的聚合进度见下方"""
            await self.db.set_dl_progress(mid, done, total)

        try:
            if kind == "forward":
                fid = row.get("ref", "")
                if not fid:
                    raise RuntimeError("empty forward id")
                data = await self.expand_forward(fid)
                if not data:
                    raise RuntimeError("get_forward_msg returned nothing")
                # 媒体熔断：转发送来后先检查原文是否命中高危词，命中则
                # 展开时只产文本占位，不下载内部媒体（涉黄资源常见于合并转发）
                stats: dict = {}
                # 块内媒体一次下载一个，实时进度聚合报在父行：已完成段的字节由
                # report 逐段落库时推进 base，当前段的 done 实时加在上面
                seg = {"base": 0}

                def _block_progress():
                    async def cb(done: int, total: int | None) -> None:
                        # seg["base"] 在回调时刻读：前序段落完成时 report 已滚入 base，
                        # 创建时快照的话第二段起就丢掉前面段的字节。
                        # total 是「当前这一段」的大小，配整块累计字节会误导
                        # （done 还可能反超 total）——块行只报累计，不报百分比
                        await self.db.set_dl_progress(mid, seg["base"] + done, None)
                    return cb

                async def report(item: dict) -> None:
                    """转发明细逐段落库：文本/嵌套转发记 dl=none，成功媒体记 ok，
                    失败媒体进 pending 由既有重试管线按 src/ref 重下"""
                    try:
                        kind = item.get("kind", "")
                        base = dict(event_id=row.get("event_id"), msg_id="", scope=scope,
                                    peer_id=row.get("peer_id"), sender_id=None,
                                    sender_nick=str(item.get("sender") or "[转发]"),
                                    ts=int(item.get("ts") or 0) or None,
                                    seg_index=None, parent=mid)
                        if kind in ("text", "forward"):
                            await self.db.add_message(**base, kind=kind,
                                                      text=item.get("text", ""))
                            return
                        if item.get("ok"):
                            await self.db.add_message(
                                **base, kind=kind, name=item.get("file"),
                                src=item.get("src"), ref=item.get("ref"),
                                md5=item.get("md5"), path=item.get("path"),
                                bytes=item.get("bytes"), dl="ok",
                                dedup=int(bool(item.get("dedup"))))
                            # 本段落盘，字节滚入父行聚合进度的 base
                            seg["base"] += int(item.get("bytes") or 0)
                        else:
                            # 展开时下载失败：进正常重试队列按 src/ref 重下
                            await self.db.add_message(
                                **base, kind=kind, name=item.get("file"),
                                src=item.get("src"), ref=item.get("ref"),
                                dl="pending")
                    except Exception as e:
                        logger.error("[FWD ITEM] 明细落库失败 id=%d: %s", mid, e)

                text = await render_node(data.get("messages", []), self.storage,
                                         self.expand_forward, self.get_file_url,
                                         scope, stats=stats, report=report,
                                         on_progress=_block_progress())
                if not text:
                    raise RuntimeError("forward rendered empty")
                # 先记账（text/ok）后写 md：崩溃落在中间时重放会因 had_text
                # 跳过 md，宁可日记缺一块也不要同一块写两遍
                # dedup = 本条转发里复用现有文件（重复跳过）的媒体数
                await self.db.set_message_text(mid, text, stats.get("dedup", 0))
                self._kick_magnets(text, mid)
                await self.db.mark_dl_ok(mid, None, None, None)
                if not had_text:
                    self.storage.append_text(f"[合并转发]\n{text}",
                                             ts=row.get("ts"))
                logger.info("[EXPAND OK] id=%d fid=%s", mid, fid)
                return
            if kind == "reply":
                rid = row.get("ref", "")
                if not rid:
                    raise RuntimeError("empty reply id")
                src = await self.get_message(rid)
                if not src:
                    raise RuntimeError("get_msg returned nothing")
                line = reply_line(src)
                await self.db.set_message_text(mid, line)
                self._kick_magnets(line, mid)
                await self.db.mark_dl_ok(mid, None, None, None)
                self.storage.append_text(line, row_prefix(row), ts=row.get("ts"))
                logger.info("[REPLY OK] id=%d quoted=%s", mid, rid)
                return
            if kind == "file":
                arg = row.get("ref") or row.get("src") or ""
                r = await self.storage.save_file(arg, row.get("name") or "file",
                                                 self.get_file_url, scope,
                                                 on_progress=_progress_self)
            else:  # image / video
                r = None
                src = await self._refresh_src(row, kind, row.get("src", ""))
                if src:
                    r = await self.storage.save_media(
                        src, kind, scope, expect_md5=md5_hint(row.get("ref")),
                        on_progress=_progress_self)
                if r is None and row.get("ref"):
                    # CDN url 可能已过期：改经 NapCat file_id 取本地原件重试
                    p = await self.get_file_url(row["ref"])
                    if p:
                        r = await self.storage.save_media(
                            p, kind, scope, expect_md5=md5_hint(row["ref"]),
                            on_progress=_progress_self)
            if r is None:
                raise RuntimeError("download failed")
            await self.db.mark_dl_ok(mid, r["md5"], r["path"], r["bytes"],
                                     r.get("dedup", False))
            # media 表登记由 storage._register 统一负责（含转发路径）
        except Exception as e:
            status = await self.db.bump_dl(mid, str(e))
            logger.warning("[DL RETRY] id=%d kind=%s -> %s: %s", mid, kind, status, e)
            if status == "dead" and kind in ("forward", "reply"):
                label = "[合并转发] 展开失败" if kind == "forward" else "[回复]"
                self.storage.append_text(label, row_prefix(row), ts=row.get("ts"))

    # ---- WS 收发 ----
    def _on_raw(self, raw: str) -> None:
        try:
            data = json.loads(raw)
        except Exception as e:
            logger.error("[ERROR] bad ws payload: %s", e)
            return
        if "echo" in data:
            fut = self._pending.get(data.get("echo", ""))
            if fut is not None and not fut.done():
                fut.set_result(data)
        else:
            self._spawn(self._ingest(raw, data))

    def _fail_pending(self, reason: str) -> None:
        for fut in self._pending.values():
            if not fut.done():
                fut.set_result({"status": "error", "wording": reason})

    async def run(self) -> bool:
        """单次连接的生命周期：返回本次是否建立过连接，重连由外层看门循环负责。"""
        token = self.config.get("ws_token", "")
        headers = {"Authorization": f"Bearer {token}"} if token else {}
        connected = False
        try:
            # heartbeat=30 发协议层 ping，NapCat 不回 pong 即判超时断开，
            # 解决 TCP 半开假死时静默失联的问题
            async with self.session.ws_connect(self.config["ws_url"],
                                               headers=headers, heartbeat=30) as ws:
                self._ws = ws
                connected = True
                if self.connect_fails:
                    logger.info("[WS RECOVERED] 断连 %d 次后重新连上", self.connect_fails)
                    self.connect_fails = 0
                logger.info("[WS CONNECTED] Listening for messages (url=%s)",
                            self.config.get("ws_url"))
                self.backfill_kick.set()   # 连接建立即唤醒停机补拉
                if self.down_since is not None:
                    self.gap_kick.set()    # 欠着一段缺口，连上了就去补
                async for msg in ws:
                    if msg.type is aiohttp.WSMsgType.TEXT:
                        self._on_raw(msg.data)
                    elif msg.type in (aiohttp.WSMsgType.CLOSE, aiohttp.WSMsgType.CLOSING,
                                      aiohttp.WSMsgType.CLOSED, aiohttp.WSMsgType.ERROR):
                        break
        except Exception as e:
            self.connect_fails += 1
            if self.connect_fails == 1 or self.connect_fails % CONNECT_LOG_EVERY == 0:
                logger.error("[WS ERROR] %s（连续第 %d 次连不上）", e, self.connect_fails)
        finally:
            self._ws = None
            self._fail_pending("websocket closed")
            if connected:
                # 掉线时刻打点：重连后 gap_task 按这个起点补缺口
                self.down_since = int(time.time())
                logger.warning("[WS CLOSED] connection ended")
        return connected


async def async_main() -> None:
    config = load_config()
    setup_logging(config["data_dir"])
    # 单实例锁：曾因 start.bat 查杀失效叠加两个进程，NapCat 给每条 WS
    # 都广播同一条消息，导致消息全量双写 + get_file 并发超时。
    import msvcrt
    lock_path = Path(config["data_dir"]) / "bot.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        lock_fh = open(lock_path, "a+")
        msvcrt.locking(lock_fh.fileno(), msvcrt.LK_NBLCK, 1)
    except OSError:
        logging.error("[EXIT] another bot.py instance holds %s - exit", lock_path)
        return
    # PID 落盘：面板的停止/状态探测优先读这个文件，PowerShell 按命令行匹配只是
    # 兜底（start "" 拉起的 pythonw 命令行常为空，见 qqbot.rs resolve_pids 注释）
    pid_path = Path(config["data_dir"]) / "bot.pid"
    pid_path.write_text(str(os.getpid()))
    try:
        await _run(config)
    finally:
        # 正常退出（Ctrl+C / stop）自己删掉；被 taskkill / 崩溃时删不了，
        # 留给 viewman 的状态探测按「进程身份对不上」清掉（见 qqbot.rs probe_pids）。
        # 只删自己的：同目录另开一个 bot.py 抢不到锁会直接 return，走不到写 pid
        try:
            if pid_path.read_text().strip() == str(os.getpid()):
                pid_path.unlink()
        except OSError:
            pass


async def _run(config: dict) -> None:
    """持有单实例锁之后的全部工作：初始化、起任务、看门重连，直到退出。"""
    logging.info("[START] ws_url=%s token_set=%s data_dir=%s",
                 config.get("ws_url"), bool(config.get("ws_token")),
                 config.get("data_dir"))
    # 已持单实例锁，此处做崩溃恢复是安全的
    db = DB(Path(config["data_dir"]) / "bot.db", recover=True)
    magnets = MagnetRegistry.build(config)
    links = LinkRegistry.build(config)
    # 看门循环：断线重连只此一处（曾在 _on_close 另起线程重连，与看门循环
    # 叠加导致同进程双 WS 实例、消息双写）。连上后断线 5s 回连；一直连不上
    # 时按 RECONNECT_BASE/2/4…封顶 RECONNECT_MAX 退避。
    async with aiohttp.ClientSession() as session:
        bot = Bot(config, session, db, magnets, links)
        # 历史视频并入统一目录（DB 路径同步改写）；必须在 load_seen 之前，
        # seen 里灌进去的才是搬完后的新路径
        await bot.storage.migrate_videos(db)
        # 盘上有文件但 media 表没登记的（历史转发保存的）补齐——删后免拉的全量依据
        await bot.storage.backfill_media(db)
        # media 登记表灌进 storage.seen：删过的文件按 md5 也不再重拉。
        # 必须赶在 worker/sweeper 之前，抢跑的下载会绕过「删后免拉」
        await bot.storage.load_seen(db)
        # done 事件超保留期就清：payload 只服务重试/回填，留着 db 无限膨胀
        pruned = await db.prune_events()
        if pruned:
            logging.info("[START] 已清理 %d 条过期 done 事件", pruned)
            if pruned > 5000:    # 大扫除后回收一次页空间，日常小删不折腾
                logging.info("[START] VACUUM 回收 bot.db 空间...")
                await db.vacuum()
        # 登记表必须先于 sweeper 载入：抢跑的那一轮 seen 是空的，
        # 会把消息里已有的磁力按 seq=1,2 再写一遍（曾在 xlsx 里堆重复行）
        if magnets:
            await magnets.load()
        if links:
            await links.load()
        worker = asyncio.create_task(bot.worker())
        sweeper = asyncio.create_task(bot.sweeper())
        # 停机补拉：批量回捞断线期间漏掉的历史，会触发 QQ 侧限流
        #（Resource type 'video-url-get' is currently disabled / 消息xxx不存在），
        # 默认关闭；实时消息流不经过这里，存档不受影响。config 里置
        # backfill_enabled=true 可重新开启。
        backfill_enabled = config.get("backfill_enabled", False)
        if backfill_enabled:
            backfiller = asyncio.create_task(bot.backfill_task())
            logging.info("[START] 停机补拉已开启（backfill_enabled）")
        else:
            backfiller = None
            logging.info("[START] 停机补拉已关闭（backfill_enabled 未开启）")
        # 断线缺口补录：默认开。缺口起点取最后一次入库事件的时间，所以进程被杀
        # /重启期间的那段也用同一条路补上，不必等 backfill_enabled 的全量扫描
        gap_on = config.get("gap_backfill_enabled", True)
        if gap_on:
            bot.down_since = await db.last_seen_ts() or None
            gapper = asyncio.create_task(bot.gap_task())
            logging.info("[START] 缺口补录已开启（起点 %s）",
                         bot.down_since or "无历史缺口")
        else:
            gapper = None
            logging.info("[START] 缺口补录已关闭（gap_backfill_enabled=false）")
        # 上次运行（含崩溃）遗留的未处理事件回队重放
        backlog = await db.fetch_due_events(limit=100000)
        if backlog:
            logging.info("[BACKLOG] %d event(s) to replay", len(backlog))
        for ev in backlog:
            try:
                await bot.q.put((ev["id"], json.loads(ev["payload"])))
            except Exception as e:
                await db.fail_event(ev["id"], f"bad payload: {e}")
        try:
            while True:
                if await bot.run():
                    await asyncio.sleep(RECONNECT_BASE)
                    continue
                # 连不上（NapCat/QQ 没在跑）：指数退避 + 抖动，恢复后自动回到 5s
                await asyncio.sleep(min(RECONNECT_BASE * 2 ** bot.connect_fails,
                                        RECONNECT_MAX) + random.uniform(0, 3))
        finally:
            worker.cancel()
            sweeper.cancel()
            if backfiller is not None:
                backfiller.cancel()
            if gapper is not None:
                gapper.cancel()


def main() -> None:
    try:
        asyncio.run(async_main())
    except KeyboardInterrupt:
        logger.info("[EXIT] interrupted")


if __name__ == "__main__":
    main()
