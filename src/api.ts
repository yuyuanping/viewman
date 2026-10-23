import { invoke } from "@tauri-apps/api/core";
import type {
  BotDlSnapshot,
  BotSettings,
  BotStatus,
  ConversionResult,
  Image,
  MediaKind,
  NapCatDiagnose,
  NapCatStatus,
  PotPlayerStatus,
  RecentlyPlayed,
  ScanOutcome,
  Video,
  VideoFileStatus,
  VideoWithProgress,
} from "./types";

/** 全部 Tauri 命令的类型化封装：组件与 hooks 一律经此调用，不散落裸 invoke */
export const api = {
  getVideosWithProgress: () => invoke<VideoWithProgress[]>("get_videos_with_progress"),
  /** 视频库视图：目录前缀 + 文件名搜索已在后端做完 + 分页，大库不再整表过桥 */
  getVideoView: (dir: string | null, search: string, limit = 2000, offset = 0) =>
    invoke<VideoViewPayload>("get_video_view", { dir, search, limit, offset }),
  /** 长任务取消：scan-video/scan-image/hevc/thumb-*，后台循环每批检查 */
  cancelTask: (task: string) => invoke<void>("cancel_task", { task }),
  /** 数据库自检：返回 "ok" 或首条损坏描述 */
  checkDbIntegrity: () => invoke<string>("check_db_integrity"),
  /** 无损重建 FTS 索引（主表不动），返回索引行数描述 */
  rebuildFtsIndexes: () => invoke<string>("rebuild_fts_indexes"),
  /** 库文件路径：主表损坏需手动恢复时用 */
  dbFilePath: () => invoke<string>("db_file_path"),
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
  /** 批量删除视频：返回三类清单（删掉/路径失效/被锁），见 DeletionReport */
  deleteVideos: (videoIds: string[]) => invoke<DeletionReport>("delete_videos", { videoIds }),
  moveVideo: (videoId: string, targetDir: string) =>
    invoke<string>("move_video", { videoId, targetDir }),
  generateThumbnails: (videoIds: string[]) =>
    invoke<number>("generate_thumbnails", { videoIds }),
  /** 启动续跑：登记上次中断留下的孤儿封面并接着生成，返回是否做了恢复 */
  resumeVideoThumbnails: () => invoke<boolean>("resume_thumbnails", { kind: "video" }),
  checkPotplayer: () => invoke<boolean>("check_potplayer"),
  checkFfprobe: () => invoke<boolean>("check_ffprobe"),
  launchPotplayer: (videoPath: string, seek: number | null) =>
    invoke<void>("launch_potplayer", { videoPath, seek }),
  potplayerStatus: (videoPath: string) => invoke<PotPlayerStatus>("potplayer_status", { videoPath }),
  loadScanRoots: (kind: MediaKind) => invoke<string[]>("load_scan_roots", { kind }),
  saveScanRoots: (kind: MediaKind, roots: string[]) => invoke<void>("save_scan_roots", { kind, roots }),
  /** 「移动到…」对话框的目标目录清单：按媒体类型各存一份，对话框里添加/删除 */
  loadMoveTargets: (kind: MediaKind) => invoke<string[]>("load_move_targets", { kind }),
  saveMoveTargets: (kind: MediaKind, targets: string[]) => invoke<void>("save_move_targets", { kind, targets }),
  /** 移除目录：忘掉扫描根并删除库内条目（磁盘文件不动），返回清除的条目数 */
  removeMediaDirectory: (kind: MediaKind, dir: string) =>
    invoke<number>("remove_media_directory", { kind, dir }),
  /** 图片库视图：目录前缀 + 文件名子串过滤已在后端做完，行序未定（排序在前端做） */
  getImageView: (dir: string | null, search: string) =>
    invoke<ImageViewPayload>("get_image_view", { dir, search }),
  /** 图片库统计：总数、缺封面数、每父目录直接文件数（目录树与每根计数的数据源） */
  getImageStats: () => invoke<ImageStatsPayload>("get_image_stats"),
  /** 缺封面图片的 id 集：封面批任务的待办清单 */
  getMissingImageThumbnailIds: () => invoke<string[]>("get_missing_image_thumbnail_ids"),
  /** 按 id 批量取图：检测面板元数据与"id 还活着吗"的收敛判定 */
  getImagesByIds: (ids: string[]) => invoke<Image[]>("get_images_by_ids", { ids }),
  scanImageDirectory: (dir: string) => invoke<ScanOutcome<Image>>("scan_image_directory", { dir }),
  /** 批量删除图片：返回三类清单（删掉/路径失效/被锁），见 DeletionReport */
  deleteImages: (imageIds: string[]) => invoke<DeletionReport>("delete_images", { imageIds }),
  /** 扩展名修正：把内容与扩展名不符的图按文件头就地改名并同步库记录 */
  fixMismatchedExtensions: () =>
    invoke<{ renamed: number; alreadyMatched: number; unrecognized: number; failed: number }>(
      "fix_mismatched_extensions",
    ),
  moveImage: (imageId: string, targetDir: string) =>
    invoke<string>("move_image", { imageId, targetDir }),
  generateImageThumbnails: (imageIds: string[]) =>
    invoke<number>("generate_image_thumbnails", { imageIds }),
  resumeImageThumbnails: () => invoke<boolean>("resume_thumbnails", { kind: "image" }),
  /**
   * 封面路径体检：清掉库里指向"已不在盘上的缓存文件"的死路径（缓存目录被整个清空后，
   * 「缺封面」会一直算出 0、生成按钮一直禁着）。返回两张表各清了多少条；
   * 封面目录整个读不到（可移动盘没插）时一条都不清。
   */
  repairThumbnailPaths: () => invoke<{ images: number; videos: number }>("repair_thumbnail_paths"),
  /** 动图检测：按文件头结构数帧（GIF/APNG/动态 WebP/AVIF 序列），返回多帧图片的 id */
  findAnimatedImages: () => invoke<string[]>("find_animated_images"),
  findDuplicateImages: (pixelVerify: boolean) =>
    invoke<DuplicateReport>("find_duplicate_images", { pixelVerify }),
  /**
   * 模板匹配（以图搜图）：以一张图为模板，返回库内与它的指纹距离（两枚哈希求和）
   * ≤ 48 的全部命中，按距离升序。指纹已缓存的只做一趟一对一比对，缺的现补并落库。
   */
  findImagesLikeTemplate: (imageId: string) =>
    invoke<TemplateMatchResult>("find_images_like_template", { imageId }),
  /**
   * 上一趟检测的落盘结果（没有则 null）。检测一趟动辄几分钟，重启后先看缓存里的组，
   * 数字对不上 libraryCount 就说明库动过、结果不含新增的那批，界面得提示一句。
   */
  getDuplicateCache: () => invoke<DuplicateCache | null>("get_duplicate_cache"),
  /** 视频侧同理：上一趟重复视频检测的落盘结果（打开应用时先恢复出来看） */
  getVideoDuplicateCache: () => invoke<VideoDuplicateCache | null>("get_video_duplicate_cache"),
  /** 面板按 ✕ 清结果：缓存一起删，不然下次打开又被恢复回来 */
  clearDetectionCache: (which: DetectionKind) => invoke<void>("clear_detection_cache", { which }),
  /** QQ Bot：设置读写、状态快照与启停。bot.py 与 riskwatch.py 以子进程运行，状态靠进程/端口/数据库三合一检测 */
  loadBotSettings: () => invoke<BotSettings>("load_bot_settings"),
  saveBotSettings: (root: string, python: string) =>
    invoke<void>("save_bot_settings", { root, python }),
  getBotStatus: () => invoke<BotStatus>("get_bot_status"),
  /** 轻量快照：只读 bot.db，下载活跃期 2 秒一次让列表近实时 */
  getBotDlSnapshot: () => invoke<BotDlSnapshot>("get_bot_dl_snapshot"),
  startBot: () => invoke<void>("start_bot"),
  /** 只停 bot.py 与 riskwatch，QQ/NapCat 不动；返回逐条结果说明 */
  stopBot: () => invoke<string>("stop_bot"),
  /** 全停 = stop.bat：bot + riskwatch + NapCat + QQ，杀 QQ 可能需要管理员权限 */
  stopAllBot: () => invoke<string>("stop_all_bot"),
  /** NapCat 管理层：状态快照（日志解析）、配置体检、一键修复配置漂移 */
  getNapcatStatus: () => invoke<NapCatStatus>("get_napcat_status"),
  napcatDiagnose: () => invoke<NapCatDiagnose>("napcat_diagnose_cmd"),
  napcatSyncConfig: () => invoke<string>("napcat_sync_config"),
};

