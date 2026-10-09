# 日志与问题排查

## 日志方案

### 前端（`src/logger.ts`）

分级日志 `logger.debug/info/warn/error(tag, ...args)`：

- dev（`import.meta.env.DEV`）：`debug/info` 也输出到浏览器控制台（Tauri dev 窗口的 DevTools）。
- prod：只输出 `warn/error`，避免刷屏。
- 按标签过滤，如 `[thumbnail]`、`[scan]`、`[potplayer]`；替代散落的 `console.warn` / 空 `catch`。

### 后端（Rust）

- 启动失败与运行时错误用 `eprintln!`（stderr）；Tauri 会把 stderr 带到运行终端。
- 正常运维信息（如孤儿封面清理数）用 `println!`。
- dev 模式在 `npm run tauri dev` 的终端可见；release 模式为 GUI 子系统，无控制台输出，问题需按下方"已知场景"逐条核对。

## 应用数据目录

Windows：`%APPDATA%/com.viewman.app`

- `viewman.db`（+ `-wal` / `-shm`）：主库。备份 = 复制这三个文件 + 各 `*-cache.json`。
- `thumbnails/`：缩略图缓存，可整目录删除后由 `resume_thumbnails` / 生成任务重建。
- `transcoded/`：HEVC 转码缓存，可删除（下次播放重新转）。
- `*-cache.json`：查重/相似/重复视频缓存。清结果走应用的「清结果」按钮（连带缓存），不要手动删文件与按钮行为不一致。

## 常见问题

### 扫描/缩略图全失败，或视频无时长、图片无尺寸

- 确认 ffmpeg / ffprobe 在系统 `PATH` 中；应用内检查 `check_ffprobe` 是否返回 true。
- 网络盘/坏文件会触发 `output_timeout`（探测 30s 超时杀进程），表现为个别文件被跳过而非整批失败。

### HEVC 视频黑屏或无法播放

- WebView2 缺 HEVC 解码器，`convert_hevc_videos` 会把指定视频转 H.264 缓存到 `transcoded/` 后播放，转换进度见 `hevc-progress`。
- 转码目录空间不足时优先释放 `transcoded/` 与 `thumbnails/`。

### 删除提示"被占用"（locked）

- 批量删除时文件被 ffmpeg（正生成封面/检测/转码）、杀软或资源管理器预览窗短暂占用。
- 后端已做 6 次退避重试（约 3s），仍失败才报 locked；先关闭占用程序再重试，或等封面任务跑完。

### 搜索结果与目录树不一致 / 搜索过慢

- FTS 索引可能损坏（报 `malformed`/`corrupt` 时）：维护菜单跑 `rebuild_fts_indexes` 无损重建全量索引；仍失败再用 `check_db_integrity` 定性主表。

### 数据库损坏警告

- `check_db_integrity` 返回非 `"ok"`：先停止应用，备份 `viewman.db*`，再考虑从备份恢复。
- 删除类错误带自愈：疑似损坏会先重建 FTS 重试一次，多数场景无需手工介入。

### PotPlayer 联动失败

- `check_potplayer` false：未装 PotPlayer，或安装在非默认路径且注册表 `HKEY_CURRENT_USER|LOCAL_MACHINE\SOFTWARE\DAUM\PotPlayer\InstallPath` 不可读。内置播放器不受影响。

### 打开应用后检测结果"过期"

- 库条数比缓存记录时大 → 界面上提示建议重跑，属预期行为（`detectionCache` 口径）。

### 修改配置后无效果

- `tauri.conf.json` / `capabilities` 改动需重启 `npm run tauri dev` 或重新 build。

## 维护清单

- 定期跑 `npm run test:all`（前端单测 + Rust 单测）。
- 库文件与 JSON 缓存建议随系统备份留存。
- 重建图标：`node scripts/gen-icon.cjs`。
