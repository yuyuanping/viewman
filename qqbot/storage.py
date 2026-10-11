"""落盘层（asyncio）：文本追加、图片/视频/文件下载与 MD5 全量去重、扩展名推断。
图片按 private/group 分流；视频统一入 videos/，不分来源。
文件被手工删除也不重拉：media 登记表灌入 self.seen 作为持久记忆。"""
import asyncio
import hashlib
import logging
import os
import re
import shutil
import time
from datetime import datetime
from pathlib import Path
from typing import Callable, Coroutine, Optional
from uuid import uuid4

import aiohttp

logger = logging.getLogger("saver.storage")

# Content-Type -> 扩展名
_EXT_BY_CT = {
    "image/jpeg": "jpg", "image/png": "png", "image/gif": "gif",
    "image/bmp": "bmp", "image/webp": "webp", "image/avif": "avif",
    "video/mp4": "mp4", "video/webm": "webm", "video/quicktime": "mov",
}
_VALID_IMG = ("jpg", "jpeg", "png", "gif", "bmp", "webp")
_VALID_VID = ("mp4", "webm", "mov", "mkv", "avi")
# 文件类（save_file）允许保留的扩展名白名单：消息里的文件名可伪造
# （发个 x.exe 就存成 .exe），白名单外按 Content-Type 推断、再退 .bin
_VALID_FILE = _VALID_IMG + _VALID_VID + (
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "csv",
    "zip", "7z", "rar", "tar", "gz", "mp3", "flac", "wav", "ogg", "m4a",
    "srt", "ass", "epub", "mobi", "azw3", "html", "xml", "json", "log",
)

# 各类下载的总超时（秒）
_TIMEOUT_BY_KIND = {"image": 15, "video": 30, "file": 60}


def _ext_from_ct(ct: str, url: str, kind: str) -> str:
    """优先 Content-Type，回退 URL 后缀，再回退默认值。"""
    if ct in _EXT_BY_CT:
        return _EXT_BY_CT[ct]
    url_ext = url.split("?")[0].split(".")[-1].lower()
    valid = _VALID_IMG if kind == "image" else _VALID_VID
    return url_ext if url_ext in valid else ("jpg" if kind == "image" else "mp4")


def _hash_file(path: Path) -> tuple[str, int]:
    """分块读本地文件算 md5——NapCat 本地原件可能是几百 MB 的视频，不能整读。"""
    digest = hashlib.md5()
    size = 0
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


# NapCat 事件里图片/视频段的 file 字段形如 HASH.jpg，HASH 即原件 md5
# （DB 实测 ~96% 吻合，其余是 GUID 之类的别名）。认不出返回 None，照常下载。
_MD5_RE = re.compile(r"[0-9a-fA-F]{32}")


def md5_hint(ref: Optional[str]) -> Optional[str]:
    """从 file 字段名里提取原件 md5，供下载前查盘免重拉。"""
    stem = Path(ref or "").stem
    return stem.lower() if _MD5_RE.fullmatch(stem) else None


def is_sticker(d: dict) -> bool:
    """识别表情包图片（只记占位不下载）：商城表情（emoji_id）、面板/收藏
    表情（sub_type=1）、以及一切 GIF——群里 GIF 实际全是表情包（实测 651
    张里 34 张 GIF 跨各种 sub_type，动图照片极罕见）。普通照片/截图不受影响。"""
    if not d:
        return False
    if d.get("emoji_id") or str(d.get("sub_type")) == "1":
        return True
    ext = str(d.get("file") or "").rsplit(".", 1)[-1].lower()
    return ext == "gif"


