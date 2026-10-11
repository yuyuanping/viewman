import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "../api";
import type { BotStatus, NapCatStage } from "../types";
import type { Notify } from "../hooks/useToasts";
import { useNapcatStatus } from "../hooks/useNapcatStatus";

function formatBytes(bytes: number | null): string {
  if (bytes == null || bytes <= 0) return "-";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let idx = 0;
  while (value >= 1024 && idx < units.length - 1) {
    value /= 1024;
    idx += 1;
  }
  return `${value.toFixed(value >= 100 || idx === 0 ? 0 : 1)} ${units[idx]}`;
}

/** 下次重试时间（Unix 秒）：busy 行正在下载，其余按相对时间显示 */
function nextTryLabel(nextTryAt: number | null, state: string): string {
  if (state === "busy") return "下载中";
  if (nextTryAt == null || nextTryAt <= 0) return "等待排期";
  const diff = Math.round(nextTryAt - Date.now() / 1000);
  if (diff <= 10) return "即将复检";
  if (diff < 90) return `${Math.ceil(diff / 10) * 10}s 后复检`;
  // 跑满 MAX_DL_TRIES(8) 后转每日复检，会出现上小时的间隔，别显示成“分钟”
  if (diff < 3600) return `约 ${Math.ceil(diff / 600)} 分钟后`;
  if (diff < 86400) return `约 ${Math.ceil(diff / 3600)} 小时后`;
  return `约 ${Math.ceil(diff / 86400)} 天后`;
}

