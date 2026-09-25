# ViewMan - 本地视频管理桌面应用

## 概述

ViewMan 是一个基于 Tauri v2 的 Windows 桌面应用，用于管理本地视频文件，支持目录扫描、手动导入、搜索过滤和观看进度保存。

## 技术栈

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri v2 |
| 前端 | React 19 + TypeScript + Vite |
| 样式 | Tailwind CSS |
| 后端 | Rust (Tauri commands) |
| 数据库 | SQLite (rusqlite) |
| 视频元数据 | ffprobe 命令行调用 |
| 视频播放 | HTML5 `<video>` + file:// 协议 |

## 架构

三层架构：React UI 层 → Tauri IPC → Rust 数据层。

Rust 端负责文件扫描、元数据提取、SQLite CRUD。前端只负责展示和播放。

## 数据库模型

### videos 表
| 字段 | 类型 | 说明 |
|---|---|---|
| id | TEXT (UUID) | 主键 |
| path | TEXT | 文件绝对路径 (UNIQUE) |
| filename | TEXT | 文件名 |
| duration | REAL | 时长(秒) |
| width | INTEGER | 分辨率宽 |
| height | INTEGER | 分辨率高 |
| file_size | INTEGER | 文件大小(字节) |
| created_at | TEXT | ISO8601 入库时间 |

### watch_progress 表
| 字段 | 类型 | 说明 |
|---|---|---|
| id | TEXT (UUID) | 主键 |
| video_id | TEXT | 外键 → videos.id |
| position | REAL | 播放位置(秒) |
| updated_at | TEXT | ISO8601 更新时间 |

## 页面结构

- **Sidebar**: 扫描目录、手动导入、播放列表(预留)
- **VideoGrid**: 视频卡片网格，每张卡片含缩略图、文件名、时长、进度条
- **SearchBar**: 按文件名搜索过滤
- **PlayerView**: HTML5 视频播放器，支持进度条、播放/暂停、跳转、关闭按钮

## 交互流程

1. 启动 → 从 SQLite 加载已有视频列表
2. 扫描目录 → Rust 端递归扫描目录，提取元数据，增量入库
3. 手动导入 → 文件选择器选取视频文件，逐个提取元数据后入库
4. 点击视频 → 打开播放器，从 watch_progress 读取上次进度，seek 到该位置
5. 播放中 → 每 15 秒自动保存进度到 SQLite
6. 关闭播放器 → 立即保存一次进度
7. 视频卡片 → 根据 watch_progress 显示进度条

## 边界情况

- 文件被移动/删除后，入库时标记为不可用（暂不自动删除记录）
- 同一目录重复扫描跳过已入库文件（按 path 去重）
- 不支持在线视频流，只管理本地文件

---

## 当前实现范围（2026-09 追记）

初版 spec 只覆盖视频库的 MVP；实际落地的功能已经超出它不少，这里对齐一次。step 级细节以仓库代码为准。

| 模块 | 状态 |
|---|---|
| 视频库 | 扫描根监视（notify 3 秒防抖后自动增量扫描）、搜索/排序/高级过滤（大小/时长/分辨率/观看状态）、观看进度、缩略图、完整播放历史 |
| 图片库 | 同一套扫描/过滤/缩略图通路；19 万条量级下切目录、批删、网格分窗做过专项优化 |
| 播放 | 内置 HTML5 播放器 + PotPlayer 外挂；WebView2 缺 HEVC 解码器时按需转 H.264 缓存到 `<app_data>/transcoded/` |
| 内容检测 | 重复图（双感知哈希圈候选 + 32×32 缩略像素定组）、相似图（pHash/dHash + 互为前 2 近邻的骨架边 + 远亲）、重复视频（锚点/中段/结尾三处采样帧都要过线）、静图短视频/伪图片检测 |
| 批量操作 | 多选（Shift 连选、Del 批删）、批量移动/转换，删除走回收站并二次确认 |
| 结果落盘缓存 | `<app_data>/similar-cache.json`、`duplicate-cache.json`、`video-duplicate-cache.json` |

### 检测结果缓存的口径

三趟检测（重复图 / 相似图 / 重复视频）都是"点开一次要等几分钟"的活，所以每趟结论落一份 JSON，重启后先恢复出来看。

- 缓存里除了分组还记 `libraryCount`（存这份结果时库里的记录数）。前端只在**库又添过条目**（条数变大）时提示
  "结果是上次检测存下的…建议重跑"：少掉的条目由 `liveGroups` / `pruneGroupsToLive` 当场裁掉，显示出来的分组
  本身没错，再挂一句"过期"只是噪声。判定逻辑在 `src/detectionCache.ts`，有单测。
- 只按条数判断，"删十张又添十张"这种等量增删看不出来——要覆盖它得把整份 id 清单或库时间戳也存进缓存。
- 缓存回得比用户手点的检测晚时以手点的结果为准（`ranLive` 挡一道）；恢复出来的结果不自动勾副本、也不自动做任何删除动作。
- 按 ✕ 清结果会连缓存一起作废；删副本不动缓存（下次启动照现在的库裁一遍，删失败的副本仍看得见）。
- 相似缓存带上组内每张的 64 位指纹（面板要算"距保留张几位"），因此可能有几 MB；读写都放阻塞线程池，不占 async runtime。

### 已知取舍

- `tauri.conf.json`：`csp: null` + `assetProtocol.scope: ["**"]`——webview 可读整个磁盘。本地媒体管理器要按任意路径
  取缩略图/视频，且应用不加载远程内容，故保持放开；一旦引入远程内容需要重新收紧。
- 并行任务（图片检测、缩略图、转码）里抢锁失败按"取回毒化内容继续"处理（`lock_ignoring_poison` / `watcher::lock`），
  worker 的 join 一律 `unwrap`：`std::thread::scope` 本来就会把子线程 panic 再抛一遍，这里只保证子线程没有会 panic 的操作。
  重启流程里的 `expect`（app_data 目录、数据库初始化）保持 fail-fast，报错比带病运行好。
- 检测判定门槛（相似阈值 4–16、重复候选各差 ≤4 位、视频三处采样各 ≤6 位）都是在本机实库上量出来的拐点，
  换数据集需要重新标定。
