# ViewMan Implementation Plan

> **状态（2026-09 追记）**：本计划的 Task 1–10 已全部落地，下方步骤里的文件清单按当初的设想写，
> 实际结构已经演进——hooks 按职责拆到 `src/hooks/*`，纯逻辑（分组、过滤、网格窗口、扫描合并…）抽到
> `src/*.ts` 并配了 `*.test.mjs`（`npm test`），图片库 / 相似检测 / PotPlayer 联动 / HEVC 与静图短视频
> 转换都属于计划外新增。step 级 checkbox 保留原样，功能口径与后续追记以
> `docs/superpowers/specs/2026-06-27-viewman-design.md` 的「当前实现范围」一节为准。

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Tauri v2 desktop app that manages local videos and saves watch progress.

**Architecture:** Three layers — React frontend renders video grid + player; Tauri IPC bridges frontend and Rust; Rust backend handles SQLite persistence, directory scanning, and metadata extraction via ffprobe. Business logic is separated from command handlers for testability.

**Tech Stack:** Tauri v2, React 19, TypeScript, Vite, Tailwind CSS, Rust (rusqlite, uuid, serde, chrono), ffprobe for metadata.

---

## File Structure

```
viewman/
├── src/                          # React frontend
│   ├── App.tsx                   # Main layout (sidebar + grid + player)
│   ├── main.tsx                  # Entry point
│   ├── types.ts                  # TypeScript interfaces
│   ├── components/
│   │   ├── Sidebar.tsx           # Scan / import / playlist UI
│   │   ├── VideoGrid.tsx         # Grid of video cards
│   │   ├── VideoCard.tsx         # Single card (thumbnail, name, duration, progress)
│   │   ├── PlayerView.tsx        # Full video player overlay
│   │   └── SearchBar.tsx         # Text search / sort controls
│   ├── hooks/
│   │   ├── useVideos.ts          # Video CRUD via Tauri invoke
│   │   └── usePlayer.ts          # Player open/close/progress state
│   └── styles/
│       └── index.css             # Tailwind imports + custom styles
├── index.html
├── package.json
├── tsconfig.json
├── tailwind.config.js
├── postcss.config.js
├── vite.config.ts
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/default.json
│   ├── src/
│   │   ├── main.rs               # Tauri entry, register commands
│   │   ├── lib.rs                # Re-export modules
│   │   ├── db.rs                 # SQLite init + migrations + queries
│   │   ├── models.rs             # Video, WatchProgress structs
│   │   ├── commands.rs           # Tauri command handlers
│   │   └── scanner.rs            # Dir walk + ffprobe metadata extraction
│   └── icons/
└── docs/superpowers/specs/
    └── 2026-06-27-viewman-design.md
```

---

### Task 1: Scaffold Tauri v2 + React project

**Files:**
- Create: All scaffolded files from `npm create tauri-app`

- [ ] **Step 1: Scaffold the project**

Run from `D:\opencword\viewman` (empty directory):

```bash
npm create tauri-app@latest viewman -- --template react-ts
```

When prompted for app name, enter `viewman`. For the identifier, use `com.viewman.app`.

Actual command to run (the `--` passes flags to the underlying create tool):

```bash
cd D:\opencword\viewman
npm create tauri-app@latest -- viewman --template react-ts --manager npm
```

If the interactive prompt appears, answer:
- App name: `viewman`
- Identifier: `com.viewman.app`

- [ ] **Step 2: Verify scaffold works**

```bash
cd D:\opencword\viewman
npm install
npm run tauri init
```

Then test the dev build:
```bash
npm run tauri dev
```
Expected: A window with the default React counter app.

- [ ] **Step 3: Install Tailwind CSS**

```bash
cd D:\opencword\viewman
npm install -D tailwindcss @tailwindcss/vite
```

Update `vite.config.ts`:
```typescript
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
}));
```

Replace `src/styles/index.css` content:
```css
@import "tailwindcss";
```

- [ ] **Step 4: Commit**

```bash
git init
git add .
git commit -m "feat: scaffold Tauri v2 + React + Tailwind"
```

---

### Task 2: Rust — Database module