export type DetectionKind = "duplicate" | "videoDuplicate";

/** 视频库视图的一页：当前目录/搜索条件下的分页行 + 聚合（计数、总大小） */
export interface VideoViewPayload {
  items: Video[];
  total: number;
  total_size: number;
}

/** 批量删除的三类回报：deleted 删掉（含陈旧 id），stale 记录在但磁盘路径已无文件
 *  （库记录一并清理，磁盘无事可做），locked 磁盘有文件但进不了回收站（关占用程序重试） */
export interface DeletionReport {
  deleted: string[];
  stale: string[];
  locked: string[];
}

/** 图片库视图的一页：当前目录/搜索条件下的全部行 + 聚合（计数、总大小） */
export interface ImageViewPayload {
  items: Image[];
  total: number;
  total_size: number;
}

/** 图片库统计：dirs 是「父目录 → 该目录直接文件数」的平表，目录树和每根计数都从它算 */
export interface ImageStatsPayload {
  total: number;
  missing_thumbnails: number;
  dirs: Array<[string, number]>;
}

export interface DuplicateCache {
  libraryCount: number;
  report: DuplicateReport;
  /** 存这份结果时是否比对了像素：老缓存没有该字段，按 true 理解 */
  pixelVerify?: boolean;
}

/** 重复视频的缓存只有分组：视频侧没有指纹面板，不需要回传每条的指纹 */
export interface VideoDuplicateCache {
  libraryCount: number;
  groups: string[][];
}

