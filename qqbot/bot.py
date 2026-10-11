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
import sys
import time
from pathlib import Path
from typing import Any

import aiohttp

from config import load_config
from db import DB
from storage import Storage, md5_hint, is_sticker
from forward import render_node, seg_text
from magnets import MagnetRegistry

logger = logging.getLogger("saver.bot")

MAX_CONCURRENT_DL = 4
SWEEP_INTERVAL = 20
REPLY_TEXT_LIMIT = 200   # 引用原文在 md 里的截断长度

# 停机补拉：连接建立/重连后拉漫游历史，把 NapCat 无订阅期间的缺口补进同一管道
BACKFILL_ACTIVE_DAYS = 14    # 只补最近活跃过的会话
BACKFILL_PAGE = 100          # 单页历史条数
BACKFILL_MAX_PAGES = 3       # 每会话最多往前翻几页
BACKFILL_KNOWN_STREAK = 30   # 从新往旧连续见到这么多条已知消息即停
BACKFILL_INTERVAL = 6 * 3600 # 进程不重启也每 6 小时补一轮（覆盖运行中断线漏的）


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
                 magnets: MagnetRegistry | None = None):
        self.config = config
        self.session = session
        self.db = db
        self.magnets = magnets
        # db 交给 storage：媒体落盘（含合并转发路径）当场登记 media 表
        self.storage = Storage(config["data_dir"], session, MAX_CONCURRENT_DL, db)
        # 全局下载限速（每秒下载数）：None/<=0 不限。配进 storage 的令牌桶
        self.storage.dl_limit = config.get("dl_limit")
        self._ws: aiohttp.ClientWebSocketResponse | None = None
        self._pending: dict[str, asyncio.Future] = {}
        self._seq = 0
        self._tasks: set[asyncio.Task] = set()
        # 事件消费队列：单消费者保证 md 文本时序
        self.q: asyncio.Queue = asyncio.Queue()
        self.magnet_kick = asyncio.Event()
        self.backfill_kick = asyncio.Event()

    def _spawn(self, coro) -> asyncio.Task:
        task = asyncio.create_task(coro)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)
        return task

    def _kick_magnets(self, text: str, mid: int | None = None) -> None:
        # 磁力链接落库即唤醒 sweeper 去登记，不等 20 秒班车：remove_magnets.py
        # 按行删 messages，晚登记的那条链接就只剩 Excel 里没人知道了。
        if not self.magnets or "magnet:" not in text.lower():
            return
        if mid is not None:
            # 回填走的是 UPDATE 老行，id 可能已低于 sweeper 游标，必须点名重扫
            self.magnets.dirty.add(mid)
        self.magnet_kick.set()

    # ---- OneBot API（请求/响应同步） ----
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
                if t == "image" and is_sticker(d):
                    # 表情包（面板/收藏/商城表情、GIF）不下载，只记占位
                    md_pending.append(("[表情包]", prefix, ts))
                    await self.db.add_message(
                        **dict(common, kind="sticker", text="[表情包]"))
                    continue
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
            # 定时班车之外，收到磁力链接会被 _kick_magnets 提前叫醒。
            # clear 放在 wait 之后：唤醒期间到达的链接不会丢，最坏是多跑一轮。
            try:
                await asyncio.wait_for(self.magnet_kick.wait(), SWEEP_INTERVAL)
            except TimeoutError:
                pass
            self.magnet_kick.clear()

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
        if self.magnets:
            try:
                await self.magnets.sync(self.db)
            except Exception as e:
                logger.error("[MAGNET] sync 异常: %s: %s", type(e).__name__, e)

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

    async def _attempt_row(self, row: dict, claimed: bool = False) -> None:
        mid = row["id"]
        # 认领失败 = sweeper 已接手这一行；两条路径同时跑会往 md 重复写一行
        if not claimed and not await self.db.claim_dl_row(mid):
            return
        kind = row.get("kind", "")
        scope = row.get("scope") or "private"
        had_text = bool((row.get("text") or "").strip())
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
                                         scope, stats=stats, report=report)
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
                                                 self.get_file_url, scope)
            else:  # image / video
                r = None
                src = row.get("src", "")
                if src:
                    r = await self.storage.save_media(
                        src, kind, scope, expect_md5=md5_hint(row.get("ref")))
                if r is None and row.get("ref"):
                    # CDN url 可能已过期：改经 NapCat file_id 取本地原件重试
                    p = await self.get_file_url(row["ref"])
                    if p:
                        r = await self.storage.save_media(
                            p, kind, scope, expect_md5=md5_hint(row["ref"]))
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

    async def run(self) -> None:
        """单次连接的生命周期：返回即代表断开，重连由外层看门循环负责。"""
        token = self.config.get("ws_token", "")
        headers = {"Authorization": f"Bearer {token}"} if token else {}
        try:
            # heartbeat=30 发协议层 ping，NapCat 不回 pong 即判超时断开，
            # 解决 TCP 半开假死时静默失联的问题
            async with self.session.ws_connect(self.config["ws_url"],
                                               headers=headers, heartbeat=30) as ws:
                self._ws = ws
                logger.info("[WS CONNECTED] Listening for messages (url=%s)",
                            self.config.get("ws_url"))
                self.backfill_kick.set()   # 连接建立即唤醒停机补拉
                async for msg in ws:
                    if msg.type is aiohttp.WSMsgType.TEXT:
                        self._on_raw(msg.data)
                    elif msg.type in (aiohttp.WSMsgType.CLOSE, aiohttp.WSMsgType.CLOSING,
                                      aiohttp.WSMsgType.CLOSED, aiohttp.WSMsgType.ERROR):
                        break
        except Exception as e:
            logger.error("[WS ERROR] %s", e)
        finally:
            self._ws = None
            self._fail_pending("websocket closed")
            logger.warning("[WS CLOSED] connection ended")


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
    logging.info("[START] ws_url=%s token_set=%s data_dir=%s",
                 config.get("ws_url"), bool(config.get("ws_token")),
                 config.get("data_dir"))
    # 已持单实例锁，此处做崩溃恢复是安全的
    db = DB(Path(config["data_dir"]) / "bot.db", recover=True)
    magnets = MagnetRegistry.build(config)
    # 看门循环：连接断开/异常后 5s 重连。重连只此一处（曾在 _on_close 另起
    # 线程重连，与看门循环叠加导致同进程双 WS 实例、消息双写）。
    async with aiohttp.ClientSession() as session:
        bot = Bot(config, session, db, magnets)
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
                await bot.run()
                await asyncio.sleep(5)
        finally:
            worker.cancel()
            sweeper.cancel()
            if backfiller is not None:
                backfiller.cancel()


def main() -> None:
    try:
        asyncio.run(async_main())
    except KeyboardInterrupt:
        logger.info("[EXIT] interrupted")


if __name__ == "__main__":
    main()
