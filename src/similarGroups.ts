/**
 * 分组审阅（重复图检测）的纯函数。
 * 后端每推一次都是当前的完整分组，所以这里只做"整份替换 + 算保留项"。
 */
import type { Image } from "./types";

export interface SimilarGroup {
  /** 仍在库的组员 id，顺序沿用后端（组内按加入时间升序，首张即默认保留张） */
  ids: string[];
  /** 本组保留的那张（组员之一） */
  keep: string;
}

/**
 * 裁掉已不在库的组员；不足 2 张的组不再算一组；大的组排前面便于先审。
 * 保留张默认是组内最早入库的那张（后端排好的首张）；手动点过「留」的那张例外——人说了才算。
 */
export function liveGroups(
  groups: string[][],
  imageById: Map<string, Image>,
  chosenKeeps?: Set<string>,
): SimilarGroup[] {
  return groups
    .map((group): SimilarGroup => {
      const ids = group.filter(id => imageById.has(id));
      const chosen = ids.find(id => chosenKeeps?.has(id) ?? false);
      return { ids, keep: chosen ?? ids[0] };
    })
    .filter(group => group.ids.length > 1)
    .sort((a, b) => b.ids.length - a.ids.length);
}

/** 各组除保留张之外的组员 */
export function extrasOfGroups(groups: SimilarGroup[]): string[] {
  return groups.flatMap(group => group.ids.filter(id => id !== group.keep));
}

/**
 * 按文件名/路径筛组。几万组靠滚是找不到某一枚的，得能直接查——
 * 也用来自证"这张到底进没进组"。空查询原样返回。
 */
export function groupsMatching(
  groups: SimilarGroup[],
  query: string,
  imageById: Map<string, Image>,
): SimilarGroup[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return groups;
  return groups.filter(group => group.ids.some(id => {
    const image = imageById.get(id);
    if (!image) return false;
    return image.filename.toLowerCase().includes(needle) || image.path.toLowerCase().includes(needle);
  }));
}

/** 换某一组的保留项后应有的勾选：该组取反，其他组保持原样 */
export function selectGroupExtras(current: Iterable<string>, group: SimilarGroup, keep: string): Set<string> {
  const members = new Set(group.ids);
  const next = new Set([...current].filter(id => !members.has(id)));
  for (const id of group.ids) if (id !== keep) next.add(id);
  return next;
}
