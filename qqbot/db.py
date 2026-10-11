"""SQLite 持久层：事件队列（消息永不丢的落库点）+ 消息索引 + FTS5 全文检索。

设计：
- events  每条原始事件先入此表再处理（WAL，单机写入微秒级）。
          状态机 new -> busy -> done；失败 error（指数退避重试）-> dead（穷尽）。
          进程崩溃遗留的 busy 在启动时回队。
- messages 每消息段一行（text/image/video/file/forward/reply/other）。
          媒体、转发展开、引用回查带 dl 重试队列：pending -> busy -> ok / dead，
          与事件流解耦——下载失败不再丢消息，稍后自动重做。dead 不是终态：
          前 8 次约 4 小时内指数退避，之后每天复检一次，共再 7 次（见 DL_*）。
- media   MD5 内容寻址登记表（全量去重/删后免拉的持久依据 + 盘点用）。
- messages_fts  trigram 分词，中文子串检索免分词（<3 字由调用方回退 LIKE）。

所有 sqlite3 操作经单锁在 to_thread 工作线程执行，asyncio 侧安全共享。
"""
import asyncio
import sqlite3
import threading
import time
from pathlib import Path

MAX_EVENT_TRIES = 6   # 事件重试上限，穷尽转 dead
MAX_DL_TRIES = 8     # 单条媒体/转发下载的快重试上限（约 4 小时窗口）
DL_LATE_TRIES = 15   # 之后每天复检一次，跑满这个总次数才不再碰（约 7 天）
DL_LATE_INTERVAL = 24 * 3600
EVENT_RETENTION_DAYS = 30   # done 事件的 payload 只服务重试/回填，30 天后清掉

_SCHEMA = """
CREATE TABLE IF NOT EXISTS events(
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  received_at REAL NOT NULL,
  status      TEXT NOT NULL DEFAULT 'new',   -- new/busy/done/error/dead
  tries       INTEGER NOT NULL DEFAULT 0,
  next_try    REAL NOT NULL DEFAULT 0,
  err         TEXT,
  payload     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_due ON events(status, next_try);

CREATE TABLE IF NOT EXISTS messages(
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id    INTEGER REFERENCES events(id),
  msg_id      TEXT,          -- OneBot 消息 id（停机补拉去重的精确依据）
  scope       TEXT,          -- private/group
  peer_id     INTEGER,       -- 群号或对方 QQ 号
  sender_id   INTEGER,
  sender_nick TEXT,
  ts          INTEGER,       -- 消息时间（Unix 秒）
  seg_index   INTEGER,
  kind        TEXT,          -- text/image/video/file/forward/other
  text        TEXT,
  name        TEXT,          -- 文件原始名
  src         TEXT,          -- 媒体 url 或本地路径
  ref         TEXT,          -- file_id / 合并转发 id
  md5         TEXT,
  path        TEXT,
  bytes       INTEGER,
  dl          TEXT NOT NULL DEFAULT 'none',  -- none/pending/busy/ok/dead
  dedup       INTEGER NOT NULL DEFAULT 0,   -- 媒体=复用现有文件(0/1)；转发=内部跳过的重复媒体数
  parent      INTEGER REFERENCES messages(id),  -- 转发明细行指向所属转发行
  tries       INTEGER NOT NULL DEFAULT 0,
  next_try    REAL NOT NULL DEFAULT 0,
  err         TEXT
);
CREATE INDEX IF NOT EXISTS messages_due ON messages(dl, next_try);
CREATE INDEX IF NOT EXISTS messages_ts ON messages(ts);

CREATE TABLE IF NOT EXISTS media(
  md5        TEXT PRIMARY KEY,
  kind       TEXT,
  scope      TEXT,
  path       TEXT,
  bytes      INTEGER,
  first_seen INTEGER
);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts
  USING fts5(text, content=messages, content_rowid=id, tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, text) VALUES('delete', old.id, old.text);
END;
CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, text) VALUES('delete', old.id, old.text);
  INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
END;
"""


