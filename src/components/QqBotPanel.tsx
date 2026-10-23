import { memo, useMemo, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "../api";
import type { BotDlItem, BotStatus, NapCatStage } from "../types";
import type { Notify } from "../hooks/useToasts";
import { formatDlSize } from "../utils";
import { useNapcatStatus } from "../hooks/useNapcatStatus";
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

/** 阶段文案与配色：模块级常量，别放进组件里每次渲染重建一遍 */
const STAGE_MAP: Record<NapCatStage, { text: string; cls: string }> = {
  ready: { text: "已就绪", cls: "bg-emerald-500/20 text-emerald-300" },
  initializing: { text: "初始化中", cls: "bg-blue-500/20 text-blue-300" },
  waiting_qr: { text: "等扫码", cls: "bg-amber-500/20 text-amber-300" },
  starting: { text: "启动中", cls: "bg-slate-500/20 text-slate-300" },
  error: { text: "报错", cls: "bg-red-500/20 text-red-300" },
  not_running: { text: "未运行", cls: "bg-gray-500/20 text-gray-400" },
};

/** 阶段彩色小标签：一眼看出卡在哪一步 */
function StageBadge({ stage }: { stage: NapCatStage }) {
  const m = STAGE_MAP[stage] ?? STAGE_MAP.not_running;
  return <span className={`px-2 py-0.5 rounded-full text-[11px] font-medium ${m.cls}`}>{m.text}</span>;
}

function StatusDot({ up, label, title }: { up: boolean; label: string; title: string }) {
  return (
    <span className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-white/5 text-xs" title={title}>
      <span className={`w-2 h-2 rounded-full ${up ? "bg-emerald-400" : "bg-gray-500"}`} aria-hidden="true" />
      {label}
    </span>
  );
}

/** QQ 存档 Bot 面板：运行状态、启停控制与 NapCat 登录状态 */
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
      <td className="py-1.5 px-2 text-right tabular-nums text-gray-400">{formatDlSize(item.size)}</td>
      <td className="py-1.5 px-2 text-gray-300">{kindLabel}</td>
      <td className="py-1.5 px-2">
        <span className={STATUS_CLASS[dlStatusOf(item)]}>
          {statusLabel(item)}{dedupSuffix}
        </span>
      </td>
    </tr>
  );
});

