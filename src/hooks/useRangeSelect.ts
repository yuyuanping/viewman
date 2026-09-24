import { useCallback, useRef } from "react";
import type { Identifiable } from "../rangeSelect";
import { rangeBetween, unionRange } from "../rangeSelect";

/**
 * 多选锚点与连选。
 *
 * - 普通点击：以这张为新锚点，切换它的勾选。
 * - Shift 点击：从锚点连选到这张（并集，锚点不动，方便反复调整区间）。
 *
 * 连选的区间落在当前过滤结果的整份清单上；换目录/改搜索时上层会整体清空勾选，
 * 所以不会把看不见的条目留在选中集合里。
 */
export function useRangeSelect<T extends Identifiable>(
  items: T[],
  setSelectedIds: React.Dispatch<React.SetStateAction<Set<string>>>,
) {
  const anchor = useRef<string | null>(null);

  const toggleSelect = useCallback((item: T) => {
    anchor.current = item.id;
    setSelectedIds(prev => {
      const next = new Set(prev);
      if (next.has(item.id)) next.delete(item.id);
      else next.add(item.id);
      return next;
    });
  }, [setSelectedIds]);

  /** 网格第 index 张被点击；shiftKey 决定是连选还是单选 */
  const selectAt = useCallback((index: number, shiftKey: boolean) => {
    const item = items[index];
    if (!item) return;
    if (!shiftKey) {
      toggleSelect(item);
      return;
    }
    const anchorIndex = items.findIndex(candidate => candidate.id === anchor.current);
    setSelectedIds(prev => unionRange(prev, rangeBetween(items, anchorIndex, index)));
  }, [items, setSelectedIds, toggleSelect]);

  return { toggleSelect, selectAt };
}
