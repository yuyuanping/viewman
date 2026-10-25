import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "../api";
import type { BotStatus, NapCatStage } from "../types";
import type { Notify } from "../hooks/useToasts";
import { useNapcatStatus } from "../hooks/useNapcatStatus";

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

/**
 * QQ 存档的控制与状态面：标题行（4 个进程端口状态点 + 启停按钮）+ NapCat 配置卡。
 * 原本长在 QqBotPanel 顶部，现在挂到侧栏 QQ tab 下，主面板只留下载列表。
 * 这两块共用 status / napcat / notify / refresh，放一起避免 App 往侧栏透传一堆零碎。
 */
export function QqBotControls({
  status,
  notify,
  refresh,
  napcat,
}: {
  status: BotStatus | null;
  notify: Notify;
  refresh: () => Promise<void>;
  napcat: ReturnType<typeof useNapcatStatus>;
}) {
  const [actionBusy, setActionBusy] = useState<string | null>(null);

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

  const handleStop = () => void runAction("stop", () => api.stopBot());

  const handleStopAll = () => {
    if (!window.confirm("全停会结束 bot、风控看门狗、NapCat 与 QQ 进程（杀 QQ 可能需要管理员权限）。继续？")) return;
    void runAction("stopAll", async () => {
      const result = await api.stopAllBot();
      return result ? `全停结果：${result}` : undefined;
    });
  };

  const running = status != null && (status.bot_running || status.bot_pid != null);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-2 flex-wrap">
        <span className="text-sm font-semibold tracking-tight">QQ 存档 Bot</span>
        {status && (
          <div className="flex items-center gap-1.5 flex-wrap">
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
            <StatusDot up={status.riskwatch_pid != null} label="看门狗" title={status.riskwatch_pid != null ? `riskwatch.py 运行中 (PID ${status.riskwatch_pid})` : "riskwatch.py 未运行"} />
            <StatusDot up={status.ws_up} label="WS" title="NapCat WebSocket 端口监听状态" />
            <StatusDot up={status.api_up} label="API" title="NapCat HTTP API 端口监听状态" />
          </div>
        )}
      </div>
      <div className="flex items-center gap-2 flex-wrap">
        <button onClick={handleStart} disabled={actionBusy != null || running} className="px-3 py-1.5 rounded-lg bg-blue-600 hover:bg-blue-500 disabled:opacity-40 text-xs font-medium">
          启动 Bot
        </button>
        <button onClick={handleStop} disabled={actionBusy != null || !running} className="px-3 py-1.5 rounded-lg bg-gray-700 hover:bg-gray-600 disabled:opacity-40 text-xs">
          停止 Bot
        </button>
        <button onClick={handleStopAll} disabled={actionBusy != null} className="px-3 py-1.5 rounded-lg bg-gray-700 hover:bg-gray-600 disabled:opacity-40 text-xs">
          全停（含 QQ）
        </button>
      </div>

      {napcat.status && (
        <section className="bg-white/5 rounded-xl p-3 flex flex-col gap-2">
          <div className="flex items-center justify-between gap-2 flex-wrap">
            <span className="text-xs font-medium flex items-center gap-2">
              NapCat
              <StageBadge stage={napcat.status.stage} />
            </span>
            <span className="text-[11px] text-gray-400 flex items-center gap-1.5 flex-wrap">
              {napcat.status.core_version && <span title="NapCat 内核版本">内核 {napcat.status.core_version}</span>}
              {napcat.status.account && <span title="正在登录的 QQ 号">账号 {napcat.status.account}</span>}
              {napcat.status.pid != null && <span title="NapCatWinBootMain.exe 进程号">PID {napcat.status.pid}</span>}
              <button onClick={() => void napcat.runDiagnose()} disabled={napcat.busy} className="px-1.5 py-0.5 rounded bg-gray-700 hover:bg-gray-600 disabled:opacity-40 text-[11px]">
                体检
              </button>
            </span>
          </div>

          <p className="text-[11px] text-gray-300">{napcat.status.summary}</p>

          {napcat.status.stage === "waiting_qr" && napcat.status.qr_image && (
            <div className="flex flex-col gap-2 items-start">
              <img
                src={convertFileSrc(napcat.status.qr_image)}
                alt="NapCat 登录二维码"
                className="w-40 h-40 object-contain bg-white rounded-lg p-1"
              />
              <span className="text-[11px] text-gray-500">用手机 QQ 扫码并确认；码约 2 分钟过期，过期后会自动换新</span>
            </div>
          )}

          {napcat.diagnose && !napcat.diagnose.healthy && (
            <div className="flex flex-col gap-2 rounded-lg bg-amber-950/60 border border-amber-900/50 p-2">
              <span className="text-[11px] text-amber-200 font-medium">配置体检发现 {napcat.diagnose.issues.length} 个问题</span>
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
                  className="px-2 py-0.5 rounded bg-amber-600 hover:bg-amber-500 disabled:opacity-40 text-[11px]"
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
