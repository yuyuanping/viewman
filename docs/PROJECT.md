# ViewMan 项目结构

本地媒体管理桌面应用：视频库 + 图片库，专为十几万量级本地媒体目录设计。
基于 Tauri 2 + React 19 + Rust，数据全部存本地 SQLite，不上传任何文件。

## 技术栈

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri 2（`tauri.conf.json`、`capabilities/`） |
| 前端 | React 19 + TypeScript 5.7 + Vite 6 + Tailwind CSS 4 |
| 后端 | Rust 2021（tokio 异步运行时，阻塞任务走阻塞线程池） |
| 存储 | SQLite（rusqlite bundled），随应用数据目录 `viewman.db` |
| 媒体探测 | ffprobe / ffmpeg 命令行调用（超时 + 无窗口） |
| 其他 | notify（文件监视）、trash（回收站）、winreg（PotPlayer 检测） |

## 架构

三层架构，前后端经 Tauri IPC 通信：

```
React UI 层                 Tauri IPC                   Rust 数据层 + 后台任务
─────────────              ─────────                  ─────────────────────
components/  ──invoke──▶  generate_handler!  ──▶  commands/（命令层）
hooks/       ◀──event──   app.emit(...)      ◀──   scanner / watcher
*.ts 纯逻辑                                     db/（SQLite）
```

- **React UI 层**：组件（`components/`）只负责展示与交互，逻辑放 `hooks/`，纯算法放 `src/*.ts`。
- **Tauri IPC**：后端通过 `lib.rs` 的 `invoke_handler!` 注册约 50 个命令；长任务用 `app.emit` 推进度事件，前端用 `useTauriEvent` 订阅。
- **Rust 数据层**：`commands/` 是 Tauri command 处理层；`db/` 是 SQLite 访问层；`scanner`/`watcher`/`potplayer` 是独立子系统。阻塞任务（扫描、检测、转码、缩略图）在 `spawn_blocking` 线程跑，不占 async runtime。

## 目录结构

```
viewman/
├── index.html                  Vite 入口
├── package.json                前端依赖 + scripts（dev/build/test/tauri）
├── vite.config.ts              Vite 配置（端口 1420、React + Tailwind 插件）
├── tsconfig*.json              TS 配置（app / node 分区）
├── app-icon.png                应用图标源图
├── scripts/
│   └── gen-icon.cjs            由源图生成各平台图标
├── src/                        React 前端
│   ├── main.tsx                入口：挂载 <App/>
│   ├── App.tsx                 主布局（侧栏 + 内容区 + 播放器遮罩）
│   ├── api.ts                  全部 Tauri 命令的类型化封装（组件/hook 唯一调用出口）
│   ├── types.ts                前端类型定义（与 Rust models 对齐）
│   ├── *.ts                    纯逻辑模块（均有同名 .test.mjs）
│   ├── components/             UI 组件
│   ├── hooks/                  逻辑钩子（按职责拆分）
│   └── styles/index.css        Tailwind 入口 + 自定义样式
├── src-tauri/                  Rust 后端
│   ├── Cargo.toml              Rust 依赖（dev 不生成调试符号）
│   ├── tauri.conf.json         Tauri 应用配置（窗口、bundle、安全）
│   ├── capabilities/default.json   权限声明
│   ├── icons/                 各平台图标
│   └── src/
│       ├── main.rs             入口（release 走 windows_subsystem）
│       ├── lib.rs              插件注册 + setup + 命令清单 + watcher 启动
│       ├── models.rs           数据结构（Video/Image/ScanOutcome…）
│       ├── scanner.rs         目录扫描 + ffprobe/ffmpeg 封装
│       ├── watcher.rs         文件监视（防抖增量扫描）
│       ├── potplayer.rs       PotPlayer 检测与调用
│       ├── db.rs              数据库初始化 + 迁移 + 自检/自愈
│       ├── db/                SQLite 访问层（videos/images/progress/settings）
│       └── commands/          Tauri command 层（按模块拆分）
└── docs/                       项目文档
    └── superpowers/           设计规格与实现计划（历史档案）
```

## 前端文件说明

### 入口与编排

- `src/main.tsx`：ReactDOM 挂载 `<App/>`。
- `src/App.tsx`：顶层布局，切换视频库 / 图片库视图，承载播放器遮罩与全局 Toast。
- `src/api.ts`：集中封装所有 `invoke` 调用并标注类型与用途注释；组件与 hook 一律经此调用，不散落裸 `invoke`。
- `src/types.ts`：与 Rust `models.rs` 对齐的接口与事件 payload 类型。

### 纯逻辑模块（`src/*.ts`，均配 `*.test.mjs`）

| 文件 | 职责 |
|---|---|
| `justifiedLayout.ts` | 谷歌相册式两端对齐网格布局算法 |
| `detectionCache.ts` | 检测结果缓存口径（libraryCount 比对、过期裁剪） |
| `scanRoots.ts` | 扫描根目录管理 |
| `scanMerge.ts` | 增量扫描结果就地合并 |
| `libraryFilter.ts` | 库过滤逻辑 |
| `dirTree.ts` | 目录树构建 |
| `rangeSelect.ts` | Shift 连选逻辑 |
| `similarGroups.ts` | 相似分组处理 |
| `useGridWindow.ts` | 大网格分窗虚拟化 |
| `logger.ts` | 分级日志（见 TROUBLESHOOTING） |

