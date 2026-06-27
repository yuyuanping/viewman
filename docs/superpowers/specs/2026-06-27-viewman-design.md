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