class DB:
    def __init__(self, path, recover: bool = False):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(str(self.path), check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._lock = threading.Lock()
        with self._lock:
            self._conn.execute("PRAGMA journal_mode=WAL")
            self._conn.executescript(_SCHEMA)
            # 老库补列（msg_id 服务停机补拉去重）；索引必须在补列之后建，
            # 否则 executescript 里的 CREATE INDEX 会在老库上直接炸掉启动
            cols = {r[1] for r in self._conn.execute("PRAGMA table_info(messages)")}
            if "msg_id" not in cols:
                self._conn.execute("ALTER TABLE messages ADD COLUMN msg_id TEXT")
            if "dedup" not in cols:
                self._conn.execute(
                    "ALTER TABLE messages ADD COLUMN dedup INTEGER NOT NULL DEFAULT 0")
            if "parent" not in cols:
                self._conn.execute("ALTER TABLE messages ADD COLUMN parent INTEGER")
            self._conn.execute(
                "CREATE INDEX IF NOT EXISTS messages_msgid ON messages(msg_id)")
            # 崩溃恢复：上一次进程遗留的 busy 一律回队。只有 bot 主进程
            # （recover=True，已先抢到单实例锁）能做——search/replay 等命令行
            # 工具若也做恢复，会把运行中 bot 正在处理的事件打回队列造成重复处理
            if recover:
                self._conn.execute("UPDATE events SET status='new' WHERE status='busy'")
                self._conn.execute("UPDATE messages SET dl='pending' WHERE dl='busy'")
            self._conn.commit()

    # ---- 异步包装 ----
    async def _run(self, fn):
        def _do():
            with self._lock:
                r = fn(self._conn)
                self._conn.commit()
                return r
        return await asyncio.to_thread(_do)

    # ---- events ----
    async def add_event(self, payload: str) -> int:
        return await self._run(lambda c: c.execute(
            "INSERT INTO events(received_at, payload) VALUES(?, ?)",
            (time.time(), payload)).lastrowid)

    async def claim_event(self, eid: int) -> bool:
        """new/error -> busy；返回 False 表示已被他人认领（防双处理）。"""
        return await self._run(lambda c: c.execute(
            "UPDATE events SET status='busy' WHERE id=? AND status IN ('new','error')",
            (eid,)).rowcount > 0)

    async def finish_event(self, eid: int) -> None:
        await self._run(lambda c: c.execute(
            "UPDATE events SET status='done' WHERE id=?", (eid,)))

    async def fail_event(self, eid: int, err: str) -> str:
        def _do(c):
            row = c.execute("SELECT tries FROM events WHERE id=?", (eid,)).fetchone()
            tries = (row["tries"] if row else 0) + 1
            status = "dead" if tries >= MAX_EVENT_TRIES else "error"
            nxt = time.time() + min(15 * 2 ** tries, 3600)
            c.execute("UPDATE events SET tries=?, status=?, next_try=?, err=? WHERE id=?",
                      (tries, status, nxt, err[:500], eid))
            return status
        return await self._run(_do)

    async def fetch_due_events(self, limit: int = 50) -> list[dict]:
        now = time.time()
        rows = await self._run(lambda c: c.execute(
            "SELECT id, payload, tries FROM events "
            "WHERE status='new' OR (status='error' AND next_try<=?) "
            "ORDER BY id LIMIT ?", (now, limit)).fetchall())
        return [dict(r) for r in rows]

    async def event_stats(self) -> dict:
        rows = await self._run(lambda c: c.execute(
            "SELECT status, COUNT(*) n FROM events GROUP BY status").fetchall())
        return {r["status"]: r["n"] for r in rows}

    # ---- messages ----
    async def add_message(self, **kw) -> int:
        cols = ",".join(kw)
        marks = ",".join("?" * len(kw))
        vals = tuple(kw.values())

        def _do(c):
            return c.execute(
                f"INSERT INTO messages({cols}) VALUES({marks})", vals).lastrowid
        return await self._run(_do)

    async def claim_pending_dl(self, limit: int = 8) -> list[dict]:
        """认领到期的 dl 重试行；返回认领到的完整行。

        dl='dead' 且还没跑满复检轮次的行也算到期：快重试只有约 4 小时窗口，
        而 QQ 往往在那之后（用户点开播放过）才把原件下到本地。
        两支都用 messages_due(dl, next_try) 索引，别写成跨 dl 的 OR。
        """
        now = time.time()

        def _do(c):
            rows = c.execute(
                "SELECT * FROM messages WHERE dl='pending' AND next_try<=?"
                " UNION ALL "
                "SELECT * FROM messages WHERE dl='dead' AND next_try<=? AND tries<? "
                "ORDER BY id LIMIT ?",
                (now, now, DL_LATE_TRIES, limit)).fetchall()
            ids = [r["id"] for r in rows]
            if ids:
                c.execute(
                    f"UPDATE messages SET dl='busy' WHERE id IN "
                    f"({','.join('?' * len(ids))})", ids)
            return [dict(r) for r in rows]
        return await self._run(_do)

    async def claim_dl_row(self, mid: int) -> bool:
        """按 id 认领单行（pending -> busy）。False 表示他人已在处理。"""
        return await self._run(lambda c: c.execute(
            "UPDATE messages SET dl='busy' WHERE id=? AND dl='pending'",
            (mid,)).rowcount > 0)

    async def mark_dl_ok(self, mid: int, md5: str, path: str,
                         size: int, dedup: bool = False) -> None:
        await self._run(lambda c: c.execute(
            "UPDATE messages SET dl='ok', md5=?, path=?, bytes=?, dedup=?, err=NULL WHERE id=?",
            (md5, path, size, int(bool(dedup)), mid)))

    async def set_message_text(self, mid: int, text: str, dedup: int = 0) -> None:
        await self._run(lambda c: c.execute(
            "UPDATE messages SET text=?, dedup=? WHERE id=?", (text, dedup, mid)))

    async def clear_event_messages(self, eid: int) -> None:
        """事件重放前清掉上次尝试已写的 messages 行（含转发明细行），
        防止重放把同一段内容重复入库。单条 DELETE，天然原子。"""
        await self._run(lambda c: c.execute(
            "DELETE FROM messages WHERE event_id=?", (eid,)))

    async def bump_dl(self, mid: int, err: str) -> str:
        """失败退避 30*2^tries 秒（封顶 30 分钟），跑满 MAX_DL_TRIES 后转 dead
        并改成每天复检一次；复检满 DL_LATE_TRIES-MAX_DL_TRIES 轮才真正不再碰。"""
        def _do(c):
            row = c.execute("SELECT tries FROM messages WHERE id=?", (mid,)).fetchone()
            tries = (row["tries"] if row else 0) + 1
            late = tries >= MAX_DL_TRIES
            dl = "dead" if late else "pending"
            nxt = time.time() + (DL_LATE_INTERVAL if late
                                 else min(30 * 2 ** tries, 1800))
            c.execute("UPDATE messages SET dl=?, tries=?, next_try=?, err=? WHERE id=?",
                      (dl, tries, nxt, err[:500], mid))
            return dl
        return await self._run(_do)

    async def upsert_media(self, md5: str, kind: str, scope: str,
                           path: str, size: int) -> None:
        await self._run(lambda c: c.execute(
            "INSERT OR IGNORE INTO media(md5, kind, scope, path, bytes, first_seen) "
            "VALUES(?,?,?,?,?,?)", (md5, kind, scope, path, size, int(time.time()))))

    async def dl_stats(self) -> dict:
        rows = await self._run(lambda c: c.execute(
            "SELECT dl, COUNT(*) n FROM messages GROUP BY dl").fetchall())
        return {r["dl"]: r["n"] for r in rows}

    # ---- 检索 ----
    async def search(self, query: str, limit: int = 50) -> list[dict]:
        """trigram FTS（>=3 字符走索引），短查询回退 LIKE。"""
        def _do(c):
            if len(query) >= 3:
                phrase = '"' + query.replace('"', '""') + '"'
                try:
                    return c.execute(
                        "SELECT m.* FROM messages_fts f "
                        "JOIN messages m ON m.id = f.rowid "
                        "WHERE messages_fts MATCH ? ORDER BY m.ts DESC LIMIT ?",
                        (phrase, limit)).fetchall()
                except sqlite3.OperationalError:
                    pass
            # 先转义反斜杠本身，再转义 LIKE 通配符，否则含 \ 的关键词会串义
            like = "%" + query.replace("\\", "\\\\").replace("%", r"\%") \
                              .replace("_", r"\_") + "%"
            return c.execute(
                "SELECT * FROM messages WHERE text LIKE ? ESCAPE '\\' "
                "ORDER BY ts DESC LIMIT ?", (like, limit)).fetchall()
        rows = await self._run(_do)
        return [dict(r) for r in rows]

    async def stats(self) -> dict:
        return {"events": await self.event_stats(), "dl": await self.dl_stats()}

    async def msg_known(self, msg_id: str, scope: str, peer_id,
                        sender_id, ts: int) -> bool:
        """停机补拉去重：msg_id 精确命中，或老数据按 会话+发送人+秒 兜底。
        （同一秒同人多条消息会被误判为已知——概率远低于重复存档的代价。）"""
        def _do(c):
            if msg_id and c.execute(
                    "SELECT 1 FROM messages WHERE msg_id=? LIMIT 1",
                    (msg_id,)).fetchone():
                return True
            return c.execute(
                "SELECT 1 FROM messages WHERE scope=? AND peer_id=? AND "
                "sender_id=? AND ts=? LIMIT 1",
                (scope, peer_id, sender_id, ts)).fetchone() is not None
        return await self._run(_do)

    async def active_chats(self, days: int) -> list[dict]:
        """最近 days 天活跃过的会话（补拉范围），按最近消息时间倒序。"""
        rows = await self._run(lambda c: c.execute(
            "SELECT scope, peer_id, MAX(ts) AS mts FROM messages "
            "WHERE ts>=? AND peer_id IS NOT NULL GROUP BY scope, peer_id "
            "ORDER BY mts DESC",
            (time.time() - days * 86400,)).fetchall())
        return [dict(r) for r in rows]

    async def prune_events(self) -> int:
        """删除超过保留期的 done 事件（payload 只在重试/回填时有用，留着
        只会让 bot.db 无限变大）。返回删除行数。"""
        cutoff = time.time() - EVENT_RETENTION_DAYS * 86400

        def _do(c):
            return c.execute(
                "DELETE FROM events WHERE status='done' AND received_at<?",
                (cutoff,)).rowcount
        return await self._run(_do)

    async def vacuum(self) -> None:
        """回收删除后的空闲页（只在启动且大扫除后调用一次，运行期靠 WAL 复用）。"""
        await self._run(lambda c: c.execute("VACUUM"))
