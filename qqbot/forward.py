"""合并转发（asyncio）：递归展开（最多 3 层），媒体单次下载，返回格式化文本。"""
from datetime import datetime
from pathlib import Path
from typing import Callable

from storage import md5_hint, is_sticker

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


def seg_text(seg: dict) -> str:
    """单个非媒体段的文本表示（文本/表情/@/回复/卡片占位）。"""
    t = seg.get("type")
    d = seg.get("data", {})
    if t == "text":
        return d.get("text", "")
    if t == "face":
        return f"[表情{d.get('id', '')}]"
    if t == "at":
        return f"@{d.get('qq', '')} "
    if t == "reply":
        return "[回复]"
    return _LABELS.get(t, f"[{t}]")



async def _save_media_segment(data: dict, kind: str, storage,
                              get_file_url: Callable, scope: str):
    file_id = data.get("file_id") or data.get("file") or ""
    expect_md5 = md5_hint(file_id)
    url = data.get("url", "")
    result = await storage.save_media(
        url, kind, scope, expect_md5=expect_md5
    ) if url else None
    if result is None and file_id:
        fallback_url = await get_file_url(file_id)
        if fallback_url:
            result = await storage.save_media(
                fallback_url, kind, scope, expect_md5=expect_md5
            )
    return result


async def render_node(msgs: list[dict], storage, expand_forward: Callable,
                      get_file_url: Callable, scope: str = "private",
                      depth: int = 0, stats: dict | None = None,
                      report=None) -> str:
    """递归渲染转发消息。图片/视频/文件在此层下载一次，避免重复。
    scope 决定媒体存入 private/ 还是 group/。
    stats 传入 dict 时累计 {"saved": 成功落盘数, "dedup": 其中复用现有文件的数}。
    report 传入 async fn 时逐段上报明细，条目字段：
      kind: text/image/video/file/forward
      sender/ts: 该段所在内层消息的发送者与时间
      text: text/forward 的正文；label: 媒体文件名或占位
      ok: True 已保存 / False 下载失败 / None 未尝试（拦截、表情包）
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
                if is_sticker(d):
                    parts.append("[表情包]")   # 表情包不下载，只记占位
                    continue
                if storage.block_media:
                    parts.append("[图片·拦截]")
                    if report:
                        await report({"kind": "image", "sender": nickname,
                                      "ts": inner_ts, "ok": None,
                                      "label": "[拦截] " + (d.get("file") or "图片")})
                    continue
                r = await _save_media_segment(d, "image", storage,
                                              get_file_url, scope)
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
                if storage.block_media:
                    parts.append("[视频·拦截]")
                    if report:
                        await report({"kind": "video", "sender": nickname,
                                      "ts": inner_ts, "ok": None,
                                      "label": "[拦截] " + (d.get("file") or "视频")})
                    continue
                r = await _save_media_segment(d, "video", storage,
                                              get_file_url, scope)
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
                if storage.block_media:
                    parts.append("[文件·拦截]")
                    if report:
                        await report({"kind": "file", "sender": nickname,
                                      "ts": inner_ts, "ok": None,
                                      "label": "[拦截] " + (d.get("file") or "文件")})
                    continue
                fid = d.get("file_id") or d.get("url") or ""
                r = await storage.save_file(fid, d.get("file", "file"),
                                            get_file_url, scope)
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
            elif t == "forward":
                nested = await _expand(seg.get("data", {}), storage, expand_forward,
                                       get_file_url, scope, depth, stats, report,
                                       nickname, inner_ts)
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
                  sender: str = "", ts: int = 0) -> str:
    nested_fid = data.get("id", "")
    if not nested_fid or depth >= MAX_DEPTH:
        return ""
    content = data.get("content")  # NapCat 直接内联的转发体
    if content:
        nested_text = await render_node(content, storage, expand_forward,
                                        get_file_url, scope, depth + 1, stats,
                                        report)
    else:
        nested_data = await expand_forward(nested_fid)
        nested_text = await render_node(
            nested_data.get("messages", []), storage, expand_forward,
            get_file_url, scope, depth + 1, stats, report
        ) if nested_data else ""
    if report:
        first = (nested_text or "[合并转发]").splitlines()[0][:80]
        await report({"kind": "forward", "sender": sender, "ts": ts,
                      "text": f"[嵌套转发] {first}"})
    if not nested_text:
        return "[合并转发](空)"
    indent = "  " * (depth + 1)
    return f"[合并转发↓]\n{nested_text}\n{indent}[合并转发↑]"
