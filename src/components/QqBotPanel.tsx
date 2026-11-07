import { memo, useCallback, useMemo, useState } from "react";
import type { BotDlItem, BotStatus } from "../types";
import { api } from "../api";
import { formatDlSize } from "../utils";
import {
  countByDlStatus,
  filterByDlStatus,
  dlForwardTaskText,
  dlItemLabel,
  dlSpeedOf,
  dlStatusOf,
  forwardGroupStats,
  DL_STATUS_LABEL,
  DL_STATUS_ORDER,
  type DlStatus,
} from "../botDlFilter";

/** 状态文字的配色：行内标签与筛选条共用一套，深浅不同但语义一致 */
const STATUS_CLASS: Record<DlStatus, string> = {
  downloading: "text-blue-300",
  dead: "text-red-300",
  dedup: "text-gray-400",
  ok: "text-emerald-300",
  archived: "text-gray-500",
};

/** 没有状态时的稳定空数组：每次渲染新建 [] 会让下面的 memo 全部失效 */
const EMPTY_ITEMS: BotDlItem[] = [];

/** 历史页签一次拉取的条数（与后端 bot_dl_history 的缺省一致；更早的记录不列出） */
const HISTORY_LIMIT = 500;

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

/** 在传行（dl=busy，「下载中」里真在传字节的那部分）的实时进度串：
 * 总量已知给「42%」，排队行没有进度显示，状态就是纯「下载中」 */
function progressPct(item: BotDlItem): string {
  if (item.dl !== "busy") return "";
  const done = item.dl_bytes ?? 0;
  const total = item.dl_total;
  if (total == null || total <= 0) return "";
  // 转发块聚合进度的 total 恒为 NULL，这里只兜单行；夹到 100 防最后一块超额
  return ` ${Math.min(100, Math.round((done / total) * 100))}%`;
}

/** 在传行的尺寸串：总量已知显示「已下 / 总量」（迅雷式）——Content-Length 优先，
 * 解析期拿到的预估大小（登记时落的 messages.bytes）兜底；都没有只显已落盘字节
 * （转发父行=块内累计）。其余行（含排队）直接显大小 */
function sizeText(item: BotDlItem): string {
  if (item.dl === "busy") {
    const done = item.dl_bytes ?? 0;
    const total = item.dl_total ?? (item.size != null && item.size > 0 ? item.size : null);
    if (total != null && total > 0) return `${formatDlSize(done)} / ${formatDlSize(total)}`;
    return done > 0 ? formatDlSize(done) : "…";
  }
  return formatDlSize(item.size);
}

/** 速度串：dlSpeedOf 给字节/秒（8 秒窗口差分），>0 才显示 */
function speedText(item: BotDlItem): string {
  const s = dlSpeedOf(item);
  return s > 0 ? ` · ${formatDlSize(s)}/s` : "";
}

/** 转发/引用壳行 busy 时的任务态文本（解析中 / 下载中 N/M 个），普通行返回 null */
function taskText(
  item: BotDlItem,
  group: { total: number; done: number } | undefined,
): string | null {
  return dlForwardTaskText(item, group);
}

/** 接收时间串：今天收到的只显 HH:MM 省宽度；跨日的（昨日消息今天复检转死等）补 MM-DD。
 * 转发明细行后端已折算成父转发收到的时刻，不是内层消息的原始发送时间 */
function recvText(item: BotDlItem): string {
  return clockText(item.ts);
}

/** 完成时间串：与接收时间同一套格式。未完成/历史行（当时没记）显示 "-" */
function doneText(item: BotDlItem): string {
  return clockText(item.ok_at);
}

