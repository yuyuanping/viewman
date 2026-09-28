import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DuplicateProgressPayload, DuplicateReport } from "../api";
import { pruneGroupsToLive, restoredResultStale } from "../detectionCache";
import { useTauriEvent } from "./useTauriEvent";
import type { Notify } from "./useToasts";

interface DuplicateOptions {
  detect: () => Promise<DuplicateReport>;
  /**
   * 把这一批移入回收站，返回真正删掉的 id。
   * 删完之后本地清单怎么更新（就地剔除还是整库重拉）由实现方决定，这里不假设。
   */
  remove: (ids: string[]) => Promise<string[]>;
  /** 提示文案里的媒体名称 */
  unit: string;
  /** 检测要逐张跑 ffmpeg 时才给：进度事件名，不传就没有进度 */
  progressEvent?: string;
  /**
   * 当前库里的条目（只用 id）：恢复上一趟结果时裁掉已经不存在的组员，
   * 以及判断"这份结果之后库里又添过条目"。不给就整份照收、也不提示过期。
   */
  liveItems?: ReadonlyArray<{ id: string }>;
  /** 上一趟检测的落盘结果（没有就是 null）。给了才会在打开应用时恢复出来看 */
  loadCache?: () => Promise<{ libraryCount: number; groups: string[][] } | null>;
  /** 清除结果时一并作废缓存，不然下次打开又原样复活 */
  clearCache?: () => void;
}

