# ViewMan

本地媒体管理工具：视频库 + 图片库，专为**大规模本地媒体目录**（十几万量级文件）设计。基于 Tauri 2 + React 19 + Rust，数据全部存本地 SQLite，不上传任何文件。

## 文档

| 文档 | 内容 |
|---|---|
| [项目结构](docs/PROJECT.md) | 架构、目录树、逐文件说明、数据流 |
| [数据库设计](docs/DATABASE.md) | 表结构、FTS/索引、迁移、自检自愈 |
| [命令接口](docs/API.md) | Tauri 命令清单、事件清单 |
| [开发与构建](docs/DEVELOPMENT.md) | 环境、开发、打包、测试、数据目录 |
| [安全模型](docs/SECURITY.md) | 本地化、CSP/权限取舍、删除与进程安全 |
| [日志与排查](docs/TROUBLESHOOTING.md) | 日志方案、数据目录、常见问题 |

历史规格与实现计划见 `docs/superpowers/`。

## 功能

### 视频库
- 目录扫描 + 文件系统监视（notify）：文件变化防抖后自动增量扫描
- 缩略图墙、播放进度记忆、最近播放 / 播放历史
- **PotPlayer 联动**：自动检测注册表中的 PotPlayer，从上次进度续播
- 检测工具：
  - **假图视频**：内容其实是图片（magic bytes 检测）的视频，可转真图或移出
  - **静态视频 / 超短视频**识别与转换
  - **HEVC 编码**视频查找与 ffprobe 转码
  - **重复视频**检测（按内容聚类分组，支持人工复核确认）
- 回收站删除（trash）、批量移动

### 图片库
- 目录扫描 + 扫描范围过滤，checkpoint 断点续扫（重扫自动跳过已入库文件）
- 谷歌相册式两端对齐网格（JustifiedGrid），目录树侧栏、范围选择、右键批量操作
- **乱码 / 错标扩展名修复**：按 magic bytes 读出真实格式，修复"内容与扩展名不符"的文件
- 检测工具（结果均缓存到 SQLite，可增量续跑、可清除）：
  - **重复图片**：内容哈希分组
  - **相似图片**：感知哈希聚类，分组人工复核
  - **动图检测**：不按扩展名预筛，直接嗅探 GIF/PNG/WebP/AVIF 真实动图标志（错标 .jpg 的 GIF 不漏检）
  - **模板搜索**：以一张图为模板，找内容相近的图
- 回收站删除、批量移动、缩略图后台生成 + 孤儿缩略图清理

## 技术栈

| 层 | 技术 |
|---|---|
| 桌面框架 | Tauri 2 |
| 前端 | React 19 + TypeScript 5.7 + Vite 6 + Tailwind CSS 4 |
| 后端 | Rust 2021（tokio 异步运行时，阻塞任务走阻塞线程池） |
| 存储 | SQLite（rusqlite bundled），随应用数据目录 `viewman.db` |
| 其他 | notify（文件监视）、trash（回收站）、winreg（PotPlayer 检测） |

## 开发

```bash
# 安装依赖
npm install

# 开发模式（自动起 Vite + Rust 编译）
npm run tauri dev

# 打包
npm run tauri build
```

### 测试

```bash
# 前端单元测试（node --test）
npm run test

# Rust 单元测试
npm run test:cargo

# 全部
npm run test:all
```

### 重新生成应用图标

```bash
node scripts/gen-icon.cjs
```

## 项目结构

```
src/                    # React 前端
  components/           # 图片库 / 视频库 / 播放器 / 分组复核等 UI
  hooks/                # usePlayer、useVideos、useImages、useDuplicates 等逻辑钩子
  *.ts                  # 纯逻辑模块（布局/缓存/扫描合并等，均配 *.test.mjs）
  api.ts / types.ts     # 命令封装与类型定义
src-tauri/
  src/commands/         # Tauri command 层（scan/images/videos/thumbnails/removal/…）
  src/commands/image_detect/  # 重复 / 相似 / 动图 / 模板检测
  src/db/               # SQLite 访问层（videos/images/progress/settings）
  src/scanner.rs        # 目录扫描器 + ffprobe/ffmpeg 封装
  src/watcher.rs        # 文件变化监视（防抖增量扫描）
  src/potplayer.rs      # PotPlayer 检测与调用
docs/                   # 项目文档（结构 / 数据库 / 接口 / 开发 / 安全 / 排障）
```

## 说明

- 检测（查重、相似、动图、假图）结果均落库缓存，二次进入页面直接读取；可一键清除重跑。
- 删除走系统回收站，不直接抹盘。
- 数据库与缩略图存放在系统应用数据目录（Windows 下 `%APPDATA%/com.viewman.app`）。