/** 时间戳 → 「今天 HH:MM / 跨日 MM-DD HH:MM」；无值显 "-" */
function clockText(sec: number | null): string {
  if (sec == null) return "-";
  const d = new Date(sec * 1000);
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
const DlRow = memo(function DlRow({
  item,
  group,
  onRetried,
}: {
  item: BotDlItem;
  /** 所属转发块的聚合进度（仅父行会拿到），busy 壳行据此显示 解析中/下载中 N/M 个 */
  group?: { total: number; done: number };
  onRetried: () => void;
}) {
  const kindLabel = KIND_LABEL[item.kind] ?? item.kind;
  const label = dlItemLabel(item);
  const task = taskText(item, group);
  const dedupSuffix = item.dl === "ok" && item.dedup > 1 ? `（${item.dedup} 个重复）` : "";
  // 明细行缩进挂在所属转发行下面（后端已按 COALESCE(parent,id) 排好序）
  const isDetail = item.parent != null;
  // 失败行的手动重试：重置回队列后轮询会自动刷出新状态，这里只管发起
  const [retryState, setRetryState] = useState<"idle" | "busy" | "error">("idle");
  const retry = () => {
    if (retryState === "busy") return;
    setRetryState("busy");
    api
      .retryBotDl([item.id])
      .then(() => {
        setRetryState("idle");
        onRetried();
      })
      .catch(() => setRetryState("error"));
  };
  return (
    <tr
      className="border-t border-white/5 hover:bg-white/5 transition-colors duration-75"
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
      <td className="py-1.5 px-2 whitespace-nowrap tabular-nums text-gray-500">{doneText(item)}</td>
      <td className="py-1.5 px-2 text-right tabular-nums text-gray-400">{sizeText(item)}</td>
      <td className="py-1.5 px-2 text-gray-300">{kindLabel}</td>
      <td className="py-1.5 px-2">
        <span className={STATUS_CLASS[dlStatusOf(item)]}>
          {(task ?? statusLabel(item))}{!task && progressPct(item)}{speedText(item)}{dedupSuffix}
        </span>
        {dlStatusOf(item) === "dead" && (
          <button
            onClick={retry}
            disabled={retryState === "busy"}
            title={
              retryState === "error"
                ? "重置失败（bot 可能在忙），再点一次重试"
                : "重置回下载队列，bot 约 20 秒内接手重下"
            }
            className={`ml-1.5 text-[11px] hover:underline ${
              retryState === "error" ? "text-red-400" : "text-blue-400"
            }`}
          >
            {retryState === "busy" ? "重试中…" : "重试"}
          </button>
        )}
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
  // 「今日 / 历史」页签：今日走快照轮询近实时；历史不限日期，按需一次拉取、不进轮询
  const [scope, setScope] = useState<"today" | "history">("today");
  const [history, setHistory] = useState<{
    loading: boolean;
    error: string | null;
    items: BotDlItem[];
  }>({ loading: false, error: null, items: EMPTY_ITEMS });

  const loadHistory = useCallback(() => {
    setHistory((h) => ({ ...h, loading: true, error: null }));
    api
      .getBotDlHistory(HISTORY_LIMIT)
      .then((items) => setHistory({ loading: false, error: null, items }))
      .catch((e) => setHistory({ loading: false, error: String(e), items: [] }));
  }, []);
  const switchScope = (next: "today" | "history") => {
    setScope(next);
    // 每次切进历史都重拉：快照在背后一直在跑，缓存的历史早已落后
    if (next === "history") loadHistory();
  };

  // 当日列表：后端已按自然日取全量，这里只做前端筛选，切换状态不用回后端。
  // 计数与筛选都走 memo：下载活跃期每 2 秒换一次列表，没必要每轮再全量遍历两遍；
  // 历史页签下列表是静态快照，status 轮询换新不会引发这里的重算
  const todayItems = status?.today_items ?? EMPTY_ITEMS;
  const scopeItems = scope === "today" ? todayItems : history.items;
  const statusCounts = useMemo(() => countByDlStatus(scopeItems), [scopeItems]);
  const visibleItems = useMemo(() => filterByDlStatus(scopeItems, dlFilter), [scopeItems, dlFilter]);
  // 转发块聚合（迅雷式任务视角）：busy 壳行显示 解析中/下载中 N/M 个
  const groupStats = useMemo(() => forwardGroupStats(scopeItems), [scopeItems]);

  // 行重试成功后的刷新：今日页签轮询会自动带出新状态；历史页签是静态快照，得重拉
  const handleRowRetried = useCallback(() => {
    if (scope === "history") loadHistory();
  }, [scope, loadHistory]);

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

      {status && (scope === "history" || todayItems.length > 0 || status.summary.ok > 0) && (
        <section className="bg-white/5 rounded-xl p-4 flex flex-col gap-2 min-h-0 overflow-hidden" style={{ flex: "1 1 260px" }}>
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <span className="text-sm font-medium">
                下载列表（{scope === "today" ? "今日" : "历史"} {scopeItems.length} 条）
              </span>
              <div className="flex rounded-full border border-white/10 text-[11px] overflow-hidden">
                {(["today", "history"] as const).map((s) => (
                  <button
                    key={s}
                    onClick={() => switchScope(s)}
                    aria-pressed={scope === s}
                    className={`px-2 py-0.5 transition-colors ${
                      scope === s ? "bg-blue-600/70 text-white" : "text-gray-400 hover:bg-white/10"
                    }`}
                  >
                    {s === "today" ? "今日" : "历史"}
                  </button>
                ))}
              </div>
            </div>
            {scope === "today" ? (
              <span className="text-xs text-gray-400">
                {status.dl.busy + status.dl.pending > 0 ? "实时刷新中（约 1 秒）" : "每 10 秒自动刷新"}
              </span>
            ) : (
              <span className="text-xs text-gray-400">
                {history.loading ? (
                  "加载中…"
                ) : (
                  <button onClick={loadHistory} className="text-blue-400 hover:underline">
                    刷新
                  </button>
                )}
              </span>
            )}
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
          {scope === "history" && history.error && (
            <div role="alert" className="bg-red-900/80 text-red-100 px-3 py-1.5 rounded-lg text-xs">
              历史记录读取失败：{history.error}
            </div>
          )}
          {/* flex-1 min-h-0 吃满卡片剩余高度：失败行常驻后列表动辄几百行，
              固定 42vh 会把大半页面浪费成空白；表头 sticky 在本滚动容器上 */}
          <div className="overflow-y-auto flex-1 min-h-0">
            {visibleItems.length === 0 ? (
              <p className="py-6 text-center text-xs text-gray-500">
                {scope === "today"
                  ? todayItems.length === 0
                    ? "今天还没有下载记录"
                    : "今日没有该状态的记录"
                  : history.loading
                    ? "加载中…"
                    : history.items.length === 0
                      ? "还没有下载记录"
                      : "没有该状态的记录"}
              </p>
            ) : (
              <table className="w-full text-xs">
                {/* 底缘阴影让滚动内容"从表头下面滑过"；近不透明底与卡片底色一致，sticky 时不露出下层行 */}
                <thead className="sticky top-0 bg-[#182231] text-gray-500 shadow-[0_1px_0_#ffffff0f,0_6px_10px_-6px_#00000080]">
                  <tr>
                    <th className="text-left font-medium py-1.5 px-2">内容</th>
                    <th className="text-left font-medium py-1.5 px-2">接收时间</th>
                    <th className="text-left font-medium py-1.5 px-2">完成</th>
                    <th className="text-right font-medium py-1.5 px-2">大小</th>
                    <th className="text-left font-medium py-1.5 px-2">类型</th>
                    <th className="text-left font-medium py-1.5 px-2">状态</th>
                  </tr>
                </thead>
                <tbody>
                  {visibleItems.map((item) => (
                    <DlRow
                      key={item.id}
                      item={item}
                      group={item.parent == null ? groupStats.get(item.id) : undefined}
                      onRetried={handleRowRetried}
                    />
                  ))}
                </tbody>
              </table>
            )}
          </div>
          {scope === "history" && !history.loading && history.items.length >= HISTORY_LIMIT && (
            <p className="text-[11px] text-gray-500">只显示最近 {HISTORY_LIMIT} 条，更早的记录未列出</p>
          )}
        </section>
      )}

    </div>
  );
}