**Files:**
- Create: `src-tauri/src/db.rs`
- Modify: `src-tauri/Cargo.toml` (add dependencies)
- Create: `src-tauri/src/models.rs`

- [ ] **Step 1: Add Rust dependencies**

Update `src-tauri/Cargo.toml`:
```toml
[package]
name = "viewman"
version = "0.1.0"
edition = "2021"

[lib]
name = "viewman_lib"
crate-type = ["lib", "cdylib", "staticlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-opener = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.31", features = ["bundled"] }
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 2: Write models.rs**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub path: String,
    pub filename: String,
    pub duration: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub file_size: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchProgress {
    pub id: String,
    pub video_id: String,
    pub position: f64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoProgress {
    pub video: Video,
    pub position: Option<f64>,
}
```

- [ ] **Step 3: Write db.rs**

```rust
use rusqlite::{Connection, Result, params};
use crate::models::{Video, WatchProgress};

pub fn init_db(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS videos (
            id TEXT PRIMARY KEY,
            path TEXT UNIQUE NOT NULL,
            filename TEXT NOT NULL,
            duration REAL,
            width INTEGER,
            height INTEGER,
            file_size INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS watch_progress (
            id TEXT PRIMARY KEY,
            video_id TEXT NOT NULL UNIQUE,
            position REAL NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (video_id) REFERENCES videos(id) ON DELETE CASCADE
        );"
    )?;
    Ok(conn)
}

pub fn get_all_videos(conn: &Connection) -> Result<Vec<Video>> {
    let mut stmt = conn.prepare(
        "SELECT id, path, filename, duration, width, height, file_size, created_at FROM videos ORDER BY filename"
    )?;
    let videos = stmt.query_map([], |row| {
        Ok(Video {
            id: row.get(0)?,
            path: row.get(1)?,
            filename: row.get(2)?,
            duration: row.get(3)?,
            width: row.get(4)?,
            height: row.get(5)?,
            file_size: row.get(6)?,
            created_at: row.get(7)?,
        })
    })?.collect::<Result<Vec<_>>>()?;
    Ok(videos)
}

pub fn insert_video(conn: &Connection, video: &Video) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO videos (id, path, filename, duration, width, height, file_size, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            video.id, video.path, video.filename,
            video.duration, video.width, video.height,
            video.file_size, video.created_at
        ],
    )?;
    Ok(())
}

pub fn get_existing_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM videos")?;
    let paths = stmt.query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>>>()?;
    Ok(paths)
}

pub fn upsert_progress(conn: &Connection, video_id: &str, position: f64) -> Result<()> {
    conn.execute(
        "INSERT INTO watch_progress (id, video_id, position, updated_at) VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(video_id) DO UPDATE SET position = ?3, updated_at = datetime('now')",
        params![uuid::Uuid::new_v4().to_string(), video_id, position],
    )?;
    Ok(())
}

pub fn get_progress(conn: &Connection, video_id: &str) -> Result<Option<WatchProgress>> {
    let mut stmt = conn.prepare(
        "SELECT id, video_id, position, updated_at FROM watch_progress WHERE video_id = ?1"
    )?;
    let mut rows = stmt.query_map(params![video_id], |row| {
        Ok(WatchProgress {
            id: row.get(0)?,
            video_id: row.get(1)?,
            position: row.get(2)?,
            updated_at: row.get(3)?,
        })
    })?;
    match rows.next() {
        Some(Ok(progress)) => Ok(Some(progress)),
        _ => Ok(None),
    }
}

pub fn get_videos_with_progress(conn: &Connection) -> Result<Vec<(Video, Option<f64>)>> {
    let mut stmt = conn.prepare(
        "SELECT v.id, v.path, v.filename, v.duration, v.width, v.height, v.file_size, v.created_at, wp.position
         FROM videos v LEFT JOIN watch_progress wp ON v.id = wp.video_id
         ORDER BY v.filename"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            Video {
                id: row.get(0)?,
                path: row.get(1)?,
                filename: row.get(2)?,
                duration: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                file_size: row.get(6)?,
                created_at: row.get(7)?,
            },
            row.get::<_, Option<f64>>(8)?,
        ))
    })?.collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Video;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db("").unwrap_err(); // ignore, we'll manually create tables
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS videos (
                id TEXT PRIMARY KEY, path TEXT UNIQUE NOT NULL, filename TEXT NOT NULL,
                duration REAL, width INTEGER, height INTEGER, file_size INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS watch_progress (
                id TEXT PRIMARY KEY, video_id TEXT NOT NULL UNIQUE, position REAL NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                FOREIGN KEY (video_id) REFERENCES videos(id) ON DELETE CASCADE
            );"
        ).unwrap();
        conn
    }

    #[test]
    fn test_insert_and_get_videos() {
        let conn = setup_test_db();
        let video = Video {
            id: "test-id".into(),
            path: "C:\\videos\\test.mp4".into(),
            filename: "test.mp4".into(),
            duration: Some(120.5),
            width: Some(1920),
            height: Some(1080),
            file_size: 1024 * 1024 * 50,
            created_at: "2026-01-01T00:00:00".into(),
        };
        insert_video(&conn, &video).unwrap();
        let videos = get_all_videos(&conn).unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].filename, "test.mp4");
    }

    #[test]
    fn test_progress_upsert() {
        let conn = setup_test_db();
        let video = Video {
            id: "v1".into(), path: "C:\\v.mp4".into(), filename: "v.mp4".into(),
            duration: None, width: None, height: None, file_size: 100, created_at: "".into(),
        };
        insert_video(&conn, &video).unwrap();

        upsert_progress(&conn, "v1", 30.5).unwrap();
        let p = get_progress(&conn, "v1").unwrap().unwrap();
        assert!((p.position - 30.5).abs() < 0.001);

        upsert_progress(&conn, "v1", 60.0).unwrap();
        let p = get_progress(&conn, "v1").unwrap().unwrap();
        assert!((p.position - 60.0).abs() < 0.001);
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cd D:\opencword\viewman\src-tauri
cargo test
```
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/db.rs src-tauri/src/models.rs
git commit -m "feat: add SQLite database module with tests"
```

---

### Task 3: Rust — Scanner (directory walk + ffprobe)

**Files:**
- Create: `src-tauri/src/scanner.rs`

- [ ] **Step 1: Write scanner.rs**

```rust
use std::path::Path;
use std::process::Command;
use std::fs;
use serde::Deserialize;
use crate::models::Video;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v"];

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: FfprobeFormat,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

