import { useMemo } from "react";
import type { Video } from "../types";
import type { SortDirection, SortField, WatchState } from "../libraryFilter";
import { filterByMedia, filterByWatchState, filterMedia, sortMedia } from "../libraryFilter";

export interface VideoLibraryOptions {
  selectedDir: string | null;
  /** 已防抖的搜索词：调用方传 debouncedQuery，输入过程不重算 */
  searchQuery: string;
  progressMap: Record<string, number | null>;
  watchState: WatchState;
  minSizeGb: string;
  minDurationMin: string;
  minHeight: string;
  sortField: SortField;
  sortDirection: SortDirection;
}

/** 视频库过滤 + 排序 + 聚合：从 App.tsx 抽出的纯派生逻辑，避免 God Component 继续膨胀 */
export function useVideoLibrary(videos: Video[], opts: VideoLibraryOptions) {
  const filteredVideos = useMemo(() => {
    const matched = filterMedia(videos, opts.selectedDir, opts.searchQuery);
    const byWatchState = filterByWatchState(matched, opts.progressMap, (v) => v.id, opts.watchState);
    const byMedia = filterByMedia(byWatchState, {
      minSize: opts.minSizeGb ? Number(opts.minSizeGb) * 1024 ** 3 : null,
      minDuration: opts.minDurationMin ? Number(opts.minDurationMin) * 60 : null,
      minHeight: opts.minHeight ? Number(opts.minHeight) : null,
    });
    return sortMedia(byMedia, opts.sortField, opts.sortDirection);
  }, [videos, opts.selectedDir, opts.searchQuery, opts.progressMap, opts.watchState, opts.minSizeGb, opts.minDurationMin, opts.minHeight, opts.sortField, opts.sortDirection]);

  const videoTotals = useMemo(() => {
    let totalDuration = 0;
    let totalSize = 0;
    for (const v of filteredVideos) {
      totalDuration += v.duration ?? 0;
      totalSize += v.file_size;
    }
    return { totalDuration, totalSize };
  }, [filteredVideos]);

  const withoutThumbnailCount = useMemo(
    () => videos.filter((v) => !v.thumbnail_path).length,
    [videos],
  );

  return { filteredVideos, videoTotals, withoutThumbnailCount };
}
