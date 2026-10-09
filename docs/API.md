# Tauri 命令接口

本应用所有前后端通信都走 Tauri IPC。前端统一经 `src/api.ts` 的类型化封装调用（不散落裸 `invoke`），后端命令在 `src-tauri/src/lib.rs` 的 `invoke_handler!` 注册，实现在 `src-tauri/src/commands/*`。

约定：命令返回 `Result<T, String>`，失败时 `String` 为中文错误信息；长任务会通过 Tauri 事件（`app.emit`）推送进度，前端用 `useTauriEvent` 订阅。

## 视频库

| 命令 | 说明 | 返回 / 事件 |
|---|---|---|
| `scan_directory(dir)` | 扫描一个视频目录，增量入库 | `ScanOutcome<Video>` + `scan-progress` |
| `get_video_view(dir, search, limit, offset)` | 目录前缀 + 文件名搜索（后端做）+ 分页 | `{items, total, total_size}` |
| `get_videos_with_progress()` | 视频 + 播放进度 | `VideoWithProgress[]` |
| `get_videos()` | 全量视频（旧接口，大库改用 `get_video_view`） | `Video[]` |
| `get_recently_played()` / `get_play_history()` | 最近播放 / 完整播放历史 | `RecentlyPlayed[]` |
| `check_video_file(video_id)` | 可读性判定 | `VideoFileStatus` |
| `get_playable_path(video_id)` | 可播放路径（内置播放器用） | `string` |
| `save_progress(video_id, position)` | 播放进度保存 | `void` |
| `generate_thumbnails(ids)` | 生成视频封面 | `number` + `thumbnail-progress` |
| `resume_thumbnails({ kind: "video" })` | 启动续跑孤儿封面 | `boolean` |

### 视频检测 / 转换

| 命令 | 说明 | 返回 / 事件 |
|---|---|---|
| `convert_fake_images(ids)` | 假图视频（内容实为图片）转真图 | `ConversionResult` |
| `find_static_videos()` | 静态视频（静图/超短）扫描 | `string[]` |
| `convert_short_videos(ids)` | 超短视频转换 | `ConversionResult` |
| `find_hevc_videos()` | HEVC 编码视频查找 | `string[]` + `hevc-detect-progress` |
| `convert_hevc_videos(ids)` | HEVC → H.264 转码 | `ConversionResult` + `hevc-progress` |
| `find_duplicate_videos()` | 重复视频检测（三处采样帧） | `string[][]` + `video-duplicate-progress` |
| `get_video_duplicate_cache()` | 上次重复视频检测的落盘结果 | `VideoDuplicateCache \| null` |
| `capture_frame()` | 抽帧 | `string` |

## 图片库

| 命令 | 说明 | 返回 / 事件 |
|---|---|---|
| `get_images()` | 全量图片 | `Image[]` |
| `get_image_view(dir, search)` | 目录前缀 + 文件名子串过滤（后端做），全部行 | `{items, total, total_size}` |
| `get_image_stats()` | 统计（总数/缺封面/每目录计数） | `ImageStatsPayload` |
| `get_images_by_ids(ids)` | 按 id 取 | `Image[]` |
| `scan_image_directory(dir)` | 扫描图片目录（checkpoint 断点续扫） | `ScanOutcome<Image>` + `image-scan-progress` |
| `fix_mismatched_extensions()` | 乱码/错标扩展名修复（magic bytes） | `string[]` |
| `delete_images(ids)` | 批量删除（回收站） | `DeletionReport` |
| `move_image(image_id, target_dir)` | 移动单张 | `string`（新路径） |
| `generate_image_thumbnails(ids)` | 生成图片封面 | `number` + `image-thumbnail-progress` |
| `resume_thumbnails({ kind: "image" })` | 启动续跑孤儿封面 | `boolean` |
| `get_missing_image_thumbnail_ids()` | 缺封面的 id | `string[]` |

### 图片检测

| 命令 | 说明 | 返回 / 事件 |
|---|---|---|
| `find_duplicate_images(pixel_verify)` | 重复图（双哈希 + 32×32 像素定组） | `DuplicateReport` + `duplicate-progress` |
| `find_images_like_template(image_id)` | 模板搜索（以图搜图） | `TemplateMatchResult` + `template-progress` |
| `find_animated_images()` | 动图检测（嗅探真实动图标志） | `string[]` |
| `get_duplicate_cache()` | 上次重复/相似检测落盘结果 | `DuplicateCache \| null` |
| `clear_detection_cache(which)` | 清检测缓存（duplicate/videoDuplicate） | `void` |

## 播放器（PotPlayer 联动）

| 命令 | 说明 | 返回 / 事件 |
|---|---|---|
| `check_potplayer()` / `check_ffprobe()` | 检测软件是否可用 | `boolean` |
| `launch_potplayer(path, seek)` | 外挂播放，从 `seek` 秒续播 | `void` |
| `potplayer_status(path)` | 轮询播放状态 | `PotPlayerStatus` |

`potplayer_status` 通过监听 PotPlayer 的进度（每次轮询同步更新），还会 `emit("player-progress")`。

## 设置与维护

| 命令 | 说明 | 返回 / 事件 |
|---|---|---|
| `load_scan_roots(kind)` / `save_scan_roots(kind, roots)` | 扫描根目录读写（kind: video/image） | `string[]` |
| `load_move_targets(kind)` / `save_move_targets(kind, targets)` | 批量移动目标目录记忆 | `string[]` |
| `remove_media_directory(kind, dir)` | 从库移除目录（保留磁盘文件） | `void` |
| `check_db_integrity()` | 数据库自检，返回 `"ok"` 或损坏描述 | `string` |
| `rebuild_fts_indexes()` | 无损重建 FTS 索引 | `string` |
| `db_file_path()` | 库文件路径（手动恢复用） | `string` |
| `cancel_task(task)` | 取消长任务（scan-video/scan-image/hevc/thumb-*） | `void` |

## 事件清单

后端 `app.emit`，载荷形状见 `src/api.ts` 各 payload 接口。

| 事件 | 载荷 | 触发时机 |
|---|---|---|
| `scan-progress` | `ScanProgressPayload` | 视频扫描逐批进度 / done 摘要 |
| `image-scan-progress` | `ScanProgressPayload` | 图片扫描逐批进度 / done 摘要 |
| `rescan-progress` | `ScanProgressPayload` | watcher 自动增量扫描 |
| `videos-changed` / `images-changed` | `ScanOutcome<T>` | watcher 扫描后增量广播，前端就地合并 |
| `thumbnail-progress` / `image-thumbnail-progress` | `ThumbnailProgressPayload` | 封面生成进度 |
| `hevc-progress` | `HevcProgressPayload` | HEVC 转码进度 |
| `hevc-detect-progress` | `HevcDetectProgressPayload` | HEVC 查找进度 |
| `duplicate-progress` / `video-duplicate-progress` | `DuplicateProgressPayload` | 查重（sigs / grouping 两阶段） |
| `template-progress` | `TemplateProgressPayload` | 模板搜索补指纹进度 |
| `player-progress` | — | PotPlayer 播放进度轮询 |
