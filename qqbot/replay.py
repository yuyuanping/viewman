"""查看/重放持久化队列状态（方案 B）。

用法：
  python replay.py                    # 查看 events / dl 队列各状态计数
  python replay.py --retry            # 把 error/dead 事件与 dead 下载重置回队列
                                       # （dead 本会自动每天复检一次，这条是立刻重做）
  python replay.py --backfill-ref     # 从 events 原文补齐 image/video 的 file id 并重排
                                       # （NapCat 把这个 id 放在 data.file，早期版本没存，
                                       #  缺了它就没有 CDN 过期后的本地原件兜底）
  python replay.py --retry-forward-media
                                       # 只重排“历史合并转发里图片/视频未保存”的行，
                                       # 成功后更新文本，不在日记中追加重复块
"""
import argparse
import asyncio
import json
import re
from pathlib import Path

from config import load_config
from db import DB


async def backfill_ref(db) -> None:
    rows = await db._run(lambda c: c.execute(
        "SELECT id, event_id, seg_index FROM messages "
        "WHERE kind IN ('image','video') AND dl IN ('dead','error','pending') "
        "AND (ref IS NULL OR ref='')").fetchall())

    def _pick(conn):
        out = []
        for mid, eid, si in rows:
            ev = conn.execute("SELECT payload FROM events WHERE id=?", (eid,)).fetchone()
            if not ev:
                continue
            try:
                segs = (json.loads(ev[0]).get("message")) or []
                fid = str((segs[si].get("data") or {}).get("file") or "")
            except (ValueError, IndexError, AttributeError):
                fid = ""
            if fid:
                out.append((mid, fid))
        return out

    plan = await db._run(_pick)
    print(f"缺 file id 的 dead/error 下载：{len(rows)} 条，可从 events 原文补回 {len(plan)} 条")
    for mid, fid in plan:
        print(f"  id={mid} ref={fid}")
    if not plan:
        return
    await db._run(lambda c: c.executemany(
        "UPDATE messages SET ref=?, dl='pending', tries=0, next_try=0, err=NULL "
        "WHERE id=?", [(fid, mid) for mid, fid in plan]))
    print(f"已回填 {len(plan)} 条并重排（运行中的 bot 约 20 秒内自动重做）")



BARE_FORWARD_MEDIA = re.compile(
    r"\[(?:图片|视频)\](?!\s+\S+\.(?:jpe?g|png|webp|gif|bmp|mp4|mov|webm))",
    re.IGNORECASE,
)


async def retry_forward_media(db) -> None:
    rows = await db._run(lambda c: c.execute(
        "SELECT id, text FROM messages "
        "WHERE kind='forward' AND COALESCE(text,'')<>'' AND dl<>'busy'"
    ).fetchall())
    ids = [r["id"] for r in rows if BARE_FORWARD_MEDIA.search(r["text"] or "")]
    print(f"合并转发中含未保存图片/视频占位：{len(ids)} 条")
    if not ids:
        return
    await db._run(lambda c: c.executemany(
        "UPDATE messages SET dl='pending', tries=0, next_try=0, err=NULL WHERE id=?",
        [(mid,) for mid in ids],
    ))
    print("已重排这些合并转发行（运行中的 bot 约 20 秒内自动重做）")


async def run(args) -> None:
    cfg = load_config()
    db = DB(Path(cfg["data_dir"]) / "bot.db")
    if args.backfill_ref:
        await backfill_ref(db)
    if args.retry_forward_media:
        await retry_forward_media(db)
    if args.retry:
        n1 = await db._run(lambda c: c.execute(
            "UPDATE events SET status='new', tries=0, next_try=0 "
            "WHERE status IN ('error','dead')").rowcount)
        n2 = await db._run(lambda c: c.execute(
            "UPDATE messages SET dl='pending', tries=0, next_try=0 "
            "WHERE dl='dead'").rowcount)
        print(f"已重置：事件 {n1} 条、下载 {n2} 条"
              + ("（运行中的 bot 约 20 秒内自动重做）" if n1 or n2 else ""))
    stats = await db.stats()
    print("events:", stats["events"] or {})
    print("media/forward dl:", stats["dl"] or {})
    total = (await db._run(lambda c: c.execute(
        "SELECT COUNT(*) FROM messages").fetchone()))[0]
    media = await db._run(lambda c: c.execute(
        "SELECT COUNT(*), COALESCE(SUM(bytes),0) FROM media").fetchone())
    print(f"messages: {total} 条 | media: {media[0]} 个 / {media[1] / 1048576:.1f} MB")


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--retry", action="store_true", help="重置 error/dead 项重放")
    p.add_argument("--backfill-ref", action="store_true",
                   help="从 events 原文补齐 image/video 的 file id 并重排该下载")
    p.add_argument("--retry-forward-media", action="store_true",
                   help="重排历史合并转发中未保存的图片/视频")
    asyncio.run(run(p.parse_args()))