export function QqBotPanel({
  notify,
  status,
  loadError,
  refresh,
}: {
  notify: Notify;
  /** 状态与轮询由 App 上的 useBotStatus 常驻提供，面板不自己再起一份定时器 */
  status: BotStatus | null;
  loadError: string | null;
  refresh: () => Promise<void>;
}) {
  const [actionBusy, setActionBusy] = useState<string | null>(null);
  const [dlFilter, setDlFilter] = useState<DlStatus | "all">("all");
  const napcat = useNapcatStatus();

  const runAction = async (name: string, action: () => Promise<string | void>) => {
    setActionBusy(name);
    try {
      const result = await action();
      if (typeof result === "string" && result.trim()) notify(result, "info");
      await refresh();
    } catch (e) {
      notify(String(e), "error");
    } finally {
      setActionBusy(null);
    }
  };

  const handleStart = () =>
    void runAction("start", async () => {
      await api.startBot();
      return "启动中：NapCat 冷启动需等待端口就绪，稍后自动刷新状态";
    });

  const handleStop = () =>
    void runAction("stop", () => api.stopBot());

  const handleStopAll = () => {
    if (!window.confirm("全停会结束 bot、风控看门狗、NapCat 与 QQ 进程（杀 QQ 可能需要管理员权限）。继续？")) return;
    void runAction("stopAll", async () => {
      const result = await api.stopAllBot();
      return result ? `全停结果：${result}` : undefined;
    });
  };

  const running = status != null && (status.bot_running || status.bot_pid != null);

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
      <div className="flex items-center justify-between gap-3 flex-wrap shrink-0">
        <div className="flex items-center gap-3">
          <span className="text-lg font-semibold tracking-tight">QQ 存档 Bot</span>
          {status && (
            <>
              <StatusDot
                up={status.bot_running || status.bot_pid != null}
                label="Bot 进程"
                title={
                  status.bot_pid != null
                    ? `bot.py 运行中 (PID ${status.bot_pid})`
                    : status.bot_running
                      ? "bot.py 运行中（PID 未读取到，不影响运行）"
                      : "bot.py 未运行"
                }
              />
              <StatusDot up={status.riskwatch_pid != null} label="风控看门狗" title={status.riskwatch_pid != null ? `riskwatch.py 运行中 (PID ${status.riskwatch_pid})` : "riskwatch.py 未运行"} />
              <StatusDot up={status.ws_up} label="WS 9501" title="NapCat WebSocket 端口监听状态" />
              <StatusDot up={status.api_up} label="API 9502" title="NapCat HTTP API 端口监听状态" />
            </>
          )}
        </div>
        <div className="flex items-center gap-2">
          <button onClick={handleStart} disabled={actionBusy != null || running} className="px-4 py-2 rounded-lg bg-blue-600 hover:bg-blue-500 disabled:opacity-40 text-sm font-medium">
            启动 Bot
          </button>
          <button onClick={handleStop} disabled={actionBusy != null || !running} className="px-4 py-2 rounded-lg bg-gray-700 hover:bg-gray-600 disabled:opacity-40 text-sm">
            停止 Bot
          </button>
          <button onClick={handleStopAll} disabled={actionBusy != null} className="px-4 py-2 rounded-lg bg-gray-700 hover:bg-gray-600 disabled:opacity-40 text-sm">
            全停（含 QQ）
          </button>
        </div>
      </div>

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

      {napcat.status && (
        <section className="bg-white/5 rounded-xl p-4 flex flex-col gap-3 shrink-0">
          <div className="flex items-center justify-between gap-2 flex-wrap">
            <span className="text-sm font-medium flex items-center gap-2">
              NapCat
              <StageBadge stage={napcat.status.stage} />
            </span>
            <span className="text-xs text-gray-400 flex items-center gap-2 flex-wrap">
              {napcat.status.core_version && <span title="NapCat 内核版本">内核 {napcat.status.core_version}</span>}
              {napcat.status.account && <span title="正在登录的 QQ 号">账号 {napcat.status.account}</span>}
              {napcat.status.pid != null && <span title="NapCatWinBootMain.exe 进程号">PID {napcat.status.pid}</span>}
              <button onClick={() => void napcat.runDiagnose()} disabled={napcat.busy} className="px-2 py-1 rounded bg-gray-700 hover:bg-gray-600 disabled:opacity-40">
                体检配置
              </button>
            </span>
          </div>

          <p className="text-xs text-gray-300">{napcat.status.summary}</p>

          {napcat.status.stage === "waiting_qr" && napcat.status.qr_image && (
            <div className="flex flex-col gap-2 items-start">
              <img
                src={convertFileSrc(napcat.status.qr_image)}
                alt="NapCat 登录二维码"
                className="w-44 h-44 object-contain bg-white rounded-lg p-1"
              />
              <span className="text-[11px] text-gray-500">用手机 QQ 扫码并确认；码约 2 分钟过期，过期后会自动换新</span>
            </div>
          )}

          {napcat.diagnose && !napcat.diagnose.healthy && (
            <div className="flex flex-col gap-2 rounded-lg bg-amber-950/60 border border-amber-900/50 p-3">
              <span className="text-xs text-amber-200 font-medium">配置体检发现 {napcat.diagnose.issues.length} 个问题</span>
              <ul className="text-[11px] text-amber-100/90 list-disc pl-4 flex flex-col gap-1">
                {napcat.diagnose.issues.map((issue, i) => <li key={i}>{issue}</li>)}
              </ul>
              <div>
                <button
                  onClick={() => void napcat.syncConfig().then((diag) => {
                    if (!diag) {
                      notify("配置同步失败，详见上方错误提示", "error");
                      return;
                    }
                    notify(diag.healthy ? "配置已同步，体检通过" : "配置已同步，重启 NapCat 后生效");
                  })}
                  disabled={napcat.busy}
                  className="px-3 py-1 rounded bg-amber-600 hover:bg-amber-500 disabled:opacity-40 text-[11px]"
                >
                  一键同步配置到生效目录
                </button>
              </div>
            </div>
          )}

          {napcat.status.webui_url && (
            <a href={napcat.status.webui_url} target="_blank" rel="noreferrer" className="text-[11px] text-blue-400 hover:underline self-start">
              打开 NapCat WebUI
            </a>
          )}
        </section>
      )}

      {status && (todayItems.length > 0 || status.summary.ok > 0) && (
        <section className="bg-white/5 rounded-xl p-4 flex flex-col gap-2 min-h-0" style={{ flex: "1 1 260px" }}>
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium">下载列表（今日 {todayItems.length} 条）</span>
            <span className="text-xs text-gray-400">
              {status.dl.busy + status.dl.pending > 0 ? "实时刷新中（约 2 秒）" : "每 10 秒自动刷新"}
            </span>
          </div>
          {/* 累计只有「已下载」一个；今日/今日重复跳过/今日失败都按本地自然日归零，
              午夜过后自动从 0 开始。排队与下载中是队列积压长度，不是日计数 */}
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-400">
            <span>
              累计已下载 <span className="text-gray-200 font-medium">{status.summary.ok}</span> 个
              （{formatDlSize(status.summary.ok_bytes)}）
            </span>
            <span>
              今日 <span className="text-gray-200 font-medium">{status.summary.today}</span> 个
              （{formatDlSize(status.summary.today_bytes)}）
            </span>
            {status.summary.dedup_today > 0 && (
              <span>今日重复跳过 {status.summary.dedup_today}</span>
            )}
            {status.dl.pending > 0 && <span className="text-yellow-300">排队 {status.dl.pending}</span>}
            {status.dl.busy > 0 && <span className="text-blue-300">下载中 {status.dl.busy}</span>}
            {status.dl.dead_today > 0 && (
              <span className="text-red-300">今日失败 {status.dl.dead_today}</span>
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