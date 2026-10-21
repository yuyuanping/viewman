import { useState, useCallback, useEffect, useRef } from "react";
import { api } from "../api";
import type { ImageStatsPayload } from "../api";
import type { RescanStatus } from "./useVideos";
import { loadScanRoots } from "../scanRootStore";

/** 库代次推进 + 统计重拉的合并窗口：短时间内多次刷新只拉一次 */
const REFRESH_DEBOUNCE_MS = 400;

/**
 * 图片库数据层。库不再整表下发：前端只持有两样东西——
 * `libraryVersion`（库内容代次，ImageLibrary 靠它重拉当前视图）和
 * `imageStats`（侧栏目录树/计数徽章用的聚合）。目录与搜索过滤都在后端。
 */
export function useImages() {
  const [imageStats, setImageStats] = useState<ImageStatsPayload | null>(null);
  const [libraryVersion, setLibraryVersion] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [rescanStatus, setRescanStatus] = useState<RescanStatus | null>(null);
  const refreshTimer = useRef<number | null>(null);
  /** 等待本轮刷新完成的调用方：后到的 join 先到的同一轮，谁的 Promise 都不会被丢弃 */
  const refreshWaiters = useRef<Array<() => void>>([]);

  useEffect(() => () => {
    if (refreshTimer.current !== null) window.clearTimeout(refreshTimer.current);
  }, []);

  /** 库内容变了：统计重拉、代次前进让 ImageLibrary 重拉视图。
   *  19 万行的视图整表过桥一次要好几秒（查询+序列化+解析+排序），短时间内的
   *  多次推进合并成一次，不然每个扫描根扫完都卡一下。
   *  已有待触发的刷新时后来者直接 join，不再重置计时——重置会清掉先到调用者的
   *  定时器，让它返回的 Promise 永远不 resolve，await 方（删除/移动/扫描）全部挂死。
   *
   *  等待方在推完代次那一刻就放行，不等统计：代次一推，ImageLibrary 自己就重拉视图，
   *  库内容已经是新的。统计只是一张侧栏用的聚合表，它要是被一个大查询卡住（它抢的
   *  是与删除/视图同一把库锁），await 它的删除流程就会永远停在「删除中…」——
   *  图早删掉了，按钮却一直不回来。统计拉不动就留旧值，下轮刷新再试。 */
  const refresh = useCallback(async () => {
    return new Promise<void>(resolve => {
      if (refreshTimer.current !== null) {
        refreshWaiters.current.push(resolve);
        return;
      }
      refreshWaiters.current.push(resolve);
      refreshTimer.current = window.setTimeout(() => {
        refreshTimer.current = null;
        const waiters = refreshWaiters.current;
        refreshWaiters.current = [];
        setLibraryVersion(v => v + 1);
        waiters.forEach(resolve => resolve());
        // 统计与等待方解绑：拉得动就更新侧栏，拉不动下一轮再说
        void (async () => {
          try {
            setImageStats(await api.getImageStats());
          } catch { /* 统计拉不动就先留旧的，下轮刷新再试 */ }
        })();
      }, REFRESH_DEBOUNCE_MS);
    });
  }, []);

  /** 扫描完成后库已在后端更新，这里只负责让前端重新拉取；返回错误信息（null = 成功） */
  const runScan = useCallback(async (dir: string): Promise<string | null> => {
    try {
      const outcome = await api.scanImageDirectory(dir);
      // 库一个字都没变就不重拉：稳定库里自动重扫四个根全都是空手而归，
      // 照旧推代次的话图片页刚打开就被四趟全库重拉卡住十几秒
      if (outcome.items.length > 0 || outcome.removed_ids.length > 0) {
        await refresh();
      }
      return null;
    } catch (e) {
      return `图片扫描失败，未完成更新：${String(e)}`;
    }
  }, [refresh]);

  /** 扫描根由后端在能读到目录时即刻记下，扫到一半被打断，下次启动会自己接着扫 */
  const scanDirectory = useCallback(async (dir: string) => {
    setLoading(true);
    setError(null);
    try {
      const result = await runScan(dir);
      if (result !== null) {
        setError(result);
      }
    } finally {
      setLoading(false);
    }
  }, [runScan]);

  /** 打开应用时自动重新扫描已记录的图片目录；个别目录失败只汇总提示。
   *  失败根 30 秒后自动再试一轮：外置硬盘/加密盘刚登录时还没就绪，首轮全灭不代表真坏了。
   *  每条失败都带上后端原因，不再只报目录名让人猜。 */
  const rescanAll = useCallback(async () => {
    const roots = await loadScanRoots("image");
    if (roots.length === 0) return;
    setLoading(true);
    const failed: Array<{ dir: string; reason: string }> = [];
    try {
      for (let i = 0; i < roots.length; i++) {
        setRescanStatus({ current: i + 1, total: roots.length, dir: roots[i] });
        const reason = await runScan(roots[i]);
        if (reason !== null) failed.push({ dir: roots[i], reason });
      }
    } finally {
      setRescanStatus(null);
      setLoading(false);
    }
    if (failed.length === 0) return;
    // 延迟再试一轮，只试失败的根；成功则清错，免得外置盘每次开机都背一次"失败"
    window.setTimeout(async () => {
      const still: typeof failed = [];
      for (const { dir } of failed) {
        const reason = await runScan(dir);
        if (reason !== null) still.push({ dir, reason });
      }
      if (still.length === 0) {
        setError(null);
      } else {
        setError(describeRescanFailure(still));
      }
    }, 30_000);
    setError(describeRescanFailure(failed));
  }, [runScan]);

  /** 重扫失败汇总：目录 + 后端原因（截断防刷屏），磁盘离线类的提示保留 */
  function describeRescanFailure(failed: Array<{ dir: string; reason: string }>): string {
    const shown = failed.slice(0, 2).map(({ dir, reason }) => {
      const short = reason.replace(/^图片扫描失败，未完成更新：/, "").slice(0, 80);
      return `${dir}（${short}）`;
    }).join("、");
    const more = failed.length > 2 ? `等 ${failed.length} 个目录` : "";
    return `部分图片目录自动扫描失败：${shown}${more}。30 秒后会自动再试一轮；目录若已删除请在侧栏移除，失败目录的库记录不会被清理。`;
  }

  const loadImages = useCallback(async () => {
    setLoading(true);
    try {
      await refresh();
    } catch (e) {
      setError(`读取图片库失败：${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [refresh]);

  const initialRun = useCallback(async () => {
    await loadImages();
    await rescanAll();
  }, [loadImages, rescanAll]);

  const clearError = useCallback(() => setError(null), []);

  return {
    imageStats, libraryVersion, loading, error, rescanStatus, clearError,
    scanDirectory, loadImages, initialRun, refresh,
  };
}
