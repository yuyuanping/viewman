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

/** 下载积压口径：messages 表按 dl 状态计数，侧栏徽标取 pending+busy+dead。
 * dead 是累计死信 backlog（复检跑满前不删），面板展示的是 dead_today */
export interface BotDlStats {
  pending: number;
  busy: number;
  dead: number;
  /** 今天才转死的条数，按自然日归零 */
  dead_today: number;
}

/** 下载列表一行：当日有下载动作的消息（含转发归档行与其转发明细行） */
export interface BotDlItem {
  id: number;
  kind: string;
  /** 文件原始名，没有则是文本前 80 字预览 */
  label: string;
  size: number | null;
  /** 媒体 1=重复跳过（复用现有文件）；转发行=内部跳过的重复媒体数 */
  dedup: number;
  err: string | null;
  dl: string;
  /** 转发明细行指向所属转发行；顶层消息为空（面板据此把它缩进到转发下面） */
  parent: number | null;
  /** 明细行的内层发送者昵称，转发里不同人发的图靠它区分 */
  sender: string | null;
  /** 本次尝试已落盘字节：仅 dl='busy' 行有意义；转发父行=块内聚合值；老库为 null */
  dl_bytes: number | null;
  /** Content-Length；来源未给（或转发聚合行）为 null → 只显示已下载量不显百分比 */
  dl_total: number | null;
}

/** 下载汇总：媒体文件（image/video/file，dl='ok'）的累计与今日完成量 */
export interface BotDlSummary {
  /** 累计已下载文件数 */
  ok: number;
  ok_bytes: number;
  /** 今日（本地自然日）完成数，每日清零 */
  today: number;
  today_bytes: number;
  /** 今日重复跳过数，每日清零 */
  dedup_today: number;
}

/** 轻量快照（只读 bot.db，不探测进程/端口），下载活跃期高频轮询用 */
export interface BotDlSnapshot {
  dl: BotDlStats;
  summary: BotDlSummary;
  today_items: BotDlItem[];
}

/** 一次状态快照：进程 + NapCat 端口 + bot.db 只读积压计数与当日下载列表，字段名与 Rust 端一致（snake_case） */
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
  summary: BotDlSummary;
  /** 当日全部下载/归档消息，按 id 倒序（按状态筛选在面板里做，不必回后端） */
  today_items: BotDlItem[];
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