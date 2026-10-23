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
  const { selectedDir, searchQuery, progressMap, watchState, minSizeGb, minDurationMin, minHeight, sortField, sortDirection } = opts;
  // 观看状态筛选才需要进度：progressMap 每 15 秒保存进度就换一次引用，
  // 把它放进依赖会让「按文件名/大小/时间」这些常规视图每 15 秒对全库重排一遍
  const needsProgress = watchState !== "all";
  const filteredVideos = useMemo(() => {
    const matched = filterMedia(videos, selectedDir, searchQuery);
    // progressMap 只在 needsProgress 时进依赖（下面的 eslint 豁免是有意的）
    const byWatchState = needsProgress ? filterByWatchState(matched, progressMap, (v) => v.id, watchState) : matched;
    const byMedia = filterByMedia(byWatchState, {
      minSize: minSizeGb ? Number(minSizeGb) * 1024 ** 3 : null,
      minDuration: minDurationMin ? Number(minDurationMin) * 60 : null,
      minHeight: minHeight ? Number(minHeight) : null,
    });
    return sortMedia(byMedia, sortField, sortDirection);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [videos, selectedDir, searchQuery, needsProgress ? progressMap : null, watchState, minSizeGb, minDurationMin, minHeight, sortField, sortDirection]);

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
