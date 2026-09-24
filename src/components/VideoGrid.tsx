import type { Video } from "../types";
import { VideoCard } from "./VideoCard";
import { useGridWindow } from "../hooks/useGridWindow";

interface VideoGridProps {
  videos: Video[];
  progressMap: Record<string, number | null>;
  missingIds?: Set<string>;
  fakeIds?: Set<string>;
  shortIds?: Set<string>;
  duplicateIds?: Set<string>;
  selectMode?: boolean;
  selectedIds?: Set<string>;
  onToggleSelect?: (video: Video) => void;
  onPlay: (video: Video) => void;
  onDeleted: (videoId: string) => void;
  onMoved: (videoId: string, newPath: string) => void;
  /** 目录/过滤切换时滚动归零并重测网格几何 */
  resetKey: unknown;
}

export function VideoGrid({ videos, progressMap, missingIds, fakeIds, shortIds, duplicateIds, selectMode, selectedIds, onToggleSelect, onPlay, onDeleted, onMoved, resetKey }: VideoGridProps) {
  const { onScroll, viewportRef, gridRef, slice, padTop, padBottom } =
    useGridWindow(videos.length, resetKey);

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

  const [start, end] = slice;

  return (
    <div className="flex-1 overflow-y-auto" ref={viewportRef} onScroll={onScroll}>
      <div className="video-tiles" ref={gridRef}>
        {padTop > 0 && <div data-pad="top" style={{ height: padTop, gridColumn: "1 / -1" }} aria-hidden="true" />}
        {videos.slice(start, end).map(video => (
          <VideoCard
            key={video.id}
            video={video}
            progress={progressMap[video.id] ?? null}
            missing={missingIds?.has(video.id) ?? false}
            fake={fakeIds?.has(video.id) ?? false}
            short={shortIds?.has(video.id) ?? false}
            duplicate={duplicateIds?.has(video.id) ?? false}
            selectMode={selectMode}
            selected={selectedIds?.has(video.id) ?? false}
            onToggleSelect={onToggleSelect}
            onPlay={onPlay}
            onDeleted={onDeleted}
            onMoved={onMoved}
          />
        ))}
        {padBottom > 0 && <div data-pad="bottom" style={{ height: padBottom, gridColumn: "1 / -1" }} aria-hidden="true" />}
      </div>
    </div>
  );
}