### 组件（`src/components/`）

视频侧：`VideoGrid`、`VideoCard`、`PlayerView`、`PlaylistPanel`、`PlayHistoryPanel`、`RecentlyPlayedList`、`SearchBar`、`Sidebar`。
图片侧：`ImageLibrary`、`ImageGrid`、`JustifiedGrid`、`ImageCard`、`ImageToolbar`、`LibraryToolbar`、`ImageViewer`、`DirTree`、`MoveTargetsDialog`。
检测复核：`GroupReviewPanel`、`TemplateMatchPanel`。
通用：`ContextMenu`、`Toast`。

### Hooks（`src/hooks/`，按职责拆分）

库与视图：`useVideos`、`useVideoView`、`useVideoLibrary`、`useImages`、`useScanProgress`、`useGridWindow`、`useRangeSelect`、`useBatchSelection`、`useMoveTargets`、`useDeleteShortcut`、`useFullscreen`。
播放：`usePlayer`、`usePlayerPlayback`、`usePlayerShortcuts`、`useVideoRotation`、`usePotPlayer`、`potPlayerSession`。
检测：`useDuplicates`、`useDuplicateGroups`、`useGroupReviewCore`、`useTemplateSearch`、`useFileCheck`、`useHevcConversion`。
缩略图：`useThumbnailGeneration`。
基础设施：`useTauriEvent`（事件订阅 + 卸载竞态处理）、`useToasts`。

## 后端文件说明

### 顶层模块（`src-tauri/src/`）

- `main.rs`：二进制入口，调用 `viewman_lib::run()`；release 关闭控制台窗口。
- `lib.rs`：注册插件（opener/dialog）、`setup` 初始化数据库与 `AppState`、启动 watcher 与孤儿缩略图清理、`invoke_handler!` 注册全部命令。
- `models.rs`：序列化数据结构（`Video`/`Image`/`ScanOutcome`/`VideoProgress`/`RecentlyPlayed`/`VideoFileStatus`/`ConversionResult`/`DeletionReport`）。
- `scanner.rs`：目录遍历、ffprobe 元数据提取、ffmpeg 抽帧/缩放；`hidden_command` 保证无窗口、`output_timeout` 防卡死。
- `watcher.rs`：notify 递归监视扫描根，3 秒防抖后串行增量扫描，失败退避；根表动态增删 watch。
- `potplayer.rs`：按固定路径 + 注册表查找 PotPlayer，`/seek=` 续播，运行状态轮询。

### 数据库层（`src-tauri/src/db.rs` 与 `db/`）

- `db.rs`：`init_db`（WAL/外键/busy_timeout）、`create_tables`、`ensure_columns`（迁移）、`ensure_fts`（FTS 回填）、`integrity_check`、`rebuild_fts`、`like_escape`。
- `db/videos.rs`、`db/images.rs`、`db/progress.rs`、`db/settings.rs`：分表 CRUD。

### 命令层（`src-tauri/src/commands.rs` 与 `commands/`）

- `commands.rs`：模块汇总、`AppState`、回收站删除辅助（`undeleted_targets`）、库损坏自愈删除（`delete_records_with_heal`）、`is_corruption_error`。
- `commands/scan.rs`：`scan_directory` / `scan_image_directory`，断点续扫 + 增量广播。
- `commands/videos.rs`、`commands/images.rs`：各自库的查询 / 视图 / 删除 / 移动 / 转换 / 检测入口。
- `commands/image_detect/`：重复图（`duplicate.rs`）、模板匹配（`template.rs`）、动图（`image_anim.rs`）、公共（`detect_util.rs`、`mod.rs`）。
- `commands/video_detect.rs`：重复视频（三处采样帧）。
- `commands/thumbnails.rs`：封面生成 / 续跑 / 孤儿清理 / 抽帧。
- `commands/players.rs`、`commands/progress.rs`、`commands/settings.rs`、`commands/removal.rs`、`commands/maintenance.rs`：各专项命令。

## 数据流示例

### 扫描

1. 用户加扫描根 → `save_scan_roots` 写 settings 表 → watcher 给新根挂 watch。
2. 点扫描 → `scan_directory` 在阻塞线程遍历目录、ffprobe 探测、增量入库；逐批 `emit("scan-progress")`。
3. 完成 `emit("scan-progress", done=true, summary)`，前端合并增量、更新视图。
4. 文件变化时 watcher 防抖后自动跑同一命令，`emit("videos-changed"/"images-changed")`。

### 播放

1. 点视频 → `check_video_file` 判定可读性 → `get_playable_path` 取播放路径。
2. 内置播放器从 `watch_progress.position` 续播；播放中每 15 秒 `save_progress`，关闭时再存一次。
3. PotPlayer 外挂：`check_potplayer` / `launch_potplayer(path, seek)` / `potplayer_status` 轮询实时进度。

### 检测

1. 触发检测 → 阻塞线程算指纹/聚类，逐批 `emit("duplicate-progress" 等)` 推进度与分组。
2. 结果落库（指纹）+ 落 JSON 缓存（`<app_data>/*-cache.json`）；下次启动先恢复缓存，按 `libraryCount` 判断是否过期。
3. 复核面板人工确认后批量删除/移动，删除走回收站。

## 历史档案

`docs/superpowers/` 下为初版设计规格与实现计划，记录了 MVP 范围与逐 Task 步骤；
当前实现已超出该计划，以本目录文档与代码为准。
