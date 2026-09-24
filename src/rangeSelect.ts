/**
 * 多选连选（shift 点击）的纯函数。
 * 连选区间落在"当前过滤结果的整份清单"上：换目录/改搜索时上层会整体清空勾选，
 * 所以不会出现看不见却被删掉的条目。
 */

export interface Identifiable {
  id: string;
}

/** 锚点到本次点击之间的一段（含两端）；锚点不在这一份清单里时退化成只含本次点击 */
export function rangeBetween<T>(items: T[], anchorIndex: number, targetIndex: number): T[] {
  if (targetIndex < 0 || targetIndex >= items.length) return [];
  if (anchorIndex < 0 || anchorIndex >= items.length) return [items[targetIndex]];
  const [from, to] = anchorIndex <= targetIndex ? [anchorIndex, targetIndex] : [targetIndex, anchorIndex];
  return items.slice(from, to + 1);
}

/** 把一段并入现有勾选（并集，不是对称差：连选只做加法，取消靠单独点击） */
export function unionRange(current: Iterable<string>, range: Iterable<Identifiable>): Set<string> {
  const next = new Set(current);
  for (const item of range) next.add(item.id);
  return next;
}
