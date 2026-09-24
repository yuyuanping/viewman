import { useMemo, useState } from "react";

interface DirNode {
  path: string;
  name: string;
  itemCount: number;
  children: DirNode[];
}

/** 目录树只依赖 path，视频库与图片库共用 */
interface MediaItem {
  path: string;
}

function buildDirTree(items: MediaItem[], rootLabel: string): DirNode {
  const root: DirNode = { path: "", name: rootLabel, itemCount: items.length, children: [] };

  for (const item of items) {
    const idx = Math.max(item.path.lastIndexOf("\\"), item.path.lastIndexOf("/"));
    if (idx === -1) continue;
    const dir = item.path.slice(0, idx);
    const parts = dir.split(/[\\/]/);

    let current = root;
    let accumulated = "";
    for (const p of parts) {
      accumulated += (accumulated ? "\\" : "") + p;
      let child = current.children.find(c => c.path === accumulated);
      if (!child) {
        child = { path: accumulated, name: p, itemCount: 0, children: [] };
        current.children.push(child);
      }
      child.itemCount++;
      current = child;
    }
  }

  function sortTree(node: DirNode) {
    node.children.sort((a, b) => a.name.localeCompare(b.name));
    node.children.forEach(sortTree);
  }
  sortTree(root);

  return root;
}

interface DirTreeProps {
  items: MediaItem[];
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

export function DirTree({ items, rootLabel, selectedDir, onSelectDir }: DirTreeProps) {
  const tree = useMemo(() => buildDirTree(items, rootLabel), [items, rootLabel]);

  return (
    <div className="flex flex-col gap-0.5 overflow-y-auto flex-1">
      <DirNodeView node={tree} depth={0} selectedDir={selectedDir} onSelectDir={onSelectDir} />
    </div>
  );
}
