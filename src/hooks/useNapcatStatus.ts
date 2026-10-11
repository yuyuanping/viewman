import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { NapCatDiagnose, NapCatStatus } from "../types";

/** 二维码约每 2 分钟过期，轮询 5 秒能及时看到 NapCat 换新码 */
const POLL_MS = 5000;

/**
 * NapCat 状态轮询，只在 QQ 面板挂载时跑。
 * 与 App 层的 bot 状态轮询分开：bot 要常驻（侧栏徽标），
 * 而 NapCat 这块只在面板里看，没必要给全局状态加一份定时器。
 */
export function useNapcatStatus() {
  const [status, setStatus] = useState<NapCatStatus | null>(null);
  const [diagnose, setDiagnose] = useState<NapCatDiagnose | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const snap = await api.getNapcatStatus();
      setStatus(snap);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const runDiagnose = useCallback(async () => {
    setBusy(true);
    try {
      setDiagnose(await api.napcatDiagnose());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  /** 同步 onebot11 配置到生效目录。返回同步后的体检结果；失败返回 null（错误已入 error） */
  const syncConfig = useCallback(async (): Promise<NapCatDiagnose | null> => {
    setBusy(true);
    try {
      await api.napcatSyncConfig();
      // 同步后重新体检，把修复结果立刻反映到面板
      const diag = await api.napcatDiagnose();
      setDiagnose(diag);
      await refresh();
      return diag;
    } catch (e) {
      setError(String(e));
      return null;
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  useEffect(() => {
    void refresh();
    void runDiagnose();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [refresh, runDiagnose]);

  return { status, diagnose, error, busy, refresh, runDiagnose, syncConfig };
}
