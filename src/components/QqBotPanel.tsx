import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "../api";
import type { BotStatus, NapCatStage } from "../types";
import type { Notify } from "../hooks/useToasts";
import { useNapcatStatus } from "../hooks/useNapcatStatus";

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

/** QQ 存档 Bot 面板：运行状态、启停控制与 NapCat 登录状态 */
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

    </div>
  );
}