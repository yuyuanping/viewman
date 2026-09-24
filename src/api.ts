import { invoke } from "@tauri-apps/api/core";
import type {
  ConversionResult,
  Image,
  MediaKind,
  PotPlayerStatus,
  RecentlyPlayed,
  Video,
  VideoFileStatus,
  VideoWithProgress,
} from "./types";

/** 全部 Tauri 命令的类型化封装：组件与 hooks 一律经此调用，不散落裸 invoke */
export const api = {
  getVideos: () => invoke<Video[]>("get_videos"),
  getVideosWithProgress: () => invoke<VideoWithProgress[]>("get_videos_with_progress"),
  getRecentlyPlayed: () => invoke<RecentlyPlayed[]>("get_recently_played"),
  scanDirectory: (dir: string) => invoke<Video[]>("scan_directory", { dir }),
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
  deleteVideo: (videoId: string) => invoke<void>("delete_video", { videoId }),
  moveVideo: (videoId: string, targetDir: string) =>
    invoke<string>("move_video", { videoId, targetDir }),
  generateThumbnails: (videoIds: string[]) =>
    invoke<number>("generate_thumbnails", { videoIds }),
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
  scanImageDirectory: (dir: string) => invoke<Image[]>("scan_image_directory", { dir }),
  deleteImage: (imageId: string) => invoke<void>("delete_image", { imageId }),
  moveImage: (imageId: string, targetDir: string) =>
    invoke<string>("move_image", { imageId, targetDir }),
  generateImageThumbnails: (imageIds: string[]) =>
    invoke<number>("generate_image_thumbnails", { imageIds }),
  findDuplicateImages: () => invoke<string[][]>("find_duplicate_images"),
};

/** 后端 scan-progress / image-scan-progress 事件负载（两套库共用同一形状） */
export interface ScanProgressPayload {
  processed: number;
  total: number;
  done: boolean;
  warnings?: string[];
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
