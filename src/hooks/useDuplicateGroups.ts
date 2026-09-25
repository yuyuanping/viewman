import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { DuplicateProgressPayload } from "../api";
import { restoredResultStale } from "../detectionCache";
import type { Image } from "../types";
import { extrasOfGroups, liveGroups, selectGroupExtras } from "../similarGroups";
import type { Notify } from "./useToasts";

interface DuplicateGroupOptions {
  images: Image[];
  setSelectMode: (on: boolean) => void;
  setSelectedIds: Dispatch<SetStateAction<Set<string>>>;
  notify: Notify;
}

/** 进度里带上的阶段与累计跳过的张数 */
export interface DuplicateRunProgress {
  processed: number;
  total: number;
  stage: string;
  skipped: number;
}

const NO_PROGRESS = null;

/**
 * 重复图片（内容相同）的分组审阅：后端边核对候选桶边推当前的完整分组，这里整份换掉。
 * 判定按"双指纹相同 + 画面核对"，保留张是组内最早入库那张，其余自动勾进多选。
 * 勾选走增量：只补新冒出来的副本，用户手动取消过的不会被擅自勾回去。
 */
export function useDuplicateGroups({ images, setSelectMode, setSelectedIds, notify }: DuplicateGroupOptions) {
  const [groups, setGroups] = useState<string[][]>([]);
  const [chosenKeeps, setChosenKeeps] = useState<Set<string>>(new Set());
  const [skipped, setSkipped] = useState(0);
  const [detected, setDetected] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [progress, setProgress] = useState<DuplicateRunProgress | null>(NO_PROGRESS);
  const [panelOpen, setPanelOpen] = useState(false);
  /** 从缓存恢复的那一轮别自动勾副本：那是上一趟的结果，凭什么一打开就把人待删清单填满 */
  const [autoTick, setAutoTick] = useState(true);
  /** 非 null = 现在看的是上一趟存的分组，值是它记下时的库内记录数；现跑一次就清成 null */
  const [restoredCount, setRestoredCount] = useState<number | null>(null);
  const autoSelected = useRef<Set<string>>(new Set());
  /** 这一轮是不是已经自己点过检测了：缓存回得慢的话别把刚跑出来的结果盖掉 */
  const ranLive = useRef(false);

  /** 只索引成组的图片，避免为 19 万条清单建一张全库 Map */
  const imageById = useMemo(() => {
    const wanted = new Set(groups.flat());
    if (wanted.size === 0) return new Map<string, Image>();
    const byId = new Map<string, Image>();
    for (const image of images) if (wanted.has(image.id)) byId.set(image.id, image);
    return byId;
  }, [images, groups]);

  const aliveGroups = useMemo(
    () => liveGroups(groups, imageById, chosenKeeps),
    [groups, imageById, chosenKeeps],
  );
  const extras = useMemo(() => extrasOfGroups(aliveGroups), [aliveGroups]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<DuplicateProgressPayload>("duplicate-progress", (event) => {
      const { processed, total, stage, skipped: gone, groups: fresh } = event.payload;
      setProgress(total > 0 && processed < total ? { processed, total, stage, skipped: gone ?? 0 } : NO_PROGRESS);
      if (fresh && fresh.length > 0) {
        // 一出组就算有结果：中途关掉面板还能从工具栏点回去，不必等整趟跑完
        setDetected(true);
        setGroups(fresh);
        setSkipped(gone ?? 0);
      }
    }).then((fn) => {
      if (disposed) fn(); else unlisten = fn;
    }).catch(() => { /* 事件收不到只是看不到中间进度，检测结束时仍会拿到完整结果 */ });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  /** 打开应用先接着上一趟的结果看：重复检测热跑也要几分钟，不该是"想看就得再跑一遍" */
  useEffect(() => {
    api.getDuplicateCache().then(cached => {
      if (!cached || cached.report.groups.length === 0 || ranLive.current) return;
      setAutoTick(false);
      setRestoredCount(cached.libraryCount);
      setGroups(cached.report.groups);
      setSkipped(cached.report.skipped ?? 0);
      setDetected(true);
    }).catch(() => { /* 读不到缓存就是还没跑过检测 */ });
  }, []);

  /** 新出现的副本补勾上，已经不算副本的撤勾；沿用上一轮的勾就等于按新结果删错图 */
  useEffect(() => {
    // 缓存恢复的那一份只记账不勾选，免得稍后把这几张当成"新冒出来的"补勾上去
    if (!autoTick) {
      for (const id of extras) autoSelected.current.add(id);
      return;
    }
    const want = new Set(extras);
    const fresh = extras.filter(id => !autoSelected.current.has(id));
    const stale = [...autoSelected.current].filter(id => !want.has(id));
    if (fresh.length === 0 && stale.length === 0) return;
    for (const id of fresh) autoSelected.current.add(id);
    for (const id of stale) autoSelected.current.delete(id);
    if (fresh.length > 0) setSelectMode(true);
    setSelectedIds(prev => {
      const next = new Set(prev);
      for (const id of fresh) next.add(id);
      for (const id of stale) next.delete(id);
      return next;
    });
  }, [extras, autoTick, setSelectedIds, setSelectMode]);

  const detect = useCallback(async () => {
    setDetecting(true);
    setAutoTick(true);
    ranLive.current = true;
    setGroups([]);
    setChosenKeeps(new Set());
    setSkipped(0);
    autoSelected.current = new Set();
    setPanelOpen(true);
    try {
      const found = await api.findDuplicateImages();
      setRestoredCount(null);
      setSkipped(found.skipped ?? 0);
      if (found.groups.length === 0) {
        setDetected(false);
        setPanelOpen(false);
        // 解不出画面的条目根本没进比对，不报出来的话"没有重复"会被当成定论
        notify(found.skipped ? `没有发现内容相同的重复图片。另有 ${found.skipped} 张解不出画面，未参与比对。` : "没有发现内容相同的重复图片。");
        return;
      }
      setGroups(found.groups);
      setDetected(true);
      const copies = found.groups.reduce((n, g) => n + g.length - 1, 0);
      notify(`检测完成：${found.groups.length} 组重复图片，副本已勾上 ${copies} 张${found.skipped ? `，另有 ${found.skipped} 张解不出画面未参与比对` : ""}。`);
    } catch (e) {
      notify(`检测重复失败：${String(e)}`, "error");
    } finally {
      setDetecting(false);
      setProgress(NO_PROGRESS);
    }
  }, [notify]);

  const clear = useCallback(() => {
    setGroups([]);
    setChosenKeeps(new Set());
    setSkipped(0);
    autoSelected.current = new Set();
    setDetected(false);
    setPanelOpen(false);
    setRestoredCount(null);
    setAutoTick(true);
    // 面板上按"清空结果"就是连缓存一起作废，不然下次打开又原样复活
    api.clearDetectionCache("duplicate").catch(() => { /* 清不掉只是重启后还能看到旧结果 */ });
  }, []);

  /** 换某组的保留项：该组勾选跟着翻转，其他组的勾选不动 */
  const keep = useCallback((fromKeep: string, keepId: string) => {
    const group = aliveGroups.find(item => item.keep === fromKeep);
    if (!group) return;
    setChosenKeeps(prev => {
      const next = new Set(prev);
      next.delete(group.keep);
      next.add(keepId);
      return next;
    });
    setSelectedIds(prev => selectGroupExtras(prev, group, keepId));
  }, [aliveGroups, setSelectedIds]);

  /** 每组只留当前标着「保留」的那张，其余全部勾上 */
  const autoSelect = useCallback(() => {
    setAutoTick(true);
    setSelectMode(true);
    setSelectedIds(new Set(extras));
    for (const id of extras) autoSelected.current.add(id);
  }, [extras, setSelectMode, setSelectedIds]);

  const clearSelection = useCallback(() => {
    setSelectedIds(new Set());
  }, [setSelectedIds]);

  /** 整组勾上/取消：勾上时保留张也在里面，全组一起进回收站是有意为之 */
  const setGroupSelection = useCallback((ids: string[], on: boolean) => {
    if (on) setSelectMode(true);
    setSelectedIds(prev => {
      const next = new Set(prev);
      for (const id of ids) {
        if (on) next.add(id); else next.delete(id);
      }
      return next;
    });
  }, [setSelectMode, setSelectedIds]);

  /** 组内除保留张之外的副本：网格上标"重复副本"的就是这一批 */
  const duplicateIds = useMemo(() => new Set(extras), [extras]);

  /** 看的是上一趟存的分组，且之后库里又添过图：这份分组未必含新添的那批（少掉的不算问题） */
  const stale = restoredResultStale(restoredCount, images.length);

  return {
    groups: aliveGroups,
    imageById,
    duplicateIds,
    groupCount: aliveGroups.length,
    skipped,
    stale,
    detected,
    detecting,
    progress,
    panelOpen,
    setPanelOpen,
    detect,
    clear,
    keep,
    autoSelect,
    clearSelection,
    setGroupSelection,
  };
}
