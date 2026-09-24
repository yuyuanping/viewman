/** 视频与图片共用的目录 + 文件名过滤 */
export function filterMedia<T extends { path: string; filename: string }>(items: T[], directory: string | null, query: string): T[] {
  const prefix = directory === null ? null : directory.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase() + "\\";
  const search = query.trim().toLowerCase();
  return items.filter(item => (prefix === null || item.path.replace(/\//g, "\\").toLowerCase().startsWith(prefix)) && item.filename.toLowerCase().includes(search));
}

export function selectedDirectoryLabel(directory: string | null, allLabel = "所有视频"): string {
  return directory === null ? allLabel : directory.split(/[\\/]/).filter(Boolean).pop() || directory;
}

export type SortField = "filename" | "duration" | "file_size" | "created_at" | "modified_at";
export type SortDirection = "asc" | "desc";
export type WatchState = "all" | "unwatched" | "in_progress" | "finished";

/** 图片没有时长，排序字段缺省时按"值缺失"处理 */
interface SortableVideo {
  filename: string;
  file_size: number;
  created_at: string;
  duration?: number | null;
  /** 图片排序用：文件修改时间，老数据可能缺失 */
  modified_at?: string | null;
}

/** 播放进度视为“已看完”的比例阈值 */
export const FINISHED_RATIO = 0.95;

export function watchStateOf(video: { duration: number | null }, position: number | null | undefined): Exclude<WatchState, "all"> {
  const pos = position ?? 0;
  if (!(pos > 0)) return "unwatched";
  const duration = video.duration;
  if (duration !== null && duration > 0 && pos / duration >= FINISHED_RATIO) return "finished";
  return "in_progress";
}

export function filterByWatchState<T extends { duration: number | null }>(
  videos: T[],
  progressMap: Record<string, number | null>,
  resolveId: (video: T) => string,
  state: WatchState,
): T[] {
  if (state === "all") return videos;
  return videos.filter(video => watchStateOf(video, progressMap[resolveId(video)]) === state);
}

/** 高级过滤条件：null/undefined 表示不过滤；数值都是下限（>，含等） */
export interface MediaFilter {
  /** 时长下限（秒） */
  minDuration?: number | null;
  /** 大小下限（字节） */
  minSize?: number | null;
  /** 高度下限（像素，1080p 档 = 1080） */
  minHeight?: number | null;
}

export function filterByMedia<T extends { duration?: number | null; file_size: number; height?: number | null }>(
  items: T[],
  filter: MediaFilter | null,
): T[] {
  if (!filter) return items;
  return items.filter(item => {
    if (filter.minDuration != null) {
      // 时长未知（可能图片伪装）的条目在时长过滤下直接排除
      if (item.duration == null || item.duration < filter.minDuration) return false;
    }
    if (filter.minSize != null && item.file_size < filter.minSize) return false;
    if (filter.minHeight != null) {
      if (item.height == null || item.height < filter.minHeight) return false;
    }
    return true;
  });
}

export function sortMedia<T extends SortableVideo>(items: T[], field: SortField, direction: SortDirection): T[] {
  const factor = direction === "asc" ? 1 : -1;
  const compare = (a: T, b: T) => {
    if (field === "filename") return factor * a.filename.localeCompare(b.filename, "zh-Hans-CN", { numeric: true });
    if (field === "created_at") return factor * a.created_at.localeCompare(b.created_at);
    if (field === "modified_at") {
      // 修改时间缺失（老库未迁移）的条目恒排末尾
      const left = a.modified_at ?? null;
      const right = b.modified_at ?? null;
      if (left === null && right === null) return 0;
      if (left === null) return 1;
      if (right === null) return -1;
      return factor * left.localeCompare(right);
    }
    const left = field === "duration" ? a.duration ?? null : a.file_size;
    const right = field === "duration" ? b.duration ?? null : b.file_size;
    // 缺失值恒排在末尾，避免切换方向时混入列表顶部
    if (left === null && right === null) return 0;
    if (left === null) return 1;
    if (right === null) return -1;
    return factor * (left - right);
  };
  return [...items].sort(compare);
}
