import { memo, useMemo, useState } from "react";
import type { BotDlItem, BotStatus } from "../types";
import { formatDlSize } from "../utils";
import {
  countByDlStatus,
  filterByDlStatus,
  dlItemLabel,
  dlStatusOf,
  DL_STATUS_LABEL,
  DL_STATUS_ORDER,
  type DlStatus,
} from "../botDlFilter";

/** 状态文字的配色：行内标签与筛选条共用一套，深浅不同但语义一致 */
const STATUS_CLASS: Record<DlStatus, string> = {
  busy: "text-blue-300",
  pending: "text-yellow-300",
  dead: "text-red-300",
  dedup: "text-gray-400",
  ok: "text-emerald-300",
  archived: "text-gray-500",
};

/** 没有状态时的稳定空数组：每次渲染新建 [] 会让下面的 memo 全部失效 */
const EMPTY_ITEMS: BotDlItem[] = [];

const KIND_LABEL: Record<string, string> = {
  image: "图片",
  video: "视频",
  file: "文件",
  forward: "转发",
  reply: "回复",
  text: "文本",
};

/** 「已归档」要区分纯文本直发与转发壳，细分一下文案 */
function statusLabel(item: BotDlItem): string {
  const status = dlStatusOf(item);
  return status === "archived" && item.kind === "text" ? "已存档" : DL_STATUS_LABEL[status];
}

/** 下载中行的实时进度串：总量已知给「42%」，未知只显「下载中」 */
function progressPct(item: BotDlItem): string {
  if (item.dl !== "busy") return "";
  const done = item.dl_bytes ?? 0;
  const total = item.dl_total;
  if (total == null || total <= 0) return "";
  // 转发块聚合进度的 total 恒为 NULL，这里只兜单行；夹到 100 防最后一块超额
  return ` ${Math.min(100, Math.round((done / total) * 100))}%`;
}

/** 下载中行的尺寸串：显示已落盘字节（转发父行=块内累计），下完翻回最终大小 */
function sizeText(item: BotDlItem): string {
  if (item.dl === "busy") {
    const done = item.dl_bytes ?? 0;
    return done > 0 ? formatDlSize(done) : "…";
  }
  return formatDlSize(item.size);
}

/** 接收时间串：今天收到的只显 HH:MM 省宽度；跨日的（昨日消息今天复检转死等）补 MM-DD。
 * 转发明细行后端已折算成父转发收到的时刻，不是内层消息的原始发送时间 */
function recvText(item: BotDlItem): string {
  if (item.ts == null) return "-";
  const d = new Date(item.ts * 1000);
  const now = new Date();
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  const sameDay =
    d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth() && d.getDate() === now.getDate();
  return sameDay ? hm : `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")} ${hm}`;
}

/** 状态筛选条上的胶囊按钮；选中态高亮，角标是该状态的当日条数 */
function FilterChip({
  active,
  label,
  count,
  onClick,
}: {
  active: boolean;
  label: string;
  count: number;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      aria-pressed={active}
      className={`px-2 py-0.5 rounded-full text-[11px] border transition-colors ${
        active
          ? "bg-blue-600/70 border-blue-500 text-white"
          : "bg-white/5 border-transparent text-gray-400 hover:bg-white/10"
      } ${count === 0 && !active ? "opacity-50" : ""}`}
    >
      {label} {count}
    </button>
  );
}

/** QQ 存档 Bot 面板：仅下载列表（启停控制与 NapCat 配置卡已移到侧栏 QqBotControls） */
/** 表格一行：转发明细缩进挂在父行下面；行组件 memo 后，2 秒一轮快照只重渲变化行 */
const DlRow = memo(function DlRow({ item }: { item: BotDlItem }) {
  const kindLabel = KIND_LABEL[item.kind] ?? item.kind;
  const label = dlItemLabel(item);
  const dedupSuffix = item.dl === "ok" && item.dedup > 1 ? `（${item.dedup} 个重复）` : "";
  // 明细行缩进挂在所属转发行下面（后端已按 COALESCE(parent,id) 排好序）
  const isDetail = item.parent != null;
  return (
    <tr
      className="border-t border-white/5 hover:bg-white/5"
      title={item.err ?? (isDetail ? `转发 #${item.parent} 的第 ${item.id} 段` : undefined)}
    >
      <td
        className={`py-1.5 px-2 text-gray-200 truncate max-w-[36ch] ${isDetail ? "pl-6 text-gray-300" : ""}`}
        title={label}
      >
        {isDetail && <span className="text-gray-600 mr-1">└</span>}
        {label}
      </td>
      <td className="py-1.5 px-2 whitespace-nowrap tabular-nums text-gray-500">{recvText(item)}</td>
      <td className="py-1.5 px-2 text-right tabular-nums text-gray-400">{sizeText(item)}</td>
      <td className="py-1.5 px-2 text-gray-300">{kindLabel}</td>
      <td className="py-1.5 px-2">
        <span className={STATUS_CLASS[dlStatusOf(item)]}>
          {statusLabel(item)}{progressPct(item)}{dedupSuffix}
        </span>
      </td>
    </tr>
  );
});

