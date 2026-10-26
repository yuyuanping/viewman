/** QQ Bot 下载快照的变化判定（纯函数）。
 *
 * useBotStatus 收到 bot-dl-changed 推送（200ms 防抖合并）后拉一次轻量快照。status 是 App 的 state，
 * 每轮都换新对象会把侧栏、工具栏、视频网格整棵树重渲染一遍；所以只有
 * 数字真变了才更新引用。
 */
import type { BotDlSnapshot, BotStatus } from "./types";

/**
 * 轻量快照与上一份相比有没有实质变化。
 *
 * 只比数字（积压计数、今日汇总、当日条数），数组内容仅扫 busy 行的进度字节
 * （下载中的 dl_bytes 每轮都在涨，列表长度却不变——不看它就永远不重渲染，
 * 进度条就死了）；终态行不可能变，无需逐行 diff。两侧都是后端整份算出来的。
 */
export function dlSnapshotChanged(prev: BotStatus, next: BotDlSnapshot): boolean {
  const a = prev.dl;
  const b = next.dl;
  if (a.pending !== b.pending || a.busy !== b.busy || a.dead !== b.dead || a.dead_today !== b.dead_today) return true;
  const s = prev.summary;
  const t = next.summary;
  if (s.ok !== t.ok || s.ok_bytes !== t.ok_bytes || s.today !== t.today
    || s.today_bytes !== t.today_bytes || s.dedup_today !== t.dedup_today
    || prev.today_items.length !== next.today_items.length) return true;
  // busy 行的实时进度：旧快照里在下的行，新快照的对应行字节变了就要换引用。
  // 建 id→bytes 映射只扫 busy 行，空闲期（没有 busy）零开销
  const prevBusy = new Map<number, number>();
  for (const it of prev.today_items) if (it.dl === "busy") prevBusy.set(it.id, it.dl_bytes ?? 0);
  for (const it of next.today_items) {
    const old = prevBusy.get(it.id);
    if (old !== undefined && old !== (it.dl_bytes ?? 0)) return true;
    if (old === undefined && it.dl === "busy" && (it.dl_bytes ?? 0) > 0) return true;
  }
  return false;
}
