"""全文检索消息存档（方案 B 索引层）。

用法：
  python search.py 关键词 [--scope private|group] [--limit N]
  python search.py --file <md5前缀>        # 按 MD5 查媒体文件

>=3 字符走 FTS5 trigram 索引，短查询自动回退 LIKE 扫描。
"""
import argparse
import asyncio
from datetime import datetime
from pathlib import Path

from config import load_config
from db import DB


async def run(args) -> None:
    cfg = load_config()
    db = DB(Path(cfg["data_dir"]) / "bot.db")
    if args.file:
        rows = await db._run(lambda c: c.execute(
            "SELECT * FROM media WHERE md5 LIKE ? ORDER BY first_seen DESC LIMIT ?",
            (args.file + "%", args.limit or 20)).fetchall())
        for r in rows:
            ts = datetime.fromtimestamp(r["first_seen"]).strftime("%Y-%m-%d %H:%M")
            print(f"{ts}  {r['kind']:<5} {r['scope']:<7} {r['bytes']}B  {r['path']}")
        if not rows:
            print("未找到匹配媒体")
        return

    if not args.query:
        raise SystemExit("请提供关键词，或用 --file 查媒体")
    rows = await db.search(args.query, limit=args.limit or 50)
    if args.scope:
        rows = [r for r in rows if r["scope"] == args.scope]
    if not rows:
        print("未找到匹配消息")
        return
    for r in rows:
        ts = datetime.fromtimestamp(r["ts"] or 0).strftime("%m-%d %H:%M:%S")
        who = r["sender_nick"] or "?"
        loc = f"群{r['peer_id']}" if r["scope"] == "group" else f"私聊{r['peer_id']}"
        extra = ""
        if r["path"]:
            extra = f"  -> {r['path']}"
        elif r["dl"] == "pending" and r["kind"] != "text":
            extra = f"  [{r['kind']} 下载未完成]"
        text = (r["text"] or "").replace("\n", " ⏎ ")
        print(f"{ts}  {loc:<12} {who:<12} [{r['kind']}]{extra}\n    {text[:200]}")
    print(f"--\n{len(rows)} 条（另有更多可加大 --limit）" if len(rows) >= (args.limit or 50)
          else f"{len(rows)} 条")


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("query", nargs="?", help="检索关键词")
    p.add_argument("--scope", choices=["private", "group"])
    p.add_argument("--limit", type=int, default=50)
    p.add_argument("--file", metavar="MD5前缀", help="按媒体 MD5 前缀查文件")
    args = p.parse_args()
    asyncio.run(run(args))
