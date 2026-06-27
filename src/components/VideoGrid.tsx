import type { Video } from "../types";
import { VideoCard } from "./VideoCard";

interface VideoGridProps {
  videos: Video[];
  progressMap: Record<string, number | null>;
  onPlay: (video: Video) => void;
}

export function VideoGrid({ videos, progressMap, onPlay }: VideoGridProps) {
  if (videos.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-500">
        <div className="text-center">
          <p className="text-lg">暂无视频</p>
          <p className="text-sm mt-1">点击左侧"扫描目录"添加视频</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-4">
        {videos.map(video => (
          <VideoCard
            key={video.id}
            video={video}
            progress={progressMap[video.id] ?? null}
            onPlay={onPlay}
          />
        ))}
      </div>
    </div>
  );
}
