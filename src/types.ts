export interface Video {
  id: string;
  path: string;
  filename: string;
  duration: number | null;
  width: number | null;
  height: number | null;
  file_size: number;
  created_at: string;
  thumbnail_path?: string | null;
}

/** 图片库条目：与视频分表存放，宽高来自 ffprobe，null 表示尚未探测 */
export interface Image {
  id: string;
  path: string;
  filename: string;
  width: number | null;
  height: number | null;
  file_size: number;
  created_at: string;
  thumbnail_path?: string | null;
  /** 文件修改时间（ISO8601），照片整理排序用；老数据可能缺失 */
  modified_at?: string | null;
}

export type MediaKind = "video" | "image";

/** 一次扫描的增量结果：items 覆盖/追加，removed_ids 剔除，两边都空说明这轮没变化 */
export interface ScanOutcome<T> {
  items: T[];
  removed_ids: string[];
}

export interface VideoWithProgress {
  video: Video;
  position: number | null;
}

export interface RecentlyPlayed {
  video: Video;
  position: number;
  updated_at: string;
}

export interface PotPlayerStatus {
  running: boolean;
  position: number | null;
  position_source: "live" | "remembered" | null;
  state: "running" | "unknown" | "stopped";
}

export interface VideoFileStatus {
  status: "readable" | "missing" | "unavailable" | "fake_image" | "error";
  message: string | null;
}

export interface ConversionResult {
  converted: number;
  errors: string[];
}