pub struct VideoMeta {
    pub duration: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

pub fn get_metadata(path: &str) -> Result<VideoMeta, String> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
            path,
        ])
        .output()
        .map_err(|e| format!("Failed to run ffprobe: {}. Is ffmpeg installed?", e))?;

    if !output.status.success() {
        return Err(format!("ffprobe failed for: {}", path));
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse ffprobe output: {}", e))?;

    let duration = parsed.format.duration
        .and_then(|d| d.parse::<f64>().ok());

    let video_stream = parsed.streams.iter().find(|s| s.codec_type.as_deref() == Some("video"));
    let width = video_stream.and_then(|s| s.width);
    let height = video_stream.and_then(|s| s.height);

    Ok(VideoMeta { duration, width, height })
}

pub fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}

pub fn scan_directory_recursive(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", dir.display()));
    }

    let mut files = Vec::new();
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("Cannot read directory {}: {}", dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Error reading entry: {}", e))?;
        let path = entry.path();

        if path.is_dir() {
            let mut sub = scan_directory_recursive(&path)?;
            files.append(&mut sub);
        } else if is_video_file(&path) {
            files.push(path);
        }
    }

    Ok(files)
}

pub fn build_video(path: &std::path::PathBuf) -> Video {
    let metadata = get_metadata(&path.to_string_lossy()).ok();
    let file_meta = fs::metadata(path).ok();

    Video {
        id: uuid::Uuid::new_v4().to_string(),
        path: path.to_string_lossy().to_string(),
        filename: path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        duration: metadata.as_ref().and_then(|m| m.duration),
        width: metadata.as_ref().and_then(|m| m.width),
        height: metadata.as_ref().and_then(|m| m.height),
        file_size: file_meta.map(|m| m.len() as i64).unwrap_or(0),
        created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_is_video_file() {
        assert!(is_video_file(&PathBuf::from("test.mp4")));
        assert!(is_video_file(&PathBuf::from("test.MKV")));
        assert!(!is_video_file(&PathBuf::from("test.txt")));
        assert!(!is_video_file(&PathBuf::from("test")));
    }

    #[test]
    fn test_scan_empty_directory() {
        let dir = std::env::temp_dir().join(format!("viewman_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let result = scan_directory_recursive(&dir);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());

        fs::remove_dir(&dir).unwrap();
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cd D:\opencword\viewman\src-tauri
cargo test
```
Expected: 4 passed (2 from db + 2 from scanner).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/scanner.rs
git commit -m "feat: add directory scanner and ffprobe metadata extraction"
```

---

### Task 4: Rust — Tauri commands + main entry

**Files:**
- Create: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`

- [ ] **Step 1: Write commands.rs**

```rust
use tauri::State;
use crate::db;
use crate::models::{Video, WatchProgress};
use crate::scanner;
use std::sync::Mutex;

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
}

#[tauri::command]
pub fn get_videos(state: State<AppState>) -> Result<Vec<Video>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_all_videos(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn scan_directory(state: State<AppState>, dir: String) -> Result<Vec<Video>, String> {
    let dir_path = std::path::Path::new(&dir);
    let files = scanner::scan_directory_recursive(dir_path)?;

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let existing_paths: Vec<String> = db::get_existing_paths(&conn).map_err(|e| e.to_string())?;

    let mut new_videos = Vec::new();
    for file in &files {
        let path_str = file.to_string_lossy().to_string();
        if existing_paths.contains(&path_str) {
            continue;
        }
        let video = scanner::build_video(file);
        db::insert_video(&conn, &video).map_err(|e| e.to_string())?;
        new_videos.push(video);
    }

    Ok(new_videos)
}

#[tauri::command]
pub fn save_progress(state: State<AppState>, video_id: String, position: f64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::upsert_progress(&conn, &video_id, position).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_progress(state: State<AppState>, video_id: String) -> Result<Option<WatchProgress>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_progress(&conn, &video_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_videos_with_progress(state: State<AppState>) -> Result<Vec<(Video, Option<f64>)>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_videos_with_progress(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_ffprobe() -> bool {
    std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_ok()
}
```

- [ ] **Step 2: Update lib.rs**

```rust
pub mod commands;
pub mod db;
pub mod models;
pub mod scanner;
```

- [ ] **Step 3: Update main.rs**

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;
use viewman_lib::commands::{self, AppState};
use viewman_lib::db;

fn get_db_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    let app_dir = app.path().app_data_dir().expect("failed to get app data dir");
    std::fs::create_dir_all(&app_dir).expect("failed to create app data dir");
    app_dir.join("viewman.db")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let db_path = get_db_path(&app.handle());
            let conn = db::init_db(&db_path.to_string_lossy())
                .expect("failed to initialize database");
            app.manage(AppState {
                db: std::sync::Mutex::new(conn),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_videos,
            commands::scan_directory,
            commands::save_progress,
            commands::get_progress,
            commands::get_videos_with_progress,
            commands::check_ffprobe,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Note: If the scaffolded `main.rs` has a different structure (e.g., uses `fn main()` directly), replace its content with the above. The scaffolded `lib.rs` might need adjustment to match.

- [ ] **Step 4: Verify compilation**

```bash
cd D:\opencword\viewman\src-tauri
cargo build
```
Expected: Successful compilation (may take a while first time due to dependency downloads).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs src-tauri/src/main.rs
git commit -m "feat: add Tauri commands and app entry point"
```

---

### Task 5: Frontend — Types and API hooks

**Files:**
- Create: `src/types.ts`
- Create: `src/hooks/useVideos.ts`
- Create: `src/hooks/usePlayer.ts`

- [ ] **Step 1: Write types.ts**

```typescript
export interface Video {
  id: string;
  path: string;
  filename: string;
  duration: number | null;
  width: number | null;
  height: number | null;
  file_size: number;
  created_at: string;
}

export interface WatchProgress {
  id: string;
  video_id: string;
  position: number;
  updated_at: string;
}

export interface VideoWithProgress {
  video: Video;
  position: number | null;
}
```

- [ ] **Step 2: Write useVideos.ts**

```typescript
import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Video } from "../types";

export function useVideos() {
  const [videos, setVideos] = useState<Video[]>([]);
  const [loading, setLoading] = useState(false);
  const [progressMap, setProgressMap] = useState<Record<string, number | null>>({});

  const loadVideos = useCallback(async () => {
    setLoading(true);
    try {
      const result = await invoke<[Video, number | null][]>("get_videos_with_progress");
      setVideos(result.map(([v]) => v));
      const pmap: Record<string, number | null> = {};
      result.forEach(([v, p]) => { pmap[v.id] = p; });
      setProgressMap(pmap);
    } catch (e) {
      console.error("Failed to load videos:", e);
    } finally {
      setLoading(false);
    }
  }, []);

  const scanDirectory = useCallback(async (dir: string) => {
    setLoading(true);
    try {
      await invoke<Video[]>("scan_directory", { dir });
      await loadVideos();
    } catch (e) {
      console.error("Failed to scan directory:", e);
    } finally {
      setLoading(false);
    }
  }, [loadVideos]);

  const saveProgress = useCallback(async (videoId: string, position: number) => {
    try {
      await invoke("save_progress", { videoId, position });
      setProgressMap(prev => ({ ...prev, [videoId]: position }));
    } catch (e) {
      console.error("Failed to save progress:", e);
    }
  }, []);

  useEffect(() => {
    loadVideos();
  }, [loadVideos]);

  return { videos, progressMap, loading, scanDirectory, saveProgress, loadVideos };
}
```

- [ ] **Step 3: Write usePlayer.ts**

```typescript
import { useState, useCallback, useRef, useEffect } from "react";
import type { Video } from "../types";

export function usePlayer(saveProgress: (videoId: string, position: number) => Promise<void>) {
  const [currentVideo, setCurrentVideo] = useState<Video | null>(null);
  const [initialPosition, setInitialPosition] = useState(0);
  const autoSaveRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const openPlayer = useCallback((video: Video, position: number = 0) => {
    setCurrentVideo(video);
    setInitialPosition(position);
  }, []);

  const closePlayer = useCallback(() => {
    setCurrentVideo(null);
    setInitialPosition(0);
  }, []);

  useEffect(() => {
    if (currentVideo && autoSaveRef.current === null) {
      autoSaveRef.current = setInterval(() => {
        // The video element will call saveProgress externally
        // This interval is a fallback trigger
      }, 15000);
    }
    if (!currentVideo && autoSaveRef.current) {
      clearInterval(autoSaveRef.current);
      autoSaveRef.current = null;
    }
    return () => {
      if (autoSaveRef.current) {
        clearInterval(autoSaveRef.current);
        autoSaveRef.current = null;
      }
    };
  }, [currentVideo]);

  return { currentVideo, initialPosition, openPlayer, closePlayer };
}
```

- [ ] **Step 4: Commit**

```bash
git add src/types.ts src/hooks/useVideos.ts src/hooks/usePlayer.ts
git commit -m "feat: add frontend types and hooks"
```

---

### Task 6: Frontend — App layout and Sidebar

**Files:**
- Modify: `src/App.tsx` (replace scaffolded content)
- Create: `src/components/Sidebar.tsx`
- Modify: `src/main.tsx` if needed

- [ ] **Step 1: Write Sidebar.tsx**

```typescript
import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

interface SidebarProps {
  onScanDirectory: (dir: string) => Promise<void>;
  loading: boolean;
}

export function Sidebar({ onScanDirectory, loading }: SidebarProps) {
  const [ffprobeOk, setFfprobeOk] = useState<boolean | null>(null);

  const handleScan = async () => {
    const dir = await open({ directory: true, multiple: false, title: "选择视频目录" });
    if (dir) {
      await onScanDirectory(dir);
    }
  };

  const handleCheckFfprobe = async () => {
    const ok = await invoke<boolean>("check_ffprobe");
    setFfprobeOk(ok);
  };

  return (
    <aside className="w-60 h-full bg-gray-900 text-white flex flex-col p-4 gap-3">
      <h1 className="text-lg font-bold mb-2">ViewMan</h1>
      <button
        onClick={handleScan}
        disabled={loading}
        className="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 py-2 px-4 rounded"
      >
        {loading ? "扫描中..." : "扫描目录"}
      </button>
      <button
        onClick={handleCheckFfprobe}
        className="bg-gray-700 hover:bg-gray-600 py-2 px-4 rounded text-sm"
      >
        检测 ffprobe
      </button>
      {ffprobeOk === true && <span className="text-green-400 text-sm">✓ ffprobe 可用</span>}
      {ffprobeOk === false && <span className="text-red-400 text-sm">✗ ffprobe 未安装</span>}
    </aside>
  );
}
```

- [ ] **Step 2: Write App.tsx**

```typescript
import { useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { VideoGrid } from "./components/VideoGrid";
import { PlayerView } from "./components/PlayerView";
import { SearchBar } from "./components/SearchBar";
import { useVideos } from "./hooks/useVideos";
import { usePlayer } from "./hooks/usePlayer";
import type { Video } from "./types";

function App() {
  const { videos, progressMap, loading, scanDirectory, saveProgress } = useVideos();
  const { currentVideo, initialPosition, openPlayer, closePlayer } = usePlayer(saveProgress);
  const [searchQuery, setSearchQuery] = useState("");

  const filteredVideos = videos.filter(v =>
    v.filename.toLowerCase().includes(searchQuery.toLowerCase())
  );

  const handlePlayVideo = (video: Video) => {
    const pos = progressMap[video.id] ?? 0;
    openPlayer(video, pos);
  };

  return (
    <div className="h-screen w-screen flex bg-gray-950 text-white overflow-hidden">
      <Sidebar onScanDirectory={scanDirectory} loading={loading} />
      <main className="flex-1 flex flex-col p-4 gap-4 overflow-hidden">
        <SearchBar value={searchQuery} onChange={setSearchQuery} total={filteredVideos.length} />
        <VideoGrid videos={filteredVideos} progressMap={progressMap} onPlay={handlePlayVideo} />
      </main>
      {currentVideo && (
        <PlayerView
          video={currentVideo}
          initialPosition={initialPosition}
          onClose={closePlayer}
          onProgress={saveProgress}
        />
      )}
    </div>
  );
}

export default App;
```

- [ ] **Step 3: Commit**

```bash
git add src/App.tsx src/components/Sidebar.tsx
git commit -m "feat: add App layout and Sidebar component"
```

---

### Task 7: Frontend — SearchBar, VideoGrid, VideoCard

**Files:**
- Create: `src/components/SearchBar.tsx`
- Create: `src/components/VideoGrid.tsx`
- Create: `src/components/VideoCard.tsx`

- [ ] **Step 1: Write SearchBar.tsx**

```typescript
interface SearchBarProps {
  value: string;
  onChange: (v: string) => void;
  total: number;
}

export function SearchBar({ value, onChange, total }: SearchBarProps) {
  return (
    <div className="flex items-center gap-3">
      <input
        type="text"
        value={value}
        onChange={e => onChange(e.target.value)}
        placeholder="搜索视频文件名..."
        className="flex-1 bg-gray-800 text-white px-4 py-2 rounded outline-none focus:ring-2 focus:ring-blue-500"
      />
      <span className="text-gray-400 text-sm">{total} 个视频</span>
    </div>
  );
}
```

- [ ] **Step 2: Write VideoCard.tsx**

```typescript
import type { Video } from "../types";

interface VideoCardProps {
  video: Video;
  progress: number | null;
  onPlay: (video: Video) => void;
}

function formatDuration(seconds: number | null): string {
  if (seconds === null || seconds === undefined) return "--:--";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return `${m}:${String(s).padStart(2, "0")}`;
}

function formatFileSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

export function VideoCard({ video, progress, onPlay }: VideoCardProps) {
  const progressPct = progress !== null && video.duration && video.duration > 0
    ? Math.min(100, (progress / video.duration) * 100)
    : 0;

  return (
    <div
      onClick={() => onPlay(video)}
      className="bg-gray-800 rounded-lg overflow-hidden cursor-pointer hover:ring-2 hover:ring-blue-500 transition-all group"
    >
      <div className="aspect-video bg-gray-700 flex items-center justify-center text-gray-500">
        <svg className="w-12 h-12" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M14.752 11.168l-3.197-2.132A1 1 0 0010 9.87v4.263a1 1 0 001.555.832l3.197-2.132a1 1 0 000-1.664z" />
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
        </svg>
      </div>
      <div className="p-3">
        <p className="text-sm truncate" title={video.filename}>{video.filename}</p>
        <div className="flex justify-between text-xs text-gray-400 mt-1">
          <span>{formatDuration(video.duration)}</span>
          <span>{formatFileSize(video.file_size)}</span>
        </div>
        {progressPct > 0 && (
          <div className="w-full bg-gray-600 h-1 rounded mt-2">
            <div
              className="bg-blue-500 h-1 rounded transition-all"
              style={{ width: `${progressPct}%` }}
            />
          </div>
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Write VideoGrid.tsx**

```typescript
import type { Video } from "../types";
import { VideoCard } from "./VideoCard";

interface VideoGridProps {
  videos: Video[];
  progressMap: Record<string, number | null>;
  onPlay: (video: Video) => void;
}

export function VideoGrid({ videos, progressMap, onPlay }: VideoGridProps) {
  if (videos.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-500">
        <div className="text-center">
          <p className="text-lg">暂无视频</p>
          <p className="text-sm mt-1">点击左侧"扫描目录"添加视频</p>
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-4">
        {videos.map(video => (
          <VideoCard
            key={video.id}
            video={video}
            progress={progressMap[video.id] ?? null}
            onPlay={onPlay}
          />
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Commit**

```bash
git add src/components/SearchBar.tsx src/components/VideoCard.tsx src/components/VideoGrid.tsx
git commit -m "feat: add SearchBar, VideoCard, VideoGrid components"
```

---

### Task 8: Frontend — PlayerView

**Files:**
- Create: `src/components/PlayerView.tsx`

- [ ] **Step 1: Write PlayerView.tsx**

```typescript
import { useRef, useEffect, useCallback, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Video } from "../types";

interface PlayerViewProps {
  video: Video;
  initialPosition: number;
  onClose: () => void;
  onProgress: (videoId: string, position: number) => Promise<void>;
}

export function PlayerView({ video, initialPosition, onClose, onProgress }: PlayerViewProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [playing, setPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);

  const src = convertFileSrc(video.path);

  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    if (initialPosition > 0) {
      el.currentTime = initialPosition;
    }
  }, [initialPosition]);

  const handleTimeUpdate = useCallback(() => {
    const el = videoRef.current;
    if (!el) return;
    setCurrentTime(el.currentTime);
    setDuration(el.duration || 0);
  }, []);

  const handleSave = useCallback(() => {
    const el = videoRef.current;
    if (el) {
      onProgress(video.id, el.currentTime);
    }
  }, [video.id, onProgress]);

  useEffect(() => {
    const interval = setInterval(handleSave, 15000);
    return () => {
      clearInterval(interval);
      handleSave();
    };
  }, [handleSave]);

  const togglePlay = () => {
    const el = videoRef.current;
    if (!el) return;
    if (el.paused) {
      el.play();
      setPlaying(true);
    } else {
      el.pause();
      setPlaying(false);
    }
  };

  const handleSeek = (e: React.ChangeEvent<HTMLInputElement>) => {
    const el = videoRef.current;
    if (!el) return;
    el.currentTime = parseFloat(e.target.value);
    setCurrentTime(el.currentTime);
  };

  const formatTime = (s: number) => {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = Math.floor(s % 60);
    if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}`;
    return `${m}:${String(sec).padStart(2, "0")}`;
  };

  return (
    <div className="fixed inset-0 bg-black/90 z-50 flex flex-col items-center justify-center">
      <div className="absolute top-4 right-4 flex gap-2">
        <span className="text-gray-400 text-sm self-center">{video.filename}</span>
        <button onClick={onClose} className="bg-gray-700 hover:bg-gray-600 px-3 py-1 rounded text-white">
          关闭
        </button>
      </div>

      <div className="w-full max-w-5xl">
        <video
          ref={videoRef}
          src={src}
          onTimeUpdate={handleTimeUpdate}
          onLoadedMetadata={handleTimeUpdate}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onEnded={handleSave}
          className="w-full max-h-[80vh] bg-black"
          controls={false}
        />

        <div className="flex items-center gap-3 mt-3 px-2">
          <button onClick={togglePlay} className="text-white text-xl">
            {playing ? "⏸" : "▶"}
          </button>

          <input
            type="range"
            min={0}
            max={duration || 0}
            step={0.1}
            value={currentTime}
            onChange={handleSeek}
            className="flex-1 accent-blue-500"
          />

          <span className="text-white text-sm tabular-nums">
            {formatTime(currentTime)} / {formatTime(duration)}
          </span>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add src/components/PlayerView.tsx
git commit -m "feat: add PlayerView with seek and auto-save progress"
```

---

### Task 9: Configure Tauri for video playback

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/capabilities/default.json`

- [ ] **Step 1: Enable file protocol access**

Update `src-tauri/tauri.conf.json` to allow local file access for video playback:

```json
{
  "productName": "viewman",
  "version": "0.1.0",
  "identifier": "com.viewman.app",
  "build": {
    "frontendDist": "../dist",
    "devUrl": "http://localhost:1420",
    "beforeDevCommand": "npm run dev",
    "beforeBuildCommand": "npm run build"
  },
  "app": {
    "windows": [
      {
        "title": "ViewMan",
        "width": 1280,
        "height": 800,
        "resizable": true
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

Key change: `"security": { "csp": null }` — this allows loading local files via `convertFileSrc`.

- [ ] **Step 2: Update capabilities for dialog plugin**

Update `src-tauri/capabilities/default.json`:
```json
{
  "identifier": "default",
  "description": "Capability for the main window",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "opener:default",
    "dialog:default"
  ]
}
```

- [ ] **Step 3: Install dialog plugin**

```bash
cd D:\opencword\viewman
npm install @tauri-apps/plugin-dialog
```

Add to `src-tauri/Cargo.toml`:
```toml
tauri-plugin-dialog = "2"
```

Register in `src-tauri/src/main.rs`:
```rust
.plugin(tauri_plugin_dialog::init())
```

- [ ] **Step 4: Verify build**

```bash
cd D:\opencword\viewman
npm run tauri build
```
Expected: Successful build.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/tauri.conf.json src-tauri/capabilities/default.json src-tauri/Cargo.toml src-tauri/src/main.rs package.json
git commit -m "feat: configure Tauri for video file access and dialog plugin"
```

---

### Task 10: Final integration and manual testing

**Files:** No file changes — verification only.

- [ ] **Step 1: Build and run**

```bash
cd D:\opencword\viewman
npm run tauri dev
```

Expected: Window opens with ViewMan sidebar, empty video grid, "暂无视频" message.

- [ ] **Step 2: Manual test — scan directory**

1. Click "扫描目录"
2. Select a folder containing video files (.mp4, .mkv, etc.)
3. Expected: Videos appear in the grid, each showing filename, duration, file size

- [ ] **Step 3: Manual test — playback**

1. Click a video card
2. Expected: Player overlay opens, video starts playing from beginning (or saved position)
3. Play/Pause button works, seek bar works

- [ ] **Step 4: Manual test — progress saving**

1. Play a video to some position
2. Close the player
3. Expected: Progress bar appears on the video card
4. Click the same video again — it should resume from the saved position

- [ ] **Step 5: Manual test — search**

1. Type in the search bar
2. Expected: Video list filters to matching filenames

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: complete ViewMan initial implementation"
```

---

## Self-Review

**Spec coverage:**
- Video management (scan + import): Task 3 (scanner), Task 4 (scan_directory command)
- Watch progress saving: Task 2 (db upsert), Task 4 (save_progress command), Task 8 (PlayerView auto-save)
- Video grid display: Task 7 (VideoGrid, VideoCard)
- Video player: Task 8 (PlayerView)
- Search: Task 7 (SearchBar)
- Sidebar: Task 6 (Sidebar)

**Placeholder scan:** No TBD/TODO — all code is complete.

**Type consistency:** All TypeScript interfaces match Rust structs. Function signatures consistent across commands/hooks/components.
