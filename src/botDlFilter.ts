/** QQ Bot 当日下载列表的状态归类与筛选。
 *
 * 面板的筛选条与行内状态标签必须用同一套判定，否则会出现「筛选失败只有 2 条、
 * 表里却有三行红字」这种自相矛盾。这里把它做成纯函数，判定逻辑单测直接喂内存行。
 *
 * dl 来自 bot.db 的重试队列状态机：none（无下载动作）/ pending / busy / ok / dead。
 * dedup 是附加标记：媒体行 = 1 表示命中 md5 去重复用了现有文件；转发行 = 内部跳过的重复媒体数。
 */
import type { BotDlItem } from "./types";

export type DlStatus = "downloading" | "dead" | "dedup" | "ok" | "archived";

/** 筛选条的展示顺序：进行中 → 出问题的 → 结果。跟人的关注顺序一致 */
export const DL_STATUS_ORDER: readonly DlStatus[] = ["downloading", "dead", "dedup", "ok", "archived"] as const;

export const DL_STATUS_LABEL: Record<DlStatus, string> = {
  downloading: "下载中",
  dead: "失败",
  dedup: "重复跳过",
  ok: "已下载",
  archived: "已归档",
};

/**
 * 一行归到哪个状态。顺序有讲究：
 * - dl 状态机优先，pending/busy/dead 压过一切（哪怕这行同时带 dedup 标记）；
 * - busy（在传）与 pending（排队）合并为一个「下载中」——迅雷式的任务视角：
 *   面板只关心「这一批在下载」，在传的行另带实时百分比（progressPct），
 *   排队等槽是同一件事的等待段，不值得单独一个状态；
 * - dl='ok' 且 kind='forward' 归「已归档」——转发父行只是展开的壳，
 *   它的 dedup 记的是内部跳过的媒体数，跟「这条被去重了」不是一回事；
 * - 剩下的 ok 行才看 dedup 区分「重复跳过」与「已下载」；
 * - dl='none' 的纯文本直发与转发明细都只是入档，没有下载动作，归「已归档」。
 */
export function dlStatusOf(item: Pick<BotDlItem, "dl" | "kind" | "dedup">): DlStatus {
  if (item.dl === "busy" || item.dl === "pending") return "downloading";
  if (item.dl === "dead") return "dead";
  if (item.dl === "none" || item.kind === "forward") return "archived";
  return item.dedup > 0 ? "dedup" : "ok";
}

/** 行的展示文本：转发明细行前缀发送者昵称，转发里不同人发的同名文件才分得清 */
export function dlItemLabel(item: BotDlItem): string {
  const name = item.label || `#${item.id}`;
  return item.parent != null && item.sender ? `${item.sender}：${name}` : name;
}

/** 当日各状态的条数，供筛选条上的角标。五个键恒在，没出现的状态为 0 */
export function countByDlStatus(items: readonly BotDlItem[]): Record<DlStatus, number> {
  const counts = { downloading: 0, dead: 0, dedup: 0, ok: 0, archived: 0 } as Record<DlStatus, number>;
  for (const item of items) counts[dlStatusOf(item)] += 1;
  return counts;
}

/** 按状态筛当日列表；"all" 原样返回（筛选是纯展示，不动后端数据） */
export function filterByDlStatus<T extends BotDlItem>(items: readonly T[], status: DlStatus | "all"): T[] {
  if (status === "all") return items.slice();
  return items.filter((item) => dlStatusOf(item) === status);
}

/** 转发块聚合：parent 行 id → 明细总数与已收敛数（ok/重复跳过/归档都算完成）。
 * 面板据此给 busy 的转发壳显示「下载中 N/M 个」——壳行自己没有字节总量，
 * 聚合进度按个数计，这是迅雷式任务视角的「排序-下载」段 */
export function forwardGroupStats(
  items: readonly BotDlItem[],
): Map<number, { total: number; done: number }> {
  const m = new Map<number, { total: number; done: number }>();
  for (const it of items) {
    if (it.parent == null) continue;
    const g = m.get(it.parent) ?? { total: 0, done: 0 };
    g.total += 1;
    const st = dlStatusOf(it);
    if (st === "ok" || st === "dedup" || st === "archived") g.done += 1;
    m.set(it.parent, g);
  }
  return m;
}

/** 转发/引用任务行的状态文本：壳行 busy 时——明细还没登记显「解析中」（展开
 * get_forward_msg / 取原文 get_msg 的往返）；明细在册显「下载中 N/M 个」。
 * 非 busy（排队中的壳）返回 null，走常规「下载中」标签 */
export function dlForwardTaskText(
  item: Pick<BotDlItem, "kind" | "dl">,
  group?: { total: number; done: number },
): string | null {
  if (item.dl !== "busy") return null;
  if (item.kind === "reply") return "解析中";
  if (item.kind !== "forward") return null;
  if (!group || group.total === 0) return "解析中";
  return `下载中 ${group.done}/${group.total} 个`;
}

/** 下载速度估算（字节/秒，0 = 不显示；面板负责格式化成「 · 1.5 MB/s」）。
 *
 * 进度按 1MB 块跳变（db.set_dl_progress 每块写回一次），逐秒差分全是锯齿：
 * 慢速行大半时间读 0、偶尔蹦一格。按 ≥8s 的窗口差分，窗口期内沿用上一次读数；
 * 15 秒没有进账（等 src 刷新 / 连接挂死）归零；行收敛（非 busy）清状态。
 * 窗口表按行 id 存模块级，由行组件渲染驱动推进（行每轮快照都重渲）；
 * 新一轮认领会把字节清零重传（bytes 比上次还小），据此重开窗口。
 */
const DL_SPEED_WINDOW_MS = 8_000;
const DL_SPEED_STALE_MS = 15_000;
type SpeedWindow = { t0: number; b0: number; last: number; lastAdv: number; speed: number };
const speedWindows = new Map<number, SpeedWindow>();

export function dlSpeedOf(
  item: Pick<BotDlItem, "id" | "dl" | "dl_bytes">,
  now = Date.now(),
): number {
  if (item.dl !== "busy") {
    speedWindows.delete(item.id);
    return 0;
  }
  const bytes = item.dl_bytes ?? 0;
  let w = speedWindows.get(item.id);
  if (!w || bytes < w.last) {
    w = { t0: now, b0: bytes, last: bytes, lastAdv: now, speed: 0 };
  } else {
    if (bytes > w.last) w.lastAdv = now;
    w.last = bytes;
    const dt = (now - w.t0) / 1000;
    if (dt >= DL_SPEED_WINDOW_MS / 1000) {
      w.speed = (bytes - w.b0) / dt;
      w.t0 = now;
      w.b0 = bytes;
    }
  }
  speedWindows.set(item.id, w);
  if (now - w.lastAdv > DL_SPEED_STALE_MS || w.speed < 1) return 0;
  return w.speed;
}