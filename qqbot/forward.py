"""合并转发（asyncio）：递归展开（最多 3 层），媒体单次下载，返回格式化文本。"""
import json
import re
from collections.abc import Callable
from datetime import datetime
from html import unescape
from pathlib import Path



MAX_DEPTH = 3

# 消息段中不直接下载、仅转成占位标签的类型
# （媒体三类在 render_node 里优先走下载，此处标签供引用消息摘要等回退用）
_LABELS = {
    "face": "[表情]", "at": "[@]", "reply": "[回复]", "voice": "[语音]",
    "image": "[图片]", "video": "[视频]", "file": "[文件]",
    "forward": "[合并转发]",
    "json": "[JSON卡片]", "xml": "[XML卡片]", "share": "[分享]",
    "location": "[位置]", "music": "[音乐]", "redbag": "[红包]",
}

# 卡片/分享类段：正文要从载体里提取链接存档，不能只落占位符——
# 网址分享、音乐/位置卡片、JSON 卡片里的磁力链接全在这一类（2026-10-11 用户要求）
_CARD_TYPES = ("json", "xml", "share", "music", "location")
# URL 段只认 ASCII 字符集：卡片 JSON 里链接后直接跟中文时不会吞字
_LINK_RE = re.compile(r"https?://[A-Za-z0-9\-._~:/?#@!$&*+,;=%()\[\]]+"
                      r"|magnet:\?xt=urn:btih:[0-9A-Za-z]+")


def _collect_links(obj, out: list[str]) -> None:
    if isinstance(obj, str):
        out.extend(_LINK_RE.findall(obj))
    elif isinstance(obj, dict):
        for v in obj.values():
            _collect_links(v, out)
    elif isinstance(obj, list):
        for v in obj:
            _collect_links(v, out)


def card_text(seg: dict) -> str:
    """卡片类段的可存档文本：占位标签 + 展示文案 + 提取出的全部链接。

    json/xml 的 data 是载体字符串（JSON 串 / XML 串），链接藏在嵌套字段或
    属性里，递归抓最稳；share/music/location 的字段是平铺的。重复链接只留
    一处，一个链接都没有时退回纯占位标签。"""
    t = seg.get("type")
    d = seg.get("data", {})
    label = _LABELS.get(t, f"[{t}]")
    body = ""   # 给人看的正文：平铺字段；json 只取 prompt
    if t in ("json", "xml"):
        raw = str(d.get("data") or "")
    elif t == "location":
        # 位置卡不是 title/content/url 这些字段：NapCat 给的是经纬度 + 地名，
        # 只拼平铺字段的话整张卡退化成 [位置]，坐标一个都不留
        title = d.get("title") or d.get("name") or ""
        summary = d.get("summary") or d.get("address") or ""
        body = " ".join(str(x) for x in (title, summary) if x)
        raw = body
        lat, lng = d.get("lat"), d.get("lng")
        if lat is not None and lng is not None:
            raw = f"{raw}\n({lat},{lng})".strip()
    else:
        raw = body = " ".join(str(d.get(k, ""))
                              for k in ("title", "content", "url", "address"))
    if t == "xml":
        raw = unescape(raw)   # 属性里的 &amp; 还原成 & 才能匹配出完整链接
    if t == "json":
        try:
            body = str(json.loads(raw).get("prompt") or "")
        except Exception:
            body = ""         # 解析不了宁可只留标签 + 抓到的链接，别把 JSON 原文抄进日记
    links: list[str] = []
    _collect_links(raw, links)
    seen: set[str] = set()
    links = [u for u in links if not (u in seen or seen.add(u))]
    lines = [label]
    # 平铺字段的正文里本来就夹着链接（share/music/location），先摘掉链接再当标题，
    # 否则同一链接在正文和下面各出现一次
    title = body
    for u in links:
        title = title.replace(u, "")
    title = " ".join(title.split())
    # 分享类小程序的 prompt 自带 [小程序名]，跟标签重复的别再抄一遍
    if title and title not in lines:
        lines.append(title)
    lines.extend(links)
    if len(lines) == 1 and t == "location":
        # 只有坐标没有地名：至少把坐标留下
        tail = raw.split("\n")[-1].strip()
        if tail.startswith("("):
            lines.append(tail)
    return "\n".join(lines)


