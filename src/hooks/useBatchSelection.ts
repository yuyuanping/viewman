import { useCallback, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import type { DeletionReport } from "../api";
import type { MovePrompt } from "../components/MoveTargetsDialog";
import type { MediaKind } from "../types";
import type { Notify } from "./useToasts";

/** 批量移动并发数：8 路过桥，500 张不再串行等半天；进度每完成一张节流到 200ms 推一次 */
const MOVE_CONCURRENCY = 8;

interface UseBatchSelectionOptions<T extends { id: string }> {
  /** 当前视图里的条目：全选/清空勾选都以它为准 */
  view: T[];
  selectedIds: Set<string>;
  setSelectedIds: Dispatch<SetStateAction<Set<string>>>;
  setSelectMode: (on: boolean) => void;
  notify: Notify;
  /** 提示文案里的名词与量词：视频/图片 与 个/张 */
  noun: string;
  measure: string;
  kind: MediaKind;
  /** 批量入回收站，返回三类清单（删掉/路径失效/被锁） */
  trash: (ids: string[]) => Promise<DeletionReport>;
  /** 失败时把 id 翻成用户能认的文件名（可选；不给就只报数量） */
  describe?: (ids: string[]) => Promise<string[]>;
  /** 单个移动，返回移动后的新路径；图片侧没有新路径语义，返回任意非空值即可 */
  moveOne: (id: string, dir: string) => Promise<string>;
  /** 移动全部完成后的本地收尾：视频做本地重定向，图片重拉统计 */
  afterMove?: (moved: Array<[string, string]>) => unknown;
}

/**
 * 网格多选模式的批量动作，视频库与图片库共用：
 * 全选/退出多选、删除所选（回收站 + 失败计数）、「移动到…」对话框 + 逐个移动（失败跳过不中断）。
 * 只负责动作本身；Del/M 快捷键的门禁条件两边不同，由调用方自己接。
 */
export function useBatchSelection<T extends { id: string }>({ view, selectedIds, setSelectedIds, setSelectMode, notify, noun, measure, kind, trash, describe, moveOne, afterMove }: UseBatchSelectionOptions<T>) {
  const [deletingSelected, setDeletingSelected] = useState(false);
  const [movingSelected, setMovingSelected] = useState(false);
  const [moveProgress, setMoveProgress] = useState<{ processed: number; total: number } | null>(null);
  const moveCancelRef = useRef(false);
  // 「移动到…」目录列表对话框的待办：批量移动与其他入口的移动共用一个
  const [movePrompt, setMovePrompt] = useState<MovePrompt | null>(null);

  const cancelMove = useCallback(() => {
    moveCancelRef.current = true;
  }, []);

  const allSelected = view.length > 0 && view.every(item => selectedIds.has(item.id));
  const toggleSelectAll = useCallback(() => {
    setSelectedIds(allSelected ? new Set() : new Set(view.map(item => item.id)));
  }, [allSelected, view, setSelectedIds]);

  const exitSelectMode = useCallback(() => {
    setSelectMode(false);
    setSelectedIds(new Set());
  }, [setSelectMode, setSelectedIds]);

  const handleDeleteSelected = useCallback(async () => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    setDeletingSelected(true);
    let report: DeletionReport | null = null;
    try {
      report = await trash(ids);
    } catch (e) {
      // 命令级失败（读库/删库炸）：后端报错已分阶段说明，直接展示并返回，
      // 不再往下发第二条"被占用"误导——失败原因根本不是占用
      setSelectedIds(new Set());
      notify(`删除失败：${String(e)}`, "error");
      setDeletingSelected(false);
      return;
    }
    // 库记录已清的（删掉 + 路径失效）都脱勾；被锁的留在勾选里，关占用程序后可直接再按 Del
    const gone = new Set([...report.deleted, ...report.stale]);
    setSelectedIds(new Set([...selectedIds].filter(id => !gone.has(id))));
    const parts: string[] = [];
    if (report.deleted.length > 0) parts.push(`已将 ${report.deleted.length} ${measure}${noun}移入回收站`);
    if (report.stale.length > 0) {
      parts.push(`${report.stale.length} ${measure}记录路径已不在磁盘，仅清理了记录（文件若还在别处，重扫可重新入库）`);
    }
    if (report.locked.length > 0) {
      // 点名被锁文件：用户能直接看出是谁占着
      let names: string[] = [];
      try {
        names = await describe?.(report.locked) ?? [];
      } catch {
        names = [];
      }
      const shown = (names.length > 0 ? names : report.locked).slice(0, 3).join("、");
      const more = report.locked.length > 3 ? `等 ${report.locked.length} ${measure}` : "";
      parts.push(`${report.locked.length} ${measure}删不掉（被占用）：${shown}${more}。关闭占用它的程序（资源管理器预览窗格/照片应用）后重试`);
    }
    notify(parts.join("；") + "。", report.locked.length > 0 ? "error" : "info");
    setDeletingSelected(false);
  }, [selectedIds, trash, describe, notify, measure, noun, setSelectedIds]);

  // 多选批量移动：弹出目标目录列表，选定后并发 move，失败只计数不中断（被占用的文件跳过）。
  // 8 并发 + 200ms 进度节流 + 可取消（关对话框即取消本轮剩余）。
  const moveSelectedTo = useCallback(async (dir: string): Promise<boolean> => {
    const ids = [...selectedIds];
    if (ids.length === 0) return false;
    moveCancelRef.current = false;
    setMovingSelected(true);
    setMoveProgress({ processed: 0, total: ids.length });
    const moved: Array<[string, string]> = [];
    let failed = 0;
    let processed = 0;
    let lastEmit = 0;
    let next = 0;
    const worker = async () => {
      while (true) {
        if (moveCancelRef.current) break;
        const i = next++;
        if (i >= ids.length) break;
        const id = ids[i];
        try {
          const newPath = await moveOne(id, dir);
          if (moveCancelRef.current) break;
          moved.push([id, newPath]);
        } catch {
          if (!moveCancelRef.current) failed += 1;
        }
        processed += 1;
        const now = Date.now();
        if (now - lastEmit >= 200 || processed >= ids.length) {
          lastEmit = now;
          setMoveProgress({ processed, total: ids.length });
        }
      }
    };
    await Promise.all(
      Array.from({ length: Math.min(MOVE_CONCURRENCY, ids.length) }, () => worker()),
    );
    await afterMove?.(moved);
    setSelectedIds(new Set());
    const ok = moved.length;
    const cancelled = moveCancelRef.current;
    notify(
      cancelled
        ? `已取消移动：已移动 ${ok} ${measure}，${failed} ${measure}失败`
        : failed > 0 ? `已移动 ${ok} ${measure}，${failed} ${measure}失败（可能被占用或目标重名冲突）` : `已将 ${ok} ${measure}${noun}移动到目标文件夹。`,
      failed > 0 || cancelled ? "error" : "info",
    );
    setMovingSelected(false);
    setMoveProgress(null);
    return true;
  }, [selectedIds, moveOne, afterMove, notify, measure, noun, setSelectedIds]);

  const handleMoveSelected = useCallback(() => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    setMovePrompt({
      kind,
      noun,
      count: ids.length,
      onPick: moveSelectedTo,
    });
  }, [selectedIds, kind, noun, moveSelectedTo, setMovePrompt]);

  return {
    allSelected,
    toggleSelectAll,
    exitSelectMode,
    deletingSelected,
    movingSelected,
    moveProgress,
    cancelMove,
    movePrompt,
    setMovePrompt,
    handleDeleteSelected,
    handleMoveSelected,
    moveSelectedTo,
  };
}