export function QqBotPanel({
  status,
  loadError,
}: {
  /** 启停按钮与状态点已移到侧栏 QqBotControls；面板只读 status 判冷启动提示 */
  status: BotStatus | null;
  loadError: string | null;
}) {
  const [dlFilter, setDlFilter] = useState<DlStatus | "all">("all");

  // 当日列表：后端已按自然日取全量，这里只做前端筛选，切换状态不用回后端。
  // 计数与筛选都走 memo：下载活跃期每 2 秒换一次列表，没必要每轮再全量遍历两遍
  const todayItems = status?.today_items ?? EMPTY_ITEMS;
  const statusCounts = useMemo(() => countByDlStatus(todayItems), [todayItems]);
  const visibleItems = useMemo(() => filterByDlStatus(todayItems, dlFilter), [todayItems, dlFilter]);

  // 端口不通且 bot 也没在跑时提示冷启动等待；NapCat 侧的扫码引导由下方
  // NapCat 状态卡的二维码+summary 负责，这里不再重复说
  const hint = !status
    ? null
    : !status.bot_running && !status.ws_up
      ? "bot 未运行、NapCat 端口也未开。若刚点过启动，NapCat 冷启动约需 1-2 分钟；期间多为等待扫码。"
      : null;

  return (
    <div className="flex-1 overflow-y-auto pr-1 flex flex-col gap-4">
      {loadError && (
        <div role="alert" className="bg-red-900/80 text-red-100 px-3 py-2 rounded-lg text-sm">
          状态读取失败：{loadError}
        </div>
      )}
      {status?.db_note && (
        <div role="status" className="bg-amber-900/80 text-amber-100 px-3 py-2 rounded-lg text-sm">
          {status.db_note}
        </div>
      )}
      {hint && (
        <div role="status" className="bg-blue-950/80 text-blue-100 px-3 py-2 rounded-lg text-sm border border-blue-900/60">
          {hint}
        </div>
      )}

      {status && (todayItems.length > 0 || status.summary.ok > 0) && (
        <section className="bg-white/5 rounded-xl p-4 flex flex-col gap-2 min-h-0" style={{ flex: "1 1 260px" }}>
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium">下载列表（今日 {todayItems.length} 条）</span>
            <span className="text-xs text-gray-400">
              {status.dl.busy + status.dl.pending > 0 ? "实时刷新中（约 1 秒）" : "每 10 秒自动刷新"}
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <FilterChip
              active={dlFilter === "all"}
              label="全部"
              count={todayItems.length}
              onClick={() => setDlFilter("all")}
            />
            {DL_STATUS_ORDER.map((s) => (
              <FilterChip
                key={s}
                active={dlFilter === s}
                label={DL_STATUS_LABEL[s]}
                count={statusCounts[s]}
                onClick={() => setDlFilter(s)}
              />
            ))}
            {dlFilter !== "all" && (
              <button
                onClick={() => setDlFilter("all")}
                className="px-2 py-0.5 rounded-full text-[11px] text-blue-400 hover:underline"
              >
                清除筛选
              </button>
            )}
          </div>
          <div className="overflow-y-auto" style={{ maxHeight: "42vh" }}>
            {visibleItems.length === 0 ? (
              <p className="py-6 text-center text-xs text-gray-500">
                {todayItems.length === 0 ? "今天还没有下载记录" : "今日没有该状态的记录"}
              </p>
            ) : (
              <table className="w-full text-xs">
                <thead className="sticky top-0 bg-[#141d2b] text-gray-400">
                  <tr>
                    <th className="text-left font-medium py-1.5 px-2">内容</th>
                    <th className="text-left font-medium py-1.5 px-2">接收时间</th>
                    <th className="text-right font-medium py-1.5 px-2">大小</th>
                    <th className="text-left font-medium py-1.5 px-2">类型</th>
                    <th className="text-left font-medium py-1.5 px-2">状态</th>
                  </tr>
                </thead>
                <tbody>
                  {visibleItems.map((item) => (
                    <DlRow key={item.id} item={item} />
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </section>
      )}

    </div>
  );
}