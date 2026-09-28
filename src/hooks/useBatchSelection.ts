import { useCallback, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import type { MovePrompt } from "../components/MoveTargetsDialog";
import type { MediaKind } from "../types";
import type { Notify } from "./useToasts";

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
  /** 批量入回收站，返回真正删掉的 id */
  trash: (ids: string[]) => Promise<string[]>;
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
export function useBatchSelection<T extends { id: string }>({ view, selectedIds, setSelectedIds, setSelectMode, notify, noun, measure, kind, trash, moveOne, afterMove }: UseBatchSelectionOptions<T>) {
  const [deletingSelected, setDeletingSelected] = useState(false);
  const [movingSelected, setMovingSelected] = useState(false);
  // 「移动到…」目录列表对话框的待办：批量移动与其他入口的移动共用一个
  const [movePrompt, setMovePrompt] = useState<MovePrompt | null>(null);

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
    if (!confirm(`确定将选中的 ${ids.length} ${measure}${noun}移入回收站？`)) return;
    setDeletingSelected(true);
    let ok = 0;
    try {
      ok = (await trash(ids)).length;
    } catch (e) {
      notify(`删除失败：${String(e)}`, "error");
    }
    const failed = ids.length - ok;
    setSelectedIds(new Set());
    notify(failed > 0 ? `已删除 ${ok} ${measure}，${failed} ${measure}失败（可能被占用）` : `已将 ${ok} ${measure}${noun}移入回收站。`, failed > 0 ? "error" : "info");
    setDeletingSelected(false);
  }, [selectedIds, trash, notify, measure, noun, setSelectedIds]);

  // 多选批量移动：弹出目标目录列表，选定后逐个 move，失败只计数不中断（被占用的文件跳过）
  const handleMoveSelected = useCallback(() => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    setMovePrompt({
      kind,
      noun,
      count: ids.length,
      onPick: async (dir) => {
        if (!confirm(`将选中的 ${ids.length} ${measure}${noun}移动到:\n${dir}`)) return false;
        setMovingSelected(true);
        const moved: Array<[string, string]> = [];
        let failed = 0;
        for (const id of ids) {
          try {
            moved.push([id, await moveOne(id, dir)]);
          } catch {
            failed += 1;
          }
        }
        await afterMove?.(moved);
        setSelectedIds(new Set());
        const ok = moved.length;
        notify(failed > 0 ? `已移动 ${ok} ${measure}，${failed} ${measure}失败（可能被占用或目标重名冲突）` : `已将 ${ok} ${measure}${noun}移动到目标文件夹。`, failed > 0 ? "error" : "info");
        setMovingSelected(false);
        return true;
      },
    });
  }, [selectedIds, kind, noun, measure, moveOne, afterMove, notify, setSelectedIds]);

  return {
    allSelected,
    toggleSelectAll,
    exitSelectMode,
    deletingSelected,
    movingSelected,
    movePrompt,
    setMovePrompt,
    handleDeleteSelected,
    handleMoveSelected,
  };
}