/** 重复条目：按内容指纹分组检测，标记每组之外的多余副本并可一键移入回收站 */
export function useDuplicates(
  notify: Notify,
  { detect: detectDuplicates, remove, unit, progressEvent, liveItems, loadCache, clearCache }: DuplicateOptions,
) {
  /** 后端给的原始名单（可能是上一趟存的）：显示前一律照现在的库裁一遍 */
  const [rawGroups, setRawGroups] = useState<string[][]>([]);
  const [detected, setDetected] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [progress, setProgress] = useState<DuplicateProgressPayload | null>(null);
  /** 非 null = 现在看的是上一趟存的分组，值是它记下时的库内记录数；现跑一次就清成 null */
  const [restoredCount, setRestoredCount] = useState<number | null>(null);
  /** 已经自己点过检测了：缓存回得慢的话别把刚跑出来的结果盖掉 */
  const ranLive = useRef(false);
  /** 缓存只消化一次：等库清单到齐再照一遍，免得"库还没加载出来"把恢复误判成空结果 */
  const cacheTaken = useRef(false);
  /** 清缓存的方法每次渲染都是新函数，放 ref 里，免得把 clear/deleteExtras 的依赖搅动 */
  const clearCacheRef = useRef(clearCache);
  useEffect(() => { clearCacheRef.current = clearCache; });

  // 收不到进度只是看不到 x/y，结果照样返回
  useTauriEvent<DuplicateProgressPayload>(progressEvent, (payload) => {
    const { processed, total, stage } = payload;
    setProgress(total > 0 && processed < total ? { processed, total, stage } : null);
  });

  /** 库清单到齐了没：到齐才去读缓存——名单要照着它裁，空清单会把整份结果裁没 */
  const libraryReady = liveItems === undefined || liveItems.length > 0;

  /** 打开应用先接着上一趟的结果看：重复视频检测热跑也要几分钟，不该是"想看就得再跑一遍" */
  useEffect(() => {
    if (!loadCache || !libraryReady || cacheTaken.current) return;
    cacheTaken.current = true;
    let disposed = false;
    loadCache().then(cached => {
      // 缓存回得晚于用户自己点的检测，就别把刚跑出来的结果盖回去
      if (disposed || !cached || cached.groups.length === 0 || ranLive.current) return;
      setRestoredCount(cached.libraryCount);
      setRawGroups(cached.groups);
      setDetected(true);
    }).catch(() => { /* 读不到缓存就是还没跑过检测 */ });
    return () => { disposed = true; };
  }, [loadCache, libraryReady]);

  /**
   * 后端给的名单照现在的库裁一遍：上一趟存下的结果里，这期间删掉/移走的组员还挂着，
   * 不裁就会把"多少组、多少副本"报多。只索引成组的 id，不为整库建一张全量 Set。
   */
  const groups = useMemo(() => {
    if (!liveItems || rawGroups.length === 0) return rawGroups;
    const wanted = new Set(rawGroups.flat());
    const alive = new Set<string>();
    for (const item of liveItems) if (wanted.has(item.id)) alive.add(item.id);
    return pruneGroupsToLive(rawGroups, id => alive.has(id));
  }, [rawGroups, liveItems]);

  // 每组第一个是保留的原件（后端按添加时间升序排好），其余为副本
  const extraIds = useMemo(() => new Set(groups.flatMap(g => g.slice(1))), [groups]);

  /**
   * "已检测"只在还有组可看时算数：恢复出来的名单可能已经被裁空（副本都删过了），
   * 而工具栏按 (detected && 组数>0) / (!detected) 两个条件渲染，留着这块牌子会两个按钮都不出现。
   */
  const duplicatesDetected = detected && groups.length > 0;

  /** 看的是上一趟存的分组，且之后库里又添过条目：这份分组未必含新添的那批 */
  const stale = restoredResultStale(restoredCount, liveItems?.length ?? 0);

  const detect = useCallback(async () => {
    setDetecting(true);
    ranLive.current = true;
    try {
      const { groups: found, skipped = 0 } = await detectDuplicates();
      // 这一份是照着当前库算出来的，不再挂"上次存下的"那块牌子
      setRestoredCount(null);
      setRawGroups(found);
      setDetected(true);
      const extras = found.reduce((n, g) => n + g.length - 1, 0);
      // 解不出画面的条目根本没进比对，不报出来的话"没有重复"会被当成定论
      const caveat = skipped > 0 ? `另有 ${skipped} 个${unit}解不出画面，未参与比对。` : "";
      if (found.length === 0) {
        notify(`没有发现内容相同的重复${unit}。${caveat}`);
        setDetected(false);
      } else {
        notify(`发现 ${found.length} 组重复${unit}，共 ${extras} 个多余副本。${caveat}`);
      }
    } catch (e) {
      notify(`检测重复失败：${String(e)}`, "error");
    } finally {
      setDetecting(false);
      setProgress(null);
    }
  }, [detectDuplicates, notify, unit]);

  const deleteExtras = useCallback(async () => {
    const ids = [...extraIds];
    if (ids.length === 0) return;
    if (!confirm(`每个重复组保留最早添加的一个，将其余 ${ids.length} 个副本移入回收站？`)) return;
    setDeleting(true);
    let ok = 0;
    let failed = 0;
    try {
      const deleted = await remove(ids);
      ok = deleted.length;
      failed = ids.length - deleted.length;
    } catch (e) {
      failed = ids.length;
      notify(`删除重复副本失败：${String(e)}`, "error");
    }
    // 本次会话不再显示结果；盘上的缓存留着——下次启动照现在的库裁一遍再摆出来，
    // 删失败（文件被占用）留下的那些副本就还能看见
    setRawGroups([]);
    setDetected(false);
    notify(failed > 0 ? `已删除 ${ok} 个重复副本，${failed} 个失败（可能被占用）` : `已删除 ${ok} 个重复副本到回收站。`);
    setDeleting(false);
  }, [extraIds, notify, remove, unit]);

  const clear = useCallback(() => {
    setRawGroups([]);
    setDetected(false);
    setRestoredCount(null);
    // 按 ✕ 是"我不认这份结果"：缓存一起作废，不然下次打开又原样复活
    clearCacheRef.current?.();
  }, []);

  return {
    duplicateGroupCount: groups.length,
    duplicateExtrasCount: extraIds.size,
    duplicateIds: extraIds,
    duplicatesDetected,
    detecting,
    progress,
    detect,
    deleting,
    deleteExtras,
    clear,
    stale,
  };
}
