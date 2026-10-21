"""网址链接登记表增量维护：新出现的 http(s) 链接与 xlsx 主表按链接比对，缺则补一行。

与 magnets.py 同一套机制（轮询 messages、游标水位、写表前按表内现值复查），
差别只有两点：
  - 登记表不存在时自动建表（磁力表由用户自己的 extract_magnets.py 生成，这里
    没有现成脚本，bot 自己当表的主人）；
  - 去重键是链接本身：大小写不敏感、去掉收尾标点、&amp; 还原成 &，原样入表。

磁力链接不进这张表——它归 magnets.py / 磁力链接汇总.xlsx 管，两张表互不重复。
"""
import asyncio
import logging
import os
import re
import time
from html import unescape
from pathlib import Path

logger = logging.getLogger("saver.links")

SHEET = "网址链接(去重)"
BATCH = 500          # 单次最多扫描的含链接行数

# 收尾可能粘着的中文/英文标点：句子里的链接后头常跟着句号、括号
_TRAIL_PUNCT = ".,;:!?、。；，！？：）)】》»\"'"

# URL 只认 ASCII 字符集（RFC 3986 保留字 + %）：中文正文跟在链接后面不留空格
# 时（"…x=1。还有"），放行 CJK 会把整句吞进链接
_URL_RE = re.compile(r"https?://[A-Za-z0-9\-._~:/?#@!$&*+,;=%()\[\]]+")
# 合并转发内层行有「昵称(12:34:55): 」前缀，标题里不该带上
_FWD_PREFIX_RE = re.compile(r"^\S{0,40}\(\d{2}:\d{2}:\d{2}\):\s*")

_HEADERS = ("序号", "链接", "标题/描述", "首次日期", "首次时间", "发送者", "群号", "备注")

# 表头关键词 -> 字段名（按子串匹配，容忍改名）
_HEADER_KEYS = (
    ("序号", "seq"),
    ("链接", "link"),
    ("标题", "title"),
    ("首次日期", "first_date"),
    ("首次时间", "first_time"),
    ("发送者", "sender"),
    ("群号", "gid"),
    ("备注", "note"),
)


def normalize_url(raw: str) -> str:
    """链接的去重键口径：转义还原 + 去收尾标点 + 统一小写比对。"""
    u = unescape(str(raw or "")).strip().rstrip(_TRAIL_PUNCT)
    return u


