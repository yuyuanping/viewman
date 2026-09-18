import type { Video } from "../types";
import { VideoCard } from "./VideoCard";

interface VideoGridProps {
  videos: Video[];
  progressMap: Record<string, number | null>;
  missingIds?: Set<string>;
  onPlay: (video: Video) => void;
  onDeleted: () => void;
}

export function VideoGrid({ videos, progressMap, missingIds, onPlay, onDeleted }: VideoGridProps) {
  if (videos.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-500">
        <div className="text-center">
          <div className="mx-auto mb-5 w-16 h-16 rounded-2xl border border-white/10 bg-gray-800 grid place-items-center text-blue-500"><svg width="28" height="28" viewBox="0 0 24 24" stroke="currentColor" fill="none" strokeWidth="1.5" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="3"/><path d="m10 9 5 3-5 3z"/></svg></div>
          <p className="text-lg text-gray-200">这里还没有视频</p>
          <p className="text-sm mt-2">扫描目录添加视频，或调整搜索条件。</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="video-tiles">
        {videos.map(video => (
          <VideoCard
            key={video.id}
            video={video}
            progress={progressMap[video.id] ?? null}
            missing={missingIds?.has(video.id) ?? false}
            onPlay={onPlay}
            onDeleted={onDeleted}
          />
        ))}
      </div>
    </div>
  );
}
