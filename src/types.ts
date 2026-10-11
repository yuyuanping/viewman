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

export type MediaKind = "video" | "image" | "qq";

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

/** QQ Bot 面板：bot 根目录与 Python 解释器，默认值在后端内置 */
export interface BotSettings {
  root: string;
  python: string;
}

/** 下载进度口径：messages 表按 dl 状态计数（dl=none 的纯文本行不占下载槽） */
export interface BotDlStats {
  none: number;
  pending: number;
  busy: number;
  ok: number;
  dead: number;
}

/** events 表按处理状态计数：消息先入这张表才被消费，堆积即积压 */
export interface BotEventStats {
  new: number;
  busy: number;
  done: number;
  error: number;
  dead: number;
}

/** 下载队列一行：待下载 / 下载中 / 失败待重试的消息 */
export interface BotQueueItem {
  id: number;
  kind: string;
  label: string | null;
  size: number | null;
  tries: number;
  /** Unix 秒：下次重试时间（busy 行正在下载，此值是上次排定的） */
  next_try_at: number | null;
  /** 最近一次失败原因（截断 120 字） */
  err: string | null;
  dl: string;
}

/** 一次状态快照：进程 + NapCat 端口 + bot.db 只读统计与下载队列，字段名与 Rust 端一致（snake_case） */
export interface BotStatus {
  settings: BotSettings;
  bot_pid: number | null;
  riskwatch_pid: number | null;
  /** bot 是否在跑（锁 bot.lock 判定，比命令行可靠）；PID 读不到时也为 true */
  bot_running: boolean;
  /** NapCat WS(9501)/HTTP(9502) 端口是否在监听 = QQ 协议层活着 */
  ws_up: boolean;
  api_up: boolean;
  dl: BotDlStats;
  events: BotEventStats;
  queue: BotQueueItem[];
  /** bot.db 不存在或打不开时给提示，不影响其余状态 */
  db_note: string | null;
}

/** NapCat 启动/登录阶段（snake_case，与 Rust 端一致） */
export type NapCatStage =
  | "not_running"
  | "starting"
  | "waiting_qr"
  | "initializing"
  | "ready"
  | "error";

/** NapCat 状态快照：把日志折叠成“现在卡在哪一步” */
export interface NapCatStatus {
  stage: NapCatStage;
  pid: number | null;
  /** 正在登录的 QQ 号 */
  account: string | null;
  /** 二维码图片路径；waiting_qr 时有，面板直接渲染 */
  qr_image: string | null;
  /** 一句话说清现在的情况 */
  summary: string;
  recent_errors: string[];
  core_version: string | null;
  webui_url: string | null;
}

export interface NapCatConfigEntry {
  file: string;
  path: string;
  account: string | null;
  ws_ports: number[];
  http_ports: number[];
  ws_enabled: boolean;
  http_enabled: boolean;
}

/** 配置体检：重点查跨目录配置漂移与端口被禁用 */
export interface NapCatDiagnose {
  shells: string[];
  core_dir: string | null;
  qq_dir: string | null;
  effective_config_dir: string | null;
  config_ports: NapCatConfigEntry[];
  issues: string[];
  healthy: boolean;
}