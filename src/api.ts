import { invoke } from "@tauri-apps/api/core";
import type {
  ConversionResult,
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
  loadScanRoots: () => invoke<string[]>("load_scan_roots"),
  saveScanRoots: (roots: string[]) => invoke<void>("save_scan_roots", { roots }),
};

/** 后端 scan-progress 事件负载 */
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

/** 后端 thumbnail-progress 事件负载 */
export interface ThumbnailProgressPayload {
  processed: number;
  total: number;
  done: boolean;
  generated: number;
  failed: number;
}
