import { useMemo, useState } from "react";
import type { Video } from "../types";

interface DirNode {
  path: string;
  name: string;
  videoCount: number;
  children: DirNode[];
}

function buildDirTree(videos: Video[]): DirNode {
  const root: DirNode = { path: "", name: "所有视频", videoCount: videos.length, children: [] };

  for (const v of videos) {
    const idx = v.path.lastIndexOf("\\");
    if (idx === -1) continue;
    const dir = v.path.slice(0, idx);
    const parts = dir.split("\\");

    let current = root;
    let accumulated = "";
    for (const p of parts) {
      accumulated += (accumulated ? "\\" : "") + p;
      let child = current.children.find(c => c.path === accumulated);
      if (!child) {
        child = { path: accumulated, name: p, videoCount: 0, children: [] };
        current.children.push(child);
      }
      child.videoCount++;
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
  videos: Video[];
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
          <span>📁 所有视频</span>
          <span className="ml-auto text-xs text-gray-400">{node.videoCount}</span>
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
        <span className="text-xs text-gray-400">{node.videoCount}</span>
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

export function DirTree({ videos, selectedDir, onSelectDir }: DirTreeProps) {
  const tree = useMemo(() => buildDirTree(videos), [videos]);

  return (
    <div className="flex flex-col gap-0.5 overflow-y-auto flex-1">
      <DirNodeView node={tree} depth={0} selectedDir={selectedDir} onSelectDir={onSelectDir} />
    </div>
  );
}
