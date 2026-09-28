export interface DirNode {
  path: string;
  name: string;
  itemCount: number;
  children: DirNode[];
}

/** 目录树只依赖 path，视频库与图片库共用 */
export interface MediaItem {
  path: string;
}

import { isUnderDir } from "./scanRoots.ts";

/**
 * 目录是否在扫描范围内：等于某个根或落在某个根之下。
 * scopeRoots 为空 = 不设限（全部算在内，保持无根时的原有行为）。
 * 目录树是「扫描范围的导航」：移动到根外的落点文件夹不进树，
 * 但文件仍在库里——总数徽章与搜索不受影响。
 */
export function inScanScope(dir: string, scopeRoots?: string[]): boolean {
  if (!scopeRoots || scopeRoots.length === 0) return true;
  // 补一个结尾分隔符再比：根自身（dir == root）也算在范围内
  return scopeRoots.some(root => isUnderDir(dir + "\\", root));
}

/** 文件路径的直接父目录；没有分隔符的裸文件名返回 null（进不了目录树） */
function parentDirOf(path: string): string | null {
  const idx = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return idx === -1 ? null : path.slice(0, idx);
}

/**
 * 建库内目录树。
 * 查找用一张「完整路径 → 节点」的扁平表：早先在每个节点的 children 里线性 find，
 * 扇出大的库（一个目录下几千张图）会退化成平方级——19 万条实测 100 秒，扁平索引后 0.4 秒。
 */
export function buildDirTree(items: MediaItem[], rootLabel: string, scopeRoots?: string[]): DirNode {
  const scoped = scopeRoots && scopeRoots.length > 0
    ? items.filter(item => {
        const parent = parentDirOf(item.path);
        return parent !== null && inScanScope(parent, scopeRoots);
      })
    : items;
  const root: DirNode = { path: "", name: rootLabel, itemCount: scoped.length, children: [] };
  const byPath = new Map<string, DirNode>([["", root]]);

  for (const item of scoped) {
    const idx = Math.max(item.path.lastIndexOf("\\"), item.path.lastIndexOf("/"));
    if (idx === -1) continue;
    const parts = item.path.slice(0, idx).split(/[\\/]/);

    let parent = root;
    let accumulated = "";
    for (const p of parts) {
      accumulated += (accumulated ? "\\" : "") + p;
      let child = byPath.get(accumulated);
      if (!child) {
        child = { path: accumulated, name: p, itemCount: 0, children: [] };
        byPath.set(accumulated, child);
        parent.children.push(child);
      }
      child.itemCount++;
      parent = child;
    }
  }

  // 排序只需一轮：children 总量与节点数同阶，逐个父节点排序不会再退化为平方
  for (const node of byPath.values()) {
    if (node.children.length > 1) node.children.sort((a, b) => a.name.localeCompare(b.name));
  }

  return root;
}

/**
 * 由后端「父目录 → 该目录直接文件数」的平表建树。
 * 与 buildDirTree 的计数语义完全一致：某个目录的计数 = 直接挂在它下面的文件数
 * + 全部后代目录的文件数。按目录而不是按文件遍历——目录数是千级，文件数是十万级。
 * 平表的键不包含"没有父目录的裸文件名"（后端就跳过了），根计数是平表之和。
 */
export function buildDirTreeFromCounts(dirCounts: Array<[string, number]>, rootLabel: string, scopeRoots?: string[]): DirNode {
  const scoped = scopeRoots && scopeRoots.length > 0
    ? dirCounts.filter(([dir]) => inScanScope(dir, scopeRoots))
    : dirCounts;
  const root: DirNode = { path: "", name: rootLabel, itemCount: 0, children: [] };
  const byPath = new Map<string, DirNode>([["", root]]);

  for (const [dir, count] of scoped) {
    if (!dir) continue;
    const parts = dir.split("\\");

    let parent = root;
    let accumulated = "";
    for (const p of parts) {
      accumulated += (accumulated ? "\\" : "") + p;
      let child = byPath.get(accumulated);
      if (!child) {
        child = { path: accumulated, name: p, itemCount: 0, children: [] };
        byPath.set(accumulated, child);
        parent.children.push(child);
      }
      child.itemCount += count;
      parent = child;
    }
  }

  for (const node of byPath.values()) {
    if (node.children.length > 1) node.children.sort((a, b) => a.name.localeCompare(b.name));
  }

  // 链式累加只走到各目录节点，根节点要单独合计（范围内的）
  root.itemCount = scoped.reduce((n, [, count]) => n + count, 0);
  return root;
}
