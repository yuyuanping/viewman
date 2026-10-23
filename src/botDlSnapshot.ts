/** QQ Bot 下载快照的变化判定（纯函数）。
 *
 * useBotStatus 在下载活跃期每 2 秒拉一次轻量快照。status 是 App 的 state，
 * 每轮都换新对象会把侧栏、工具栏、视频网格整棵树重渲染一遍；所以只有
 * 数字真变了才更新引用。
 */
import type { BotDlSnapshot, BotStatus } from "./types";

/**
 * 轻量快照与上一份相比有没有实质变化。
 *
 * 只比数字（积压计数、今日汇总、当日条数），不比数组内容——列表本身在下载期间
 * 大概率在变，但只比长度就够：真有新行/状态翻转，长度或计数必变。两侧都是
 * 后端整份算出来的，不必逐行 diff。
 */
export function dlSnapshotChanged(prev: BotStatus, next: BotDlSnapshot): boolean {
  const a = prev.dl;
  const b = next.dl;
  if (a.pending !== b.pending || a.busy !== b.busy || a.dead !== b.dead || a.dead_today !== b.dead_today) return true;
  const s = prev.summary;
  const t = next.summary;
  return s.ok !== t.ok || s.ok_bytes !== t.ok_bytes || s.today !== t.today
    || s.today_bytes !== t.today_bytes || s.dedup_today !== t.dedup_today
    || prev.today_items.length !== next.today_items.length;
}
