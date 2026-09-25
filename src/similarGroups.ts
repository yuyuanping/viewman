/**
 * 相似图分组（pHash）的纯函数。
 * 后端每推一次都是当前的完整分组（口径换了组会缩小，不能并），所以这里只做"整份替换 + 算保留项"。
 * 组里还挂着一些远亲：它们有 ≤阈值 的邻居、只是没连上骨架，看得见但不自动勾。
 */
import type { Image } from "./types";

/**
 * 每组默认保留哪张：
 * earliest 留最早入库那张（后端组内按加入时间升序排出，首张即它）；
 * highest / largest 按分辨率、文件大小挑主，套图里通常想留的是清晰那张而不是先来那张。
 * 规则只管默认值，人在面板里点「留」覆盖的那张记在 chosenKeeps 里，优先级更高。
 */
export type KeepRule = "earliest" | "highest" | "largest";

export const KEEP_RULES: { value: KeepRule; label: string }[] = [
  { value: "earliest", label: "留最早入库" },
  { value: "highest", label: "留分辨率最大" },
  { value: "largest", label: "留文件最大" },
];

/** 后端把 64 位指纹拆成两个 32 位发来（JS 的位运算只有 32 位） */
export type HashPair = [lo: number, hi: number];

function popcount32(x: number): number {
  let n = x >>> 0;
  n = n - ((n >>> 1) & 0x55555555);
  n = (n & 0x33333333) + ((n >>> 2) & 0x33333333);
  n = (n + (n >>> 4)) & 0x0f0f0f0f;
  return (n * 0x01010101) >>> 24;
}

/** 两枚指纹相差的位数：0 是同一张，越大越不像；缺指纹时给 null 让调用方省掉这块 UI */
export function hashDistance(a?: HashPair, b?: HashPair): number | null {
  if (!a || !b) return null;
  return popcount32(a[0] ^ b[0]) + popcount32(a[1] ^ b[1]);
}

export interface SimilarGroup {
  /** 仍在库的组员 id，顺序沿用后端 */
  ids: string[];
  /** 本组保留的那张（组员之一，默认不从远亲里挑） */
  keep: string;
  /** 挂在组尾的远亲：有邻居但没连上骨架，展示出来但不自动勾 */
  far: string[];
}

/** 按规则挑本组的保留张；读不到元数据时退回组内首张 */
export function pickKeep(ids: string[], imageById: Map<string, Image>, rule: KeepRule): string {
  let best = ids[0];
  if (rule === "earliest" || best === undefined) return best;
  let bestScore = -1;
  for (const id of ids) {
    const image = imageById.get(id);
    if (!image) continue;
    const score = rule === "largest" ? image.file_size : (image.width ?? 0) * (image.height ?? 0);
    if (score > bestScore) {
      bestScore = score;
      best = id;
    }
  }
  return best;
}

/**
 * 裁掉已不在库的组员；不足 2 张的组不再算相似组；大的组排前面便于先审。
 * 默认保留张只在骨架成员里挑：远亲本来就比阈值松一档，让它当主会把整组带偏。
 * 手动点过「留」的那张例外——人说了才算。
 */
export function liveGroups(
  groups: string[][],
  imageById: Map<string, Image>,
  chosenKeeps?: Set<string>,
  rule: KeepRule = "earliest",
  farIds?: Set<string>,
): SimilarGroup[] {
  return groups
    .map((group): SimilarGroup => {
      const ids = group.filter(id => imageById.has(id));
      const far = farIds ? ids.filter(id => farIds.has(id)) : [];
      const farSet = new Set(far);
      // 组员几乎都退库了、只剩远亲：那就没得挑，退回全表
      const pool = farSet.size > 0 && farSet.size < ids.length ? ids.filter(id => !farSet.has(id)) : ids;
      const chosen = ids.find(id => chosenKeeps?.has(id) ?? false);
      return { ids, keep: chosen ?? pickKeep(pool, imageById, rule), far };
    })
    .filter(group => group.ids.length > 1)
    .sort((a, b) => b.ids.length - a.ids.length);
}

/** 各组除保留张之外的组员：远亲不算副本，自动勾上它等于把"看着不太像"的也一起删 */
export function extrasOfGroups(groups: SimilarGroup[]): string[] {
  return groups.flatMap(group =>
    group.ids.filter(id => id !== group.keep && !group.far.includes(id)),
  );
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

/** 换某一组的保留项后应有的勾选：该组取反，其他组保持原样（远亲仍不自动勾） */
export function selectGroupExtras(current: Iterable<string>, group: SimilarGroup, keep: string): Set<string> {
  const members = new Set(group.ids);
  const next = new Set([...current].filter(id => !members.has(id)));
  for (const id of group.ids) if (id !== keep && !group.far.includes(id)) next.add(id);
  return next;
}
