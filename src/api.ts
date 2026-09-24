import { invoke } from "@tauri-apps/api/core";
import type {
  ConversionResult,
  Image,
  MediaKind,
  PotPlayerStatus,
  RecentlyPlayed,
  ScanOutcome,
  Video,
  VideoFileStatus,
  VideoWithProgress,
} from "./types";

/** 全部 Tauri 命令的类型化封装：组件与 hooks 一律经此调用，不散落裸 invoke */
export const api = {
  getVideos: () => invoke<Video[]>("get_videos"),
  getVideosWithProgress: () => invoke<VideoWithProgress[]>("get_videos_with_progress"),
  getRecentlyPlayed: () => invoke<RecentlyPlayed[]>("get_recently_played"),
  /** 完整播放历史（无条数上限） */
  getPlayHistory: () => invoke<RecentlyPlayed[]>("get_play_history"),
  scanDirectory: (dir: string) => invoke<ScanOutcome<Video>>("scan_directory", { dir }),
  saveProgress: (videoId: string, position: number) =>
    invoke<void>("save_progress", { videoId, position }),
  checkVideoFile: (videoId: string) => invoke<VideoFileStatus>("check_video_file", { videoId }),
  getPlayablePath: (videoId: string) => invoke<string>("get_playable_path", { videoId }),
  convertFakeImages: (videoIds: string[]) =>
    invoke<ConversionResult>("convert_fake_images", { videoIds }),
  findStaticVideos: () => invoke<string[]>("find_static_videos"),
  convertShortVideos: (videoIds: string[]) =>
    invoke<ConversionResult>("convert_short_videos", { videoIds }),
  findHevcVideos: () => invoke<string[]>("find_hevc_videos"),
  convertHevcVideos: (videoIds: string[]) =>
    invoke<ConversionResult>("convert_hevc_videos", { videoIds }),
  findDuplicateVideos: () => invoke<string[][]>("find_duplicate_videos"),
  /** 批量删除视频：一次回收站事务 + 一次库事务，返回真正删掉的 id */
  deleteVideos: (videoIds: string[]) => invoke<string[]>("delete_videos", { videoIds }),
  moveVideo: (videoId: string, targetDir: string) =>
    invoke<string>("move_video", { videoId, targetDir }),
  generateThumbnails: (videoIds: string[]) =>
    invoke<number>("generate_thumbnails", { videoIds }),
  /** 播放器截图：截当前帧存到视频同目录，返回输出路径 */
  captureFrame: (videoId: string, position: number) =>
    invoke<string>("capture_frame", { videoId, position }),
  checkPotplayer: () => invoke<boolean>("check_potplayer"),
  checkFfprobe: () => invoke<boolean>("check_ffprobe"),
  launchPotplayer: (videoPath: string, seek: number | null) =>
    invoke<void>("launch_potplayer", { videoPath, seek }),
  potplayerStatus: (videoPath: string) => invoke<PotPlayerStatus>("potplayer_status", { videoPath }),
  loadScanRoots: (kind: MediaKind) => invoke<string[]>("load_scan_roots", { kind }),
  saveScanRoots: (kind: MediaKind, roots: string[]) => invoke<void>("save_scan_roots", { kind, roots }),
  /** 移除目录：忘掉扫描根并删除库内条目（磁盘文件不动），返回清除的条目数 */
  removeMediaDirectory: (kind: MediaKind, dir: string) =>
    invoke<number>("remove_media_directory", { kind, dir }),
  getImages: () => invoke<Image[]>("get_images"),
  scanImageDirectory: (dir: string) => invoke<ScanOutcome<Image>>("scan_image_directory", { dir }),
  /** 批量删除图片：一次回收站事务 + 一次库事务，返回真正删掉的 id */
  deleteImages: (imageIds: string[]) => invoke<string[]>("delete_images", { imageIds }),
  moveImage: (imageId: string, targetDir: string) =>
    invoke<string>("move_image", { imageId, targetDir }),
  generateImageThumbnails: (imageIds: string[]) =>
    invoke<number>("generate_image_thumbnails", { imageIds }),
  findDuplicateImages: () => invoke<string[][]>("find_duplicate_images"),
  /**
   * 相似图检测（pHash）：汉明距离 ≤ threshold 算一组。
   * 指纹落在库里，只有新图/改过的图才重算，所以改阈值只是重新比对，几秒就回。
   * 除了分组，还带回组内各成员的指纹（拆成两个 32 位），面板据此算"距保留张几位"。
   */
  findSimilarImages: (threshold: number) => invoke<SimilarResult>("find_similar_images", { threshold }),
};

/** 一张图的 64 位指纹，高低 32 位分开传（JS 位运算只有 32 位） */
export interface SimilarHash {
  id: string;
  lo: number;
  hi: number;
}

export interface SimilarResult {
  groups: string[][];
  /** 只含成组的图片：孤张不给，免得整库指纹白传一趟 */
  hashes: SimilarHash[];
}

/** 后端 similar-progress 事件负载 */
export interface SimilarProgressPayload {
  /** 本次需要补算指纹的张数（已缓存的不计在内） */
  processed: number;
  total: number;
  done: boolean;
  /** 这一批碰到过的相似组（完整成员，重叠即视作同一组） */
  groups: string[][];
}

/** 后端 scan-progress / image-scan-progress 事件负载（两套库共用同一形状） */
export interface ScanProgressPayload {
  processed: number;
  total: number;
  done: boolean;
  warnings?: string[];
  /** 扫描摘要（done=true 时携带）：本次新增 / 移除 / 元数据刷新的条目数 */
  summary?: { added: number; removed: number; refreshed: number };
}

/** 后端 hevc-progress 事件负载 */
export interface HevcProgressPayload {
  processed: number;
  total: number;
  done: boolean;
  converted: number;
  failed: number;
}

/** 后端 hevc-detect-progress 事件负载 */
export interface HevcDetectProgressPayload {
  processed: number;
  total: number;
  done: boolean;
}

/** 后端 thumbnail-progress 事件负载 */
export interface ThumbnailProgressPayload {
  processed: number;
  total: number;
  done: boolean;
  generated: number;
  failed: number;
}