/** 模板匹配的一条命中：图片 id + 与模板的指纹距离（pHash 与 dHash 的汉明距离之和） */
export interface TemplateMatch {
  id: string;
  distance: number;
}

export interface TemplateMatchResult {
  /** 按距离升序的全部命中（阈值过滤在前端做，改宽容度不必重跑） */
  matches: TemplateMatch[];
  /** 解不出指纹、因此没参与比对的张数 */
  skipped: number;
}

/** 后端 template-progress 事件负载：补算缺失指纹的进度 */
export interface TemplateProgressPayload {
  processed: number;
  total: number;
}

/** 后端 duplicate-progress 事件负载 */
export interface DuplicateProgressPayload {
  /** sigs 阶段是本次需要补算的张数；grouping 阶段是候选桶数（已缓存指纹/像素的不计） */
  processed: number;
  total: number;
  /** sigs = 补算图像指纹，grouping = 逐桶补算缩略像素并定组 */
  stage: string;
  /** 累计解不出画面、因此没参与比对的张数（只有图片检测给） */
  skipped?: number;
  /** 截至这次已定下的完整分组：只在 grouping 阶段有，整份给、前端整份替换 */
  groups?: string[][];
}

/** 重复检测结果：分组 + 解不出画面而被跳过的张数（视频侧还没统计，就不给这个数） */
export interface DuplicateReport {
  groups: string[][];
  skipped?: number;
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
