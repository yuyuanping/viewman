/**
 * 相似图分组（pHash）的纯函数。
 * 后端边算边推：每条消息只带"这一批碰到过的组"的完整成员，同一组会被反复推送、逐次变大，
 * 所以先用 mergeSimilarGroups 把重叠的并成一条，再算保留项。
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
  /** 合并后清单里的下标，面板拿它当 React key */
  at: number;
  /** 仍在库的组员 id，顺序沿用后端 */
  ids: string[];
  /** 本组保留的那张（组员之一） */
  keep: string;
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
 * 并入增量推来的组：与已有组共享任意组员即视作同一组，成员取并集。
 * 保留原有的组顺序（大的组不会被一条增量消息挤到后面去）。
 */
export function mergeSimilarGroups(existing: string[][], incoming: string[][]): string[][] {
  const out: string[][] = existing.map(group => [...group]);
  /** 组员 id → 所在组下标；被并掉的组留下空槽，下标在整个调用里保持有效 */
  const ownerOf = new Map<string, number>();
  out.forEach((group, index) => {
    for (const id of group) ownerOf.set(id, index);
  });

  for (const group of incoming) {
    const hits = new Set<number>();
    for (const id of group) {
      const index = ownerOf.get(id);
      if (index !== undefined) hits.add(index);
    }
    const targets = [...hits].filter(index => out[index].length > 0);
    const homeIndex = targets.length > 0 ? targets[0] : out.push([]) - 1;
    const home = out[homeIndex];
    const seen = new Set(home);
    for (const id of group) {
      if (!seen.has(id)) {
        seen.add(id);
        home.push(id);
      }
      ownerOf.set(id, homeIndex);
    }
    for (const index of targets.slice(1)) {
      for (const id of out[index]) {
        if (!seen.has(id)) {
          seen.add(id);
          home.push(id);
        }
        ownerOf.set(id, homeIndex);
      }
      out[index] = [];
    }
  }
  return out.filter(group => group.length > 1);
}

/** 裁掉已不在库的组员；不足 2 张的组不再算相似组；大的组排前面便于先审 */
export function liveGroups(
  groups: string[][],
  imageById: Map<string, Image>,
  chosenKeeps?: Set<string>,
  rule: KeepRule = "earliest",
): SimilarGroup[] {
  return groups
    .map((group, at): SimilarGroup => {
      const ids = group.filter(id => imageById.has(id));
      const chosen = ids.find(id => chosenKeeps?.has(id) ?? false);
      return { at, ids, keep: chosen ?? pickKeep(ids, imageById, rule) };
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