/** 阶段彩色小标签：一眼看出卡在哪一步 */
function StageBadge({ stage }: { stage: NapCatStage }) {
  const map: Record<NapCatStage, { text: string; cls: string }> = {
    ready: { text: "已就绪", cls: "bg-emerald-500/20 text-emerald-300" },
    initializing: { text: "初始化中", cls: "bg-blue-500/20 text-blue-300" },
    waiting_qr: { text: "等扫码", cls: "bg-amber-500/20 text-amber-300" },
    starting: { text: "启动中", cls: "bg-slate-500/20 text-slate-300" },
    error: { text: "报错", cls: "bg-red-500/20 text-red-300" },
    not_running: { text: "未运行", cls: "bg-gray-500/20 text-gray-400" },
  };
  const m = map[stage] ?? map.not_running;
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

/** QQ 存档 Bot 面板：运行状态、启停控制、下载队列进度与 bot 目录/Python 设置 */
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
  const [root, setRoot] = useState("");
  const [python, setPython] = useState("");
  // ref 做轮询回填的同步守卫，state 驱动按钮禁用（改 ref 不会触发重渲染）
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  // 用户手改过输入框后不再被轮询回填覆盖，否则每 10 秒就会跳回磁盘上的旧值
  const dirty = useRef(false);
  const napcat = useNapcatStatus();

  useEffect(() => {
    if (savingRef.current || dirty.current) return;
    setRoot(status?.settings.root ?? "");
    setPython(status?.settings.python ?? "");
  }, [status]);

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

  const handleSaveSettings = async () => {
    if (!root.trim() || !python.trim()) {
      notify("bot 目录与 Python 路径都不能为空", "error");
      return;
    }
    savingRef.current = true;
    setSaving(true);
    try {
      await api.saveBotSettings(root.trim(), python.trim());
      notify("Bot 设置已保存");
      dirty.current = false;
      await refresh();
    } catch (e) {
      notify(`保存失败：${String(e)}`, "error");
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const handlePickRoot = async () => {
    try {
      const dir = await open({ directory: true, multiple: false, title: "选择 QQ Bot 目录（需包含 bot.py）" });
      if (typeof dir === "string") { dirty.current = true; setRoot(dir); }
    } catch (e) {
      notify(`选择目录失败：${String(e)}`, "error");
    }
  };

  // 以锁判定为准：pythonw 空命令行时 PID 读不到，但 bot 确实在跑
  const running = status != null && (status.bot_running || status.bot_pid != null);
  const dl = status?.dl;
  const dlTotal = dl ? dl.ok + dl.pending + dl.busy + dl.dead : 0;
  const dlPercent = dlTotal > 0 && dl ? Math.round((dl.ok / dlTotal) * 100) : 0;
  const events = status?.events;
  const queued = events ? events.new + events.busy : 0;

  // 端口不通时给出可执行的原因：NapCat 在等扫码登录时进程在但端口不开，
  // 这正是“点了启动没反应”的真实原因，不写出来就只能对着四个灰灯猜。
  const hint = !status
    ? null
    : status.bot_running && !status.ws_up
      ? "bot.py 在跑但连不上 NapCat：NapCat 进程已起、端口未开，通常是卡在“等待扫码登录”。"
      + "需要扫码后 9501 才会监听。二维码在 NapCat 目录下 resources\\app\\napcat\\cache\\qrcode.png。"
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

          {napcat.status.recent_errors.length > 0 && (
            <details className="text-xs text-gray-400">
              <summary className="cursor-pointer select-none">最近错误（{napcat.status.recent_errors.length}）</summary>
              <ul className="mt-2 flex flex-col gap-1 max-h-32 overflow-y-auto">
                {napcat.status.recent_errors.map((e, i) => (
                  <li key={i} className="text-red-300/90 break-all">{e}</li>
                ))}
              </ul>
            </details>
          )}

          {napcat.status.webui_url && (
            <a href={napcat.status.webui_url} target="_blank" rel="noreferrer" className="text-[11px] text-blue-400 hover:underline self-start">
              打开 NapCat WebUI
            </a>
          )}
        </section>
      )}

      {status && dl && events && (
        <section className="bg-white/5 rounded-xl p-4 flex flex-col gap-3 shrink-0">
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium">下载进度</span>
            <span className="text-xs text-gray-400 tabular-nums">
              成功 {dl.ok} / 下载中 {dl.busy} / 排队 {dl.pending} / 待复检 {dl.dead}
            </span>
          </div>
          <div className="h-2 rounded-full bg-white/10 overflow-hidden" role="progressbar" aria-valuenow={dlPercent} aria-valuemin={0} aria-valuemax={100}>
            <div className="h-full bg-emerald-500 transition-all" style={{ width: `${dlPercent}%` }} />
          </div>
          <div className="flex items-center justify-between text-xs text-gray-400 flex-wrap gap-1">
            <span>完成率 {dlPercent}%（纯文本消息 {dl.none} 条不计入）</span>
            <span>
              事件积压 {queued}（新 {events.new} / 处理中 {events.busy} / 完成 {events.done} / 出错 {events.error} / 弃 {events.dead}）
            </span>
          </div>
        </section>
      )}

      {status && status.queue.length > 0 && (
        <section className="bg-white/5 rounded-xl p-4 flex flex-col gap-2 min-h-0" style={{ flex: "1 1 260px" }}>
          <div className="flex items-center justify-between">
            <span className="text-sm font-medium">下载队列（{status.queue.length}）</span>
            <span className="text-xs text-gray-400">每 10 秒自动刷新</span>
          </div>
          <div className="overflow-y-auto" style={{ maxHeight: "42vh" }}>
            <table className="w-full text-xs">
              <thead className="sticky top-0 bg-[#141d2b] text-gray-400">
                <tr>
                  <th className="text-left font-medium py-1.5 px-2">类型</th>
                  <th className="text-left font-medium py-1.5 px-2">名称</th>
                  <th className="text-right font-medium py-1.5 px-2">大小</th>
                  <th className="text-right font-medium py-1.5 px-2">重试</th>
                  <th className="text-left font-medium py-1.5 px-2">状态</th>
                  <th className="text-left font-medium py-1.5 px-2">失败原因</th>
                </tr>
              </thead>
              <tbody>
                {status.queue.map((item) => (
                  <tr key={item.id} className="border-t border-white/5 hover:bg-white/5">
                    <td className="py-1.5 px-2 text-gray-300">{item.kind}</td>
                    <td className="py-1.5 px-2 text-gray-200 truncate max-w-[32ch]" title={item.label ?? `#${item.id}`}>
                      {item.label ?? `#${item.id}`}
                    </td>
                    <td className="py-1.5 px-2 text-right tabular-nums text-gray-400">{formatBytes(item.size)}</td>
                    <td className="py-1.5 px-2 text-right tabular-nums text-gray-400">{item.tries}</td>
                    <td className="py-1.5 px-2">
                      <span className={item.dl === "busy" ? "text-blue-300" : item.dl === "dead" ? "text-amber-300" : "text-yellow-300"}>
                        {item.dl === "busy" ? "下载中" : item.dl === "dead" ? "待复检" : "排队"}
                        <span className="text-gray-500"> · {nextTryLabel(item.next_try_at, item.dl)}</span>
                      </span>
                    </td>
                    <td className="py-1.5 px-2 text-gray-400 truncate max-w-[30ch]" title={item.err ?? undefined}>
                      {item.err ?? ""}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}

      {status && status.queue.length === 0 && !loadError && (
        <div className="text-xs text-gray-500 text-center py-6 bg-white/5 rounded-xl">
          {running ? "当前没有待下载或失败的消息" : "Bot 未运行，先点「启动 Bot」"}
        </div>
      )}

      <section className="bg-white/5 rounded-xl p-4 flex flex-col gap-3 shrink-0">
        <span className="text-sm font-medium">设置（保存后立即生效于下次启动）</span>
        <label className="flex flex-col gap-1 text-xs text-gray-400">
          Bot 目录（含 bot.py 与 start.bat）
          <div className="flex gap-2">
            <input
              value={root}
              onChange={(e) => { dirty.current = true; setRoot(e.target.value); }}
              className="flex-1 bg-black/30 rounded-lg px-3 py-1.5 text-sm text-gray-200 outline-none focus:ring-1 focus:ring-blue-500"
              spellCheck={false}
            />
            <button onClick={handlePickRoot} className="px-3 py-1.5 rounded-lg bg-gray-700 hover:bg-gray-600 text-xs">
              浏览…
            </button>
          </div>
        </label>
        <label className="flex flex-col gap-1 text-xs text-gray-400">
          Python 解释器（python.exe）
          <input
            value={python}
            onChange={(e) => { dirty.current = true; setPython(e.target.value); }}
            className="bg-black/30 rounded-lg px-3 py-1.5 text-sm text-gray-200 outline-none focus:ring-1 focus:ring-blue-500"
            spellCheck={false}
          />
        </label>
        <div>
          <button
            onClick={() => void handleSaveSettings()}
            disabled={saving}
            className="px-4 py-1.5 rounded-lg bg-blue-600 hover:bg-blue-500 disabled:opacity-40 text-xs font-medium"
          >
            保存设置
          </button>
        </div>
      </section>
    </div>
  );
}