# 开发与构建

## 环境要求

- **Node.js**：建议 LTS（Vite 6 / Tailwind 4 要求 Node ≥ 18）。
- **Rust**：stable 工具链（`edition = "2021"`）。Tauri 2 首次编译较慢。
- **系统**：Windows 桌面端为主（PotPlayer 检测、注册表查找走 `winreg`）。
- **ffmpeg / ffprobe**：需在系统 `PATH` 中。应用通过命令行调用，用于元数据探测、缩略图抽帧、HEVC 转码、动图检测。可用 `check_ffprobe` 命令验证可见性。
- Windows release 版（GUI 子系统）调用 ffmpeg 时会闪黑窗，已由 `scanner::hidden_command` 的 `CREATE_NO_WINDOW` 屏蔽，无需用户处理。

## 安装

```bash
npm install
```

前端依赖在 `package.json`，Rust 依赖在 `src-tauri/Cargo.toml`（`rusqlite` 带 `bundled` 特性，无需系统 SQLite）。

## 开发模式

```bash
npm run tauri dev
```

自动起 Vite（`localhost:1420`）+ Rust 增量编译，热更新前端。`beforeDevCommand` 即 `npm run dev`。

> 开发构建不生成调试符号（`Cargo.toml` 中 `[profile.dev] debug = 0`），避免 debug 目录膨胀到数 GB。

## 构建与打包

```bash
npm run tauri build
```

先跑 `npm run build`（`tsc -b && vite build` → `dist/`），再 Rust release 编译并打安装包。产物在 `src-tauri/target/release/bundle/`，Windows 下含 `.msi` / `.exe` 安装器。

`tauri.conf.json` 关键项：`identifier = com.viewman.app`、窗口 1280×800、`bundle.targets = "all"`、图标 `icons/icon.ico`。

## 测试

```bash
# 前端纯逻辑单元测试（node --test）
npm run test

# Rust 单元测试
npm run test:cargo

# 全部
npm run test:all
```

- 前端：`src/*.test.mjs` 与 `src/hooks/*.test.mjs`，覆盖布局算法、检测缓存口径、扫描合并、范围选择、目录树、网格窗口、日志等纯逻辑。`package.json` 的 `test` script 显式列出全部测试文件。
- Rust：`#[cfg(test)]` 内联在各模块（`db.rs` 的迁移/事务/回滚、`commands/thumbnails.rs` 的孤儿恢复等）。

建议改完对应模块先跑该模块测试，再跑全量。

## 图标生成

```bash
node scripts/gen-icon.cjs
```

由 `app-icon.png` 生成各平台图标到 `src-tauri/icons/`。

## 数据目录与产物

应用运行时数据位于系统应用数据目录（Windows `%APPDATA%/com.viewman.app`）：

- `viewman.db` / `viewman.db-wal` / `viewman.db-shm` — SQLite 库及 WAL。
- `thumbnails/` — 视频/图片缩略图缓存（文件名按 id 命名）。
- `transcoded/` — HEVC → H.264 转码缓存（供 WebView2 播放）。
- `duplicate-cache.json` / `similar-cache.json` / `video-duplicate-cache.json` — 检测结果落盘。

库文件路径可在应用内通过 `db_file_path` 命令取到；需要手动备份/恢复时定位用。
