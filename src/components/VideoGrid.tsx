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
  onDeleted: () => void;
  onMoved: () => void;
}

export function VideoGrid({ videos, progressMap, missingIds, fakeIds, shortIds, duplicateIds, selectMode, selectedIds, onToggleSelect, onPlay, onDeleted, onMoved }: VideoGridProps) {
  // 与 CSS .video-tiles 的 minmax/gap 对齐；900px 断点换成窄屏值
  const isNarrow = typeof window !== "undefined" && window.innerWidth <= 900;
  const { onScroll, viewportRef, probeRef, slice, padTop, padBottom } =
    useGridWindow(videos.length, isNarrow ? 170 : 200, isNarrow ? 14 : 20);

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
      {/* 探针卡：绝对定位到屏外，量真实高度；不在网格内占位 */}
      <div ref={probeRef} className="absolute overflow-hidden pointer-events-none" style={{ width: 200, left: -9999, top: 0 }} aria-hidden="true">
        <div className="video-tiles" style={{ display: "grid", gridTemplateColumns: "200px" }}>
          <VideoCard
            video={videos[0]}
            progress={progressMap[videos[0].id] ?? null}
            onPlay={() => undefined}
            onDeleted={() => undefined}
            onMoved={() => undefined}
          />
        </div>
      </div>
      <div className="video-tiles">
        {padTop > 0 && <div style={{ height: padTop, gridColumn: "1 / -1" }} aria-hidden="true" />}
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
        {padBottom > 0 && <div style={{ height: padBottom, gridColumn: "1 / -1" }} aria-hidden="true" />}
      </div>
    </div>
  );
}
