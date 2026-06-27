import type { Video } from "../types";

interface VideoGridProps {
  videos: Video[];
  progressMap: Record<string, number | null>;
  onPlay: (video: Video) => void;
}

export function VideoGrid({ videos, progressMap, onPlay }: VideoGridProps) {
  return (
    <div className="grid grid-cols-4 gap-4 overflow-y-auto flex-1">
      {videos.map(v => (
        <div
          key={v.id}
          onClick={() => onPlay(v)}
          className="bg-gray-800 rounded p-3 cursor-pointer hover:bg-gray-700"
        >
          <p className="truncate">{v.filename}</p>
          {progressMap[v.id] != null && (
            <span className="text-xs text-gray-400">
              {Math.round(progressMap[v.id]!)}s
            </span>
          )}
        </div>
      ))}
    </div>
  );
}