class Storage:
    def __init__(self, data_dir, session: aiohttp.ClientSession,
                 max_concurrent_dl: int = 4, db=None):
        self.session = session
        self.db = db   # 可选：登记 media 表用（测试传 None 跳过登记）
        self.data_dir = Path(data_dir)
        self.msg_dir = self.data_dir / "messages"
        # 图片按来源分流：private/ 与 group/
        self.priv_img = self.data_dir / "private" / "images"
        self.priv_vid = self.data_dir / "private" / "videos"
        self.priv_file = self.data_dir / "private" / "files"
        self.grp_img = self.data_dir / "group" / "images"
        self.grp_vid = self.data_dir / "group" / "videos"
        self.grp_file = self.data_dir / "group" / "files"
        # 视频统一目录：不分私聊/群，方便整目录管理（删除/盘点）
        self.vid_all = self.data_dir / "videos"
        for d in (self.msg_dir, self.priv_img, self.priv_vid, self.priv_file,
                  self.grp_img, self.grp_vid, self.grp_file, self.vid_all):
            d.mkdir(parents=True, exist_ok=True)
        # 上次崩溃残留的 ~dl*.part 临时件：启动时单实例、无下载在途，直接清
        for d in (self.priv_img, self.priv_vid, self.grp_img, self.grp_vid,
                  self.priv_file, self.grp_file, self.vid_all):
            for p in d.glob("~dl*.part"):
                p.unlink(missing_ok=True)
        # messages 日记句柄常开、按日轮换（append_text 每段消息都写）
        self._md_fh = None
        self._md_day = ""
        # 并发下载上限：再多并发任务也只占这么多连接，慢下载靠协程挂起不占线程
        self._dl_sem = asyncio.Semaphore(max_concurrent_dl)
        # 全局限速（令牌桶）：每秒最多下载 dl_limit 个媒体，超出排队等令牌。
        # 默认 None=不限速；腾讯对短时高频资源拉取有风控熔断
        # （Resource type disabled, success N / failure M），限速让流量更像真人。
        self.dl_limit: Optional[float] = None
        self._tokens = 1.0
        self._last_refill = time.monotonic()
        self._dl_limit_lock = asyncio.Lock()
        # 媒体下载熔断开关：被 bot.py 在展开高危合并转发前置 True，
        # render_node 读它决定是否只产占位文本不下媒体
        self.block_media = False
        # 已保存过的 md5 -> (path, bytes)：load_seen() 启动时从 media 表灌入，
        # 之后每存一个媒体同步记一份。文件被手工删除也仍算「已保存」，
        # 同内容再出现时免下载——这是删后不重拉的持久记忆。
        self.seen: dict[str, tuple[str, int]] = {}

    async def load_seen(self, db) -> None:
        """把 media 登记表灌进 self.seen；必须赶在 worker/sweeper 启动之前调用。"""
        rows = await db._run(
            lambda c: c.execute("SELECT md5, path, bytes FROM media").fetchall())
        self.seen = {r["md5"]: (r["path"], r["bytes"] or 0) for r in rows}
        logger.info("[STORAGE] seen 灌入 %d 个 md5", len(self.seen))

    async def _register(self, digest: str, kind: str, scope: str,
                        path: str, size: int) -> None:
        """每次成功落盘（含去重复用）都补进 media 表。合并转发里保存的媒体
        没有独立 messages 行，不在这一层登记的话，seen 灌入、--file 检索、
        删后免拉就都漏掉它们。INSERT OR IGNORE，重复调用无害。"""
        if self.db is None:
            return
        try:
            await self.db.upsert_media(digest, kind, scope, path, size)
        except Exception as e:
            logger.error("[STORAGE] media 登记失败 %s: %s", digest, e)

    async def backfill_media(self, db) -> None:
        """盘上以 md5 命名但 media 表里没有的文件补登记（历史转发路径保存的、
        手工恢复的文件等）。文件名即内容 md5，无需重新哈希。
        须在 load_seen 之前跑，灌进 seen 的才是全量。"""
        def _collect():
            out = []
            for base, kind, scope in (
                    (self.vid_all, "video", ""),
                    (self.priv_img, "image", "private"),
                    (self.grp_img, "image", "group"),
                    (self.priv_vid, "video", "private"),
                    (self.grp_vid, "video", "group"),
                    (self.priv_file, "file", "private"),
                    (self.grp_file, "file", "group")):
                for p in base.iterdir():
                    if not p.is_file() or not _MD5_RE.fullmatch(p.stem):
                        continue
                    try:
                        st = p.stat()
                    except OSError:
                        continue
                    out.append((p.stem, kind, scope, str(p), st.st_size,
                                int(st.st_mtime)))
            return out

        items = await asyncio.to_thread(_collect)
        if not items:
            return

        def _ins(c):
            c.executemany(
                "INSERT OR IGNORE INTO media(md5,kind,scope,path,bytes,first_seen) "
                "VALUES(?,?,?,?,?,?)", items)
            return c.execute("SELECT COUNT(*) FROM media").fetchone()[0]

        total = await db._run(_ins)
        logger.info("[STORAGE] 盘上补登记完成，media 表现有 %d 项", total)

    def _scope_dir(self, base_priv: Path, base_grp: Path, scope: str) -> Path:
        return base_grp if scope == "group" else base_priv

    def _media_base(self, kind: str, scope: str) -> Path:
        if kind == "image":
            return self._scope_dir(self.priv_img, self.grp_img, scope)
        return self.vid_all        # 视频统一目录，不分来源

    def _existing(self, digest: str, kind: str) -> Optional[Path]:
        """该 md5 的已落盘文件。图片查 private/group 两侧；视频查统一目录
        外加 private|group/videos 两个历史位置（迁移搬空前仍可能非空）。"""
        bases = ((self.priv_img, self.grp_img) if kind == "image"
                 else (self.vid_all, self.priv_vid, self.grp_vid))
        for base in bases:
            hit = next((p for p in base.glob(f"{digest}.*") if p.is_file()), None)
            if hit is not None:
                return hit
        return None

    async def migrate_videos(self, db) -> None:
        """历史视频并入统一目录：private|group/videos 里的文件搬进 videos/
        （同盘 rename，秒完成），DB 里指向老目录的 path 一并改写。
        幂等——老目录空了就是空转。须在 load_seen 之前跑，seen 才拿到新路径。"""
        moved = dropped = 0
        for f in list(self.priv_vid.glob("*")) + list(self.grp_vid.glob("*")):
            if not f.is_file():
                continue
            dest = self.vid_all / f.name
            if dest.exists():
                # md5 命名下同名即同内容，源是冗余副本
                await asyncio.to_thread(f.unlink)
                dropped += 1
            else:
                await asyncio.to_thread(f.rename, dest)
                moved += 1
        old = {str(self.priv_vid).lower(), str(self.grp_vid).lower()}

        def _repoint(c):
            n = 0
            for table, key in (("media", "md5"), ("messages", "id")):
                rows = c.execute(
                    f"SELECT {key} AS k, path FROM {table} "
                    f"WHERE kind='video' AND path IS NOT NULL").fetchall()
                ups = [(str(self.vid_all / Path(r["path"]).name), r["k"])
                       for r in rows
                       if str(Path(r["path"]).parent).lower() in old]
                if ups:
                    c.executemany(f"UPDATE {table} SET path=? WHERE {key}=?", ups)
                    n += len(ups)
            return n

        n = await db._run(_repoint)
        if moved or dropped or n:
            logger.info("[STORAGE] 视频统一目录：搬入 %d 个、冗余副本删 %d 个，"
                        "DB 路径改写 %d 条 -> %s", moved, dropped, n, self.vid_all)

    # ---- 路径 ----
    def _today(self) -> str:
        return datetime.now().strftime("%Y-%m-%d")

    def msg_file(self) -> Path:
        return self.msg_dir / f"{self._today()}.md"

    # ---- 文本 ----
    def append_text(self, text: str, prefix: str = "", ts: Optional[int] = None) -> None:
        # 句柄常开按日轮换：省去每段消息一次 open/close；flush 保证崩溃不丢
        # ts 传消息真实 unix 时间戳（BACKFILL 补录的历史消息必须用它定日，
        # 否则历史消息会写进处理时刻所在的当天 md —— 曾在停机补拉后把
        # 9.30/10.01 的消息全写进 10.02.md）；ts=None 时沿用 now()（实时消息）。
        now = datetime.now() if ts is None else datetime.fromtimestamp(ts)
        today = now.strftime("%Y-%m-%d")
        if self._md_fh is None or self._md_day != today:
            if self._md_fh is not None:
                self._md_fh.close()
            self._md_fh = open(self.msg_dir / f"{today}.md", "a", encoding="utf-8")
            self._md_day = today
        line = f"**{now:%H:%M:%S}** {prefix}{text}\n"
        self._md_fh.write(line)
        self._md_fh.flush()
        logger.debug("[SAVED] %s", line.strip())

    # ---- 下载 ----
    async def _wait_token(self) -> None:
        """按全局令牌桶限速：dl_limit 为每秒允许的下载数，None/<=0 不限。"""
        limit = self.dl_limit
        if not limit or limit <= 0:
            return
        async with self._dl_limit_lock:
            now = time.monotonic()
            self._tokens = min(self._tokens + (now - self._last_refill) * limit, limit)
            self._last_refill = now
            if self._tokens >= 1:
                self._tokens -= 1
                return
            deficit = (1 - self._tokens) / limit
        # 令牌不足：锁外睡 deficit 秒，让别人也能拿令牌
        await asyncio.sleep(deficit)
        async with self._dl_limit_lock:
            now = time.monotonic()
            self._tokens = min(self._tokens + (now - self._last_refill) * limit, limit)
            self._last_refill = now
            self._tokens = max(self._tokens - 1, 0)

    async def _stream_to(self, url: str, kind: str,
                         tmp: Path) -> Optional[tuple[str, int, str]]:
        """流式下载 url 到 tmp：1MB 块边写边算 md5，内存占用与文件大小无关。
        失败重试 1 次（并清掉残件）；成功返回 (md5hex, bytes, Content-Type)。"""
        timeout = aiohttp.ClientTimeout(total=_TIMEOUT_BY_KIND.get(kind, 30))
        for attempt in (1, 2):
            try:
                digest = hashlib.md5()
                size = 0
                async with self.session.get(url, timeout=timeout) as resp:
                    resp.raise_for_status()
                    ct = resp.headers.get("Content-Type", "").split(";")[0].strip().lower()
                    with open(tmp, "wb") as f:
                        async for chunk in resp.content.iter_chunked(1 << 20):
                            await asyncio.to_thread(f.write, chunk)
                            digest.update(chunk)
                            size += len(chunk)
                return digest.hexdigest(), size, ct
            except Exception as e:
                logger.error("[ERROR] download %s attempt %d/2 failed: %s",
                             kind, attempt, e)
        tmp.unlink(missing_ok=True)
        return None

    # ---- 图片/视频 ----
    async def save_media(self, url: str, kind: str, scope: str = "private",
                         expect_md5: Optional[str] = None) -> Optional[dict]:
        """下载图片或视频，按 MD5 命名去重。图片按 scope 分流 private/group；
        视频统一入 videos/（scope 只对图片有意义）。
        支持两类来源：http(s) 直链；本地绝对路径（NapCat 有时直接给 nt_qq
        Ori 原件路径，走 HTTP 会失败，须直接读文件）。
        expect_md5 是事件自带的原件 md5（md5_hint）。命中即免拉、不复制，
        DB 行直接指向现有文件。判定来源有二：盘上目录里已有该 md5 的文件，
        或 media 登记表里有它（文件已被手工删除也算保存过）。
        成功返回 {"placeholder","md5","path","bytes"}，失败返回 None。"""
        if not url:
            logger.error("[ERROR] empty url for %s", kind)
            return None
        ext = "jpg" if kind == "image" else "mp4"
        valid = _VALID_IMG if kind == "image" else _VALID_VID
        await self._wait_token()
        async with self._dl_sem:
            if expect_md5:
                hit = self.seen.get(expect_md5)
                if hit is None:
                    hit_path = await asyncio.to_thread(self._existing, expect_md5, kind)
                    if hit_path is not None:
                        hit = (str(hit_path),
                               (await asyncio.to_thread(hit_path.stat)).st_size)
                if hit is not None:
                    self.seen[expect_md5] = hit
                    path_s, size = hit
                    logger.info("[%s DEDUP] %s (md5 已登记，免下载)",
                                kind.upper(), Path(path_s).name)
                    await self._register(expect_md5, kind, scope, path_s, size)
                    return {"placeholder": f"[{'图片' if kind == 'image' else '视频'}] "
                                           f"{Path(path_s).name}",
                            "md5": expect_md5, "path": path_s, "bytes": size,
                            "dedup": True}
            # 下载/读取：流式边写边算 md5，不再把整个文件读进内存
            tmp = None
            if url.startswith(("http://", "https://")):
                tmp = self._media_base(kind, scope) / f"~dl{uuid4().hex}.part"
                got = await self._stream_to(url, kind, tmp)
                if got is None:
                    return None
                digest, size, ct = got
                ext = _ext_from_ct(ct, url, kind)
            else:
                p = Path(url)
                if not p.is_file():
                    logger.error("[ERROR] local path not found for %s: %s", kind, url)
                    return None
                digest, size = await asyncio.to_thread(_hash_file, p)
                e = p.suffix.lstrip(".").lower()
                ext = e if e in valid else ext
            path = self._media_base(kind, scope) / f"{digest}.{ext}"
            dup = path.exists()
            if not dup:
                if tmp is not None:
                    await asyncio.to_thread(os.replace, tmp, path)
                else:
                    # 本地原件（NapCat 缓存）：复制进统一目录，不动原件。
                    # 先写 .part 再 replace：中断不会留下顶着 md5 名的半文件
                    # （会被 dup 检查复用、被 backfill_media 洗白成有效登记）
                    part = base / f"~dl{uuid4().hex}.part"
                    try:
                        await asyncio.to_thread(shutil.copyfile, url, part)
                    except Exception:
                        part.unlink(missing_ok=True)
                        raise
                    await asyncio.to_thread(os.replace, part, path)
            elif tmp is not None:    # 下载完才发现内容重复：丢临时件复用现有文件
                await asyncio.to_thread(tmp.unlink)
            self.seen[digest] = (str(path), size)
        await self._register(digest, kind, scope, str(path), size)
        if dup:
            logger.info("[%s DEDUP] %s (内容重复，复用现有文件)", kind.upper(), path.name)
        else:
            logger.info("[%s SAVED] %s", kind.upper(), path)
        return {"placeholder": f"[{'图片' if kind == 'image' else '视频'}] {path.name}",
                "md5": digest, "path": str(path), "bytes": size, "dedup": dup}

    # ---- 文件 ----
    async def save_file(self, file_id_or_url: str, filename: str,
                        get_url: Callable[[str], Coroutine[None, None, Optional[str]]],
                        scope: str = "private") -> Optional[dict]:
        """下载文件，按 MD5 命名去重。三种来源：http(s) 直链；本地绝对路径；
        文件 ID 经 get_url 解析（NapCat 通常给本地路径）——解析是慢 API 调用，
        放在下载并发额度之外，NapCat 卡住时不会占死下载槽。传输流式落盘，
        内存与文件大小无关。
        成功返回 {"placeholder","md5","path","bytes"}，失败返回 None。
        """
        if not file_id_or_url:
            logger.warning("[FILE SKIP] %s - empty file id", filename)
            return None
        try:
            src = file_id_or_url
            if not src.startswith(("http://", "https://")) \
                    and not Path(src).is_absolute():
                src = await get_url(src)
                if not src:
                    logger.warning("[FILE SKIP] %s - cannot resolve file source",
                                   filename)
                    return None
        except Exception as e:
            logger.error("[ERROR] Failed to resolve file %s: %s", filename, e)
            return None
        base = self._scope_dir(self.priv_file, self.grp_file, scope)
        await self._wait_token()
        async with self._dl_sem:
            tmp = None
            ct = ""
            try:
                if src.startswith(("http://", "https://")):
                    tmp = base / f"~dl{uuid4().hex}.part"
                    got = await self._stream_to(src, "file", tmp)
                    if got is None:
                        return None
                    digest, size, ct = got
                else:
                    if not Path(src).is_file():
                        logger.error("[ERROR] local file not found: %s", src)
                        return None
                    digest, size = await asyncio.to_thread(_hash_file, Path(src))
                sfx = Path(filename).suffix.lstrip(".").lower()
                if sfx in _VALID_FILE:
                    ext = sfx
                elif ct in _EXT_BY_CT:
                    ext = _EXT_BY_CT[ct]
                else:
                    ext = "bin"
                path = base / f"{digest}.{ext}"
                dup = path.exists()
                if not dup:
                    if tmp is not None:
                        await asyncio.to_thread(os.replace, tmp, path)
                    else:
                        # 本地原件：复制进来，不动 NapCat 缓存。
                        # 先写 .part 再 replace，理由同 save_media：中断不留半文件
                        part = base / f"~dl{uuid4().hex}.part"
                        try:
                            await asyncio.to_thread(shutil.copyfile, src, part)
                        except Exception:
                            part.unlink(missing_ok=True)
                            raise
                        await asyncio.to_thread(os.replace, part, path)
                elif tmp is not None:   # 内容重复：丢临时件复用现有文件
                    await asyncio.to_thread(tmp.unlink)
            except Exception as e:
                logger.error("[ERROR] Failed to save file %s: %s", filename, e)
                if tmp is not None:
                    tmp.unlink(missing_ok=True)
                return None
        self.seen[digest] = (str(path), size)
        await self._register(digest, "file", scope, str(path), size)
        logger.info("[FILE SAVED] %s", path)
        return {"placeholder": f"[文件] {filename}",
                "md5": digest, "path": str(path), "bytes": size, "dedup": dup}