def seg_text(seg: dict) -> str:
    """单个非媒体段的文本表示（文本/表情/@/回复/卡片占位）。"""
    t = seg.get("type")
    d = seg.get("data", {})
    if t == "text":
        return d.get("text", "")
    if t in _CARD_TYPES:
        return card_text(seg)
    if t == "face":
        return f"[表情{d.get('id', '')}]"
    if t == "at":
        return f"@{d.get('qq', '')} "
    if t == "reply":
        return "[回复]"
    return _LABELS.get(t, f"[{t}]")



async def _save_media_segment(data: dict, kind: str, storage,
                              get_file_url: Callable, scope: str,
                              on_progress=None):
    file_id = data.get("file_id") or data.get("file") or ""
    expect_md5 = md5_hint(file_id)
    url = data.get("url", "")
    result = await storage.save_media(
        url, kind, scope, expect_md5=expect_md5, on_progress=on_progress
    ) if url else None
    if result is None and file_id:
        fallback_url = await get_file_url(file_id)
        if fallback_url:
            result = await storage.save_media(
                fallback_url, kind, scope, expect_md5=expect_md5,
                on_progress=on_progress
            )
    return result


async def render_node(msgs: list[dict], storage, expand_forward: Callable,
                      get_file_url: Callable, scope: str = "private",
                      depth: int = 0, stats: dict | None = None,
                      report=None, on_progress=None) -> str:
    """递归渲染转发消息。图片/视频/文件在此层下载一次，避免重复。
    scope 决定媒体存入 private/ 还是 group/。
    stats 传入 dict 时累计 {"saved": 成功落盘数, "dedup": 其中复用现有文件的数}。
    report 传入 async fn 时逐段上报明细，条目字段：
      kind: text/image/video/file/forward
      sender/ts: 该段所在内层消息的发送者与时间
      text: text/forward 的正文；label: 媒体文件名或占位

      dedup/md5/path/bytes: ok=True 时媒体结果；src/ref/file: 供失败重试用"""
    if depth > MAX_DEPTH or not msgs:
        return ""
    prefix = "  " * (depth + 1)
    lines = []
    for m in msgs:
        sender = m.get("sender", {})
        nickname = sender.get("nickname", str(sender.get("user_id", "?")))
        inner_ts = m.get("time", 0)
        ts = datetime.fromtimestamp(inner_ts).strftime("%H:%M:%S")
        parts = []
        for seg in m.get("message", []):
            t = seg.get("type")
            d = seg.get("data", {})
            if t == "image":
                r = await _save_media_segment(d, "image", storage,
                                              get_file_url, scope, on_progress)
                if r and stats is not None:
                    stats["saved"] = stats.get("saved", 0) + 1
                    if r.get("dedup"):
                        stats["dedup"] = stats.get("dedup", 0) + 1
                if report:
                    await report({"kind": "image", "sender": nickname, "ts": inner_ts,
                                  "src": d.get("url", ""), "ref": d.get("file_id") or d.get("file") or "",
                                  "file": d.get("file"),
                                  "ok": r is not None,
                                  "dedup": (r or {}).get("dedup"),
                                  "md5": (r or {}).get("md5"), "path": (r or {}).get("path"),
                                  "bytes": (r or {}).get("bytes"),
                                  "label": Path(r["path"]).name if r else (d.get("file") or "图片")})
                parts.append(r["placeholder"] if r else "[图片]")
            elif t == "video":
                r = await _save_media_segment(d, "video", storage,
                                              get_file_url, scope, on_progress)
                if r and stats is not None:
                    stats["saved"] = stats.get("saved", 0) + 1
                    if r.get("dedup"):
                        stats["dedup"] = stats.get("dedup", 0) + 1
                if report:
                    await report({"kind": "video", "sender": nickname, "ts": inner_ts,
                                  "src": d.get("url", ""), "ref": d.get("file_id") or d.get("file") or "",
                                  "file": d.get("file"),
                                  "ok": r is not None,
                                  "dedup": (r or {}).get("dedup"),
                                  "md5": (r or {}).get("md5"), "path": (r or {}).get("path"),
                                  "bytes": (r or {}).get("bytes"),
                                  "label": Path(r["path"]).name if r else (d.get("file") or "视频")})
                parts.append(r["placeholder"] if r else "[视频]")
            elif t == "file":
                fid = d.get("file_id") or d.get("url") or ""
                r = await storage.save_file(fid, d.get("file", "file"),
                                            get_file_url, scope,
                                            on_progress=on_progress)
                if r and stats is not None:
                    stats["saved"] = stats.get("saved", 0) + 1
                    if r.get("dedup"):
                        stats["dedup"] = stats.get("dedup", 0) + 1
                if report:
                    await report({"kind": "file", "sender": nickname, "ts": inner_ts,
                                  "src": fid, "ref": d.get("file_id") or "",
                                  "file": d.get("file"),
                                  "ok": r is not None,
                                  "dedup": (r or {}).get("dedup"),
                                  "md5": (r or {}).get("md5"), "path": (r or {}).get("path"),
                                  "bytes": (r or {}).get("bytes"),
                                  "label": d.get("file") or "文件"})
                parts.append(r["placeholder"] if r else "[文件]")
            elif t == "record":
                fid = d.get("file_id") or d.get("url") or ""
                name = d.get("file") or f"voice_{fid[:8] or 'msg'}.mp3"
                r = await storage.save_file(fid, name, get_file_url, scope,
                                            on_progress=on_progress)
                if r and stats is not None:
                    stats["saved"] = stats.get("saved", 0) + 1
                    if r.get("dedup"):
                        stats["dedup"] = stats.get("dedup", 0) + 1
                if report:
                    await report({"kind": "file", "sender": nickname, "ts": inner_ts,
                                  "src": fid, "ref": d.get("file_id") or "",
                                  "file": name, "ok": r is not None,
                                  "dedup": (r or {}).get("dedup"),
                                  "md5": (r or {}).get("md5"), "path": (r or {}).get("path"),
                                  "bytes": (r or {}).get("bytes"), "label": name})
                parts.append(r["placeholder"] if r else "[语音]")
            elif t == "mface":
                r = await _save_media_segment(d, "image", storage,
                                              get_file_url, scope, on_progress)
                if r and stats is not None:
                    stats["saved"] = stats.get("saved", 0) + 1
                    if r.get("dedup"):
                        stats["dedup"] = stats.get("dedup", 0) + 1
                if report:
                    await report({"kind": "image", "sender": nickname, "ts": inner_ts,
                                  "src": d.get("url", ""), "ref": d.get("file_id") or "",
                                  "file": d.get("file") or "",
                                  "ok": r is not None,
                                  "dedup": (r or {}).get("dedup"),
                                  "md5": (r or {}).get("md5"), "path": (r or {}).get("path"),
                                  "bytes": (r or {}).get("bytes"),
                                  "label": Path(r["path"]).name if r else (d.get("emoji_id") or "表情")})
                parts.append(r["placeholder"] if r else "[表情]")
            elif t == "forward":
                nested = await _expand(seg.get("data", {}), storage, expand_forward,
                                       get_file_url, scope, depth, stats, report,
                                       nickname, inner_ts, on_progress)
                parts.append(nested or "[合并转发]")
            else:
                if report and t == "text":
                    txt = d.get("text", "")
                    if txt.strip():
                        await report({"kind": "text", "sender": nickname,
                                      "ts": inner_ts, "text": txt})
                parts.append(seg_text(seg))
        lines.append(f"{prefix}{nickname}({ts}): {''.join(parts)}")
    return "\n".join(lines)


async def _expand(data: dict, storage, expand_forward: Callable,
                  get_file_url: Callable, scope: str, depth: int,
                  stats: dict | None = None, report=None,
                  sender: str = "", ts: int = 0, on_progress=None) -> str:
    nested_fid = data.get("id", "")
    if not nested_fid or depth >= MAX_DEPTH:
        return ""
    content = data.get("content")  # NapCat 直接内联的转发体
    if content:
        nested_text = await render_node(content, storage, expand_forward,
                                        get_file_url, scope, depth + 1, stats,
                                        report, on_progress)
    else:
        nested_data = await expand_forward(nested_fid)
        nested_text = await render_node(
            nested_data.get("messages", []), storage, expand_forward,
            get_file_url, scope, depth + 1, stats, report, on_progress
        ) if nested_data else ""
    if report:
        first = (nested_text or "[合并转发]").splitlines()[0][:80]
        await report({"kind": "forward", "sender": sender, "ts": ts,
                      "text": f"[嵌套转发] {first}"})
    if not nested_text:
        return "[合并转发](空)"
    indent = "  " * (depth + 1)
    return f"[合并转发↓]\n{nested_text}\n{indent}[合并转发↑]"
from storage import md5_hint
