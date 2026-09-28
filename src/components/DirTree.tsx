import { useMemo, useState } from "react";
import { buildDirTree, buildDirTreeFromCounts } from "../dirTree";
import type { DirNode, MediaItem } from "../dirTree";

interface DirTreeProps {
  /** 全量清单建树（视频库还在用） */
  items?: MediaItem[];
  /** 「父目录 → 直接文件数」平表建树（图片库，库不再整表下发） */
  dirCounts?: Array<[string, number]>;
  /** 扫描范围：只展示落在这些根内（含根自身）的文件夹；缺省不设限 */
  scopeRoots?: string[];
  rootLabel: string;
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
}

function DirNodeView({ node, depth, selectedDir, onSelectDir }: {
  node: DirNode;
  depth: number;
  selectedDir: string | null;
  onSelectDir: (dir: string | null) => void;
}) {
  const [expanded, setExpanded] = useState(depth < 2);
  const hasChildren = node.children.length > 0;
  const isSelected = node.path === (selectedDir ?? "");

  if (node.path === "") {
    return (
      <>
        <button
          onClick={() => onSelectDir(null)}
          className={`w-full text-left px-2 py-1.5 rounded text-sm flex items-center gap-2 ${isSelected ? "bg-blue-600" : "hover:bg-gray-700"}`}
        >
          <span>📁 {node.name}</span>
          <span className="ml-auto text-xs text-gray-400">{node.itemCount}</span>
        </button>
        {node.children.map(child => (
          <DirNodeView
            key={child.path}
            node={child}
            depth={depth + 1}
            selectedDir={selectedDir}
            onSelectDir={onSelectDir}
          />
        ))}
      </>
    );
  }

  return (
    <>
      <button
        onClick={() => { onSelectDir(node.path); if (hasChildren) setExpanded(!expanded); }}
        className={`w-full text-left px-2 py-1 rounded text-sm flex items-center gap-1 ${isSelected ? "bg-blue-600" : "hover:bg-gray-700"}`}
        style={{ paddingLeft: `${8 + depth * 16}px` }}
      >
        {hasChildren ? (
          <span className="text-xs w-4">{expanded ? "▼" : "▶"}</span>
        ) : (
          <span className="text-xs w-4">📄</span>
        )}
        <span className="truncate flex-1">{node.name}</span>
        <span className="text-xs text-gray-400">{node.itemCount}</span>
      </button>
      {hasChildren && expanded && node.children.map(child => (
        <DirNodeView
          key={child.path}
          node={child}
          depth={depth + 1}
          selectedDir={selectedDir}
          onSelectDir={onSelectDir}
        />
      ))}
    </>
  );
}

export function DirTree({ items, dirCounts, scopeRoots, rootLabel, selectedDir, onSelectDir }: DirTreeProps) {
  const tree = useMemo(
    () => dirCounts
      ? buildDirTreeFromCounts(dirCounts, rootLabel, scopeRoots)
      : buildDirTree(items ?? [], rootLabel, scopeRoots),
    [dirCounts, items, scopeRoots, rootLabel],
  );

  return (
    <div className="flex flex-col gap-0.5 overflow-y-auto flex-1">
      <DirNodeView node={tree} depth={0} selectedDir={selectedDir} onSelectDir={onSelectDir} />
    </div>
  );
}
