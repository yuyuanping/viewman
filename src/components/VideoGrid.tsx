import { useCallback, useMemo, useRef } from "react";
import type { Video } from "../types";
import { VideoCard } from "./VideoCard";
import { EmptyState } from "./EmptyState";
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
  onToggleSelect?: (index: number, shiftKey: boolean) => void;
  onPlay: (video: Video) => void;
  onDeleted: (videoId: string) => void;
  onMoved: (videoId: string, newPath: string) => void;
  /** 目录/过滤切换时滚动归零并重测网格几何 */
  resetKey: unknown;
}

export function VideoGrid({ videos, progressMap, missingIds, fakeIds, shortIds, duplicateIds, selectMode, selectedIds, onToggleSelect, onPlay, onDeleted, onMoved, resetKey }: VideoGridProps) {
  const { onScroll, viewportRef, gridRef, slice, padTop, padBottom } =
    useGridWindow(videos.length, resetKey);

  // 卡片勾选回调与 ImageGrid 同一套：每张卡绑一个「第几张 + shift」的闭包，
  // 按 id 缓存保证引用稳定（VideoCard 是 memo 的），闭包里经 ref 实时查当前下标
  const indexOf = useMemo(() => {
    const m = new Map<string, number>();
    videos.forEach((v, i) => m.set(v.id, i));
    return m;
  }, [videos]);
  const indexRef = useRef(indexOf);
  indexRef.current = indexOf;
  const onToggleSelectRef = useRef(onToggleSelect);
  onToggleSelectRef.current = onToggleSelect;
  const toggleCache = useRef(new Map<string, (shiftKey: boolean) => void>());
  const toggleFor = useCallback((id: string) => {
    const cache = toggleCache.current;
    let fn = cache.get(id);
    if (!fn) {
      fn = (shiftKey: boolean) => onToggleSelectRef.current?.(indexRef.current.get(id) ?? -1, shiftKey);
      cache.set(id, fn);
      if (cache.size > videos.length * 2) {
        for (const key of cache.keys()) if (!indexOf.has(key)) cache.delete(key);
      }
    }
    return fn;
  }, [videos.length, indexOf]);

  if (videos.length === 0) {
    return (
      <EmptyState
        icon={
          <svg width="30" height="30" viewBox="0 0 24 24" stroke="currentColor" fill="none" strokeWidth="1.5" aria-hidden="true">
            <rect x="3" y="4" width="18" height="16" rx="3" />
            <path d="m10 9 5 3-5 3z" />
          </svg>
        }
        title="这里还没有视频"
        hint="扫描目录添加视频，或调整搜索条件。"
      />
    );
  }

  const [start, end] = slice;

  return (
    <div
      className="flex-1 overflow-y-auto"
      ref={viewportRef}
      onScroll={onScroll}
      role="grid"
      aria-label={`视频网格，共 ${videos.length} 个视频`}
      aria-rowcount={videos.length}
    >
      <div className="video-tiles" ref={gridRef}>
        {padTop > 0 && <div data-pad="top" style={{ height: padTop, gridColumn: "1 / -1" }} aria-hidden="true" />}
        {videos.slice(start, end).map((video) => (
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
            onToggleSelect={toggleFor(video.id)}
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
