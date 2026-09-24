/**
 * 相似图分组（pHash）的纯函数。
 * 后端每组按加入时间升序返回，组内首张即最早入库的原件；但它只是默认保留项，
 * 真正留哪张由人在面板里点「留」来定，改动记在 keeps 里（键为原始组下标）。
 */

export interface SimilarGroup {
  /** 后端返回的原始组下标，用于固定 keeps 的键 */
  at: number;
  /** 仍在库的组员 id，顺序沿用后端 */
  ids: string[];
  /** 本组保留的那张（组员之一） */
  keep: string;
}

/** 裁掉已不在库的组员；不足 2 张的组不再算相似组；大的组排前面便于先审 */
export function liveGroups(groups: string[][], alive: Set<string>, keeps: Record<number, string>): SimilarGroup[] {
  return groups
    .map((group, at): SimilarGroup => {
      const ids = group.filter(id => alive.has(id));
      const chosen = keeps[at];
      return { at, ids, keep: chosen !== undefined && ids.includes(chosen) ? chosen : ids[0] };
    })
    .filter(group => group.ids.length > 1)
    .sort((a, b) => b.ids.length - a.ids.length);
}

/** 各组除保留张之外的组员 */
export function extrasOfGroups(groups: SimilarGroup[]): string[] {
  return groups.flatMap(group => group.ids.filter(id => id !== group.keep));
}

/** 换某一组的保留项后应有的勾选：该组取反，其他组保持原样 */
export function selectGroupExtras(current: Iterable<string>, group: SimilarGroup, keep: string): Set<string> {
  const members = new Set(group.ids);
  const next = new Set([...current].filter(id => !members.has(id)));
  for (const id of group.ids) if (id !== keep) next.add(id);
  return next;
}