class LinkRegistry:
    def __init__(self, xlsx: Path):
        self.xlsx = Path(xlsx)
        self.seen: set[str] = set()   # 小写化后的去重键
        self.seq = 0
        self.cursor = 0
        self._scan_max = 0
        self.dirty: set[int] = set()
        self._dirty_batch: list[int] = []
        self.buffer: list[dict] = []
        self._err = ""
        self.disabled = False

    @classmethod
    def build(cls, config: dict):
        data_dir = Path(config["data_dir"])
        xlsx = Path(config.get("link_xlsx") or data_dir / "网址链接汇总.xlsx")
        try:
            reg = cls(xlsx)
            if not xlsx.is_file():
                reg._create()
                logger.info("[LINK] 登记表不存在，已新建: %s", xlsx)
            return reg
        except Exception as e:
            logger.error("[LINK] 初始化失败，跳过登记: %s: %s", type(e).__name__, e)
            return None

    def _create(self) -> None:
        import openpyxl
        wb = openpyxl.Workbook()
        wb.active.title = SHEET
        wb.active.append(_HEADERS)
        tmp = self.xlsx.with_suffix(".xlsx.tmp")
        wb.save(tmp)
        os.replace(tmp, self.xlsx)
        wb.close()

    # ---- 读表 ----
    def _read_sheet(self, wb) -> dict:
        ws = wb[SHEET]
        hdr = [str(v or "") for v in next(ws.iter_rows(max_row=1, values_only=True))]
        cols = {}
        for idx, name in enumerate(hdr):
            for key, field in _HEADER_KEYS:
                if key in name and field not in cols:
                    cols[field] = idx
                    break
        if "link" not in cols:
            raise KeyError(f"表头没有『链接』列: {hdr}")
        l_idx, s_idx = cols["link"], cols.get("seq")
        links: set[str] = set()
        seqs: set[int] = set()
        nrows = 0
        for row in ws.iter_rows(min_row=2, values_only=True):
            nrows += 1
            if len(row) > l_idx:
                u = normalize_url(row[l_idx]).lower()
                if u:
                    links.add(u)
            if s_idx is not None and len(row) > s_idx:
                try:
                    seqs.add(int(float(row[s_idx])))
                except (TypeError, ValueError):
                    pass
        return {"cols": cols, "links": links, "seqs": seqs, "nrows": nrows}

    @staticmethod
    def _base_seq(seqs: set[int], nrows: int) -> int:
        return max(seqs) if seqs else nrows

    def _load(self):
        import openpyxl
        wb = openpyxl.load_workbook(self.xlsx, read_only=True, data_only=True)
        try:
            state = self._read_sheet(wb)
        finally:
            wb.close()
        self.seen = state["links"]
        self.seq = self._base_seq(state["seqs"], state["nrows"])

    async def load(self) -> None:
        try:
            await asyncio.to_thread(self._load)
        except Exception as e:
            self.disabled = True
            logger.error("[LINK] 登记表不可用，本次运行不再登记: %s: %s",
                         type(e).__name__, e)
            return
        logger.info("[LINK] 登记表就绪: %d 个链接, 序号从 %d 起",
                    len(self.seen), self.seq + 1)

    # ---- 抽取 ----
    def _row_for(self, text, scope, peer_id, nick, ts, fm) -> dict | None:
        link = normalize_url(fm.group(0))
        key = link.lower()
        if not link or key in self.seen:
            return None
        self.seen.add(key)
        text = text or ""
        line_start = text.rfind("\n", 0, fm.start()) + 1
        head = text[line_start:fm.start()].strip(" ：:　\t")
        head = _FWD_PREFIX_RE.sub("", head)
        when = time.localtime(ts or time.time())
        d, t = time.strftime("%Y-%m-%d", when), time.strftime("%H:%M:%S", when)
        return dict(
            link=link, title=head[:300] if head else "",
            first_date=d, first_time=t,
            sender=str(nick or ""),
            gid=str(peer_id or "") if scope == "group" else "",
            note="实时新增",
        )

    def _rows_for(self, text, scope, peer_id, nick, ts, mid=None) -> list[dict]:
        out = []
        for fm in _URL_RE.finditer(text or ""):
            try:
                rec = self._row_for(text, scope, peer_id, nick, ts, fm)
            except Exception as e:
                logger.warning("[LINK] 解析失败 msg.id=%s: %s: %s", mid,
                               type(e).__name__, e)
                continue
            if rec:
                out.append(rec)
        return out

    # ---- 写表 ----
    def _flush(self) -> int:
        """追加 buffer 里表里还没有的行。返回被复查挡掉的行数。"""
        import openpyxl
        wb_r = openpyxl.load_workbook(self.xlsx, read_only=True, data_only=True)
        try:
            state = self._read_sheet(wb_r)
        finally:
            wb_r.close()
        cols = state["cols"]
        keep, batch_seen = [], set()
        for r in self.buffer:
            k = r["link"].lower()
            if k not in state["links"] and k not in batch_seen:
                batch_seen.add(k)
                keep.append(r)
        dropped = len(self.buffer) - len(keep)
        if not keep:
            return dropped
        wb = openpyxl.load_workbook(self.xlsx)
        try:
            ws = wb[SHEET]
            width = max(cols.values()) + 1
            taken = set(state["seqs"])
            for rec in keep:
                rec["seq"] = self._base_seq(taken, state["nrows"]) + 1
                taken.add(rec["seq"])
                row = [None] * width
                for field, idx in cols.items():
                    if field in rec:
                        row[idx] = rec[field]
                ws.append(row)
            tmp = self.xlsx.with_suffix(".xlsx.tmp")
            wb.save(tmp)
            os.replace(tmp, self.xlsx)
        finally:
            wb.close()
        self.seen |= state["links"] | {r["link"].lower() for r in keep}
        self.seq = self._base_seq(taken, state["nrows"])
        return dropped

    def _commit(self) -> None:
        self.cursor = max(self.cursor, self._scan_max)
        self.dirty.difference_update(self._dirty_batch)
        self._dirty_batch = []

    async def sync(self, db) -> None:
        if self.disabled:
            return
        if not self.buffer:
            self._dirty_batch = sorted(self.dirty)[:BATCH]
            where = "id>?"
            args = [self.cursor]
            if self._dirty_batch:
                where += " OR id IN (%s)" % ",".join("?" * len(self._dirty_batch))
                args += self._dirty_batch
            rows = await db._run(lambda c: c.execute(
                f"SELECT id, scope, peer_id, sender_nick, ts, text FROM messages "
                f"WHERE ({where}) AND (text LIKE '%http://%' OR text LIKE '%https://%') "
                f"ORDER BY id LIMIT ?",
                (*args, BATCH)).fetchall())
            rows = [dict(r) for r in rows]
            for r in rows:
                self._scan_max = max(self._scan_max, r["id"])
                new = self._rows_for(r["text"], r["scope"], r["peer_id"],
                                     r["sender_nick"], r["ts"], r["id"])
                if new:
                    self.buffer.extend(new)
                    logger.info("[LINK] 待登记 %s",
                                ", ".join(x["link"][:40] for x in new))
        if not self.buffer:
            self._commit()
            return
        try:
            n = len(self.buffer)
            dropped = await asyncio.to_thread(self._flush)
            self._commit()
            self.buffer.clear()
            self._err = ""
            logger.info("[LINK] 已写入 %d 行%s -> %s", n - dropped,
                        f"（复查挡掉 {dropped} 行：表里已有）" if dropped else "",
                        self.xlsx)
        except Exception as e:
            # Excel 开着这个文件时写不进去（PermissionError）：攒着，下一轮再试
            if not self._err:
                logger.error("[LINK] 写入失败，%d 行待重试: %s: %s",
                             len(self.buffer), type(e).__name__, e)
            self._err = str(e)
