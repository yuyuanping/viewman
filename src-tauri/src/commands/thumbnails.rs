use tauri::{Emitter, Manager, State};

use crate::db;
use crate::scanner;

use super::AppState;

#[derive(Clone, serde::Serialize)]
pub struct ThumbnailProgress {
    pub processed: usize,
    pub total: usize,
    pub done: bool,
    pub generated: usize,
    pub failed: usize,
}

/// 一张待生成封面的源文件：图片没有时长，duration 传 None 即走首帧=整图缩放
pub(crate) struct ThumbJob {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) duration: Option<f64>,
}

/// 缩略图缓存目录：`<app_data>/thumbnails`
pub(crate) fn thumbnails_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("thumbnails");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建缩略图目录失败: {}", e))?;
    Ok(dir)
}

/// 封面缓存文件名由条目 id 决定，条目移除后一并清理，避免留下孤儿文件
pub(crate) fn clear_thumbnail_cache(app: &tauri::AppHandle, id: &str) {
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join("thumbnails").join(format!("{}.jpg", id)));
    }
}

/// 逐张生成封面并立即写库。抽帧过程不持有数据库锁，中途关闭应用已完成的部分不丢。
/// 视频与图片共用这条通路，差异只在事件名与 `persist` 写的是哪张表。
pub(crate) async fn run_thumbnail_jobs(
    app: &tauri::AppHandle,
    jobs: Vec<ThumbJob>,
    event: &'static str,
    persist: fn(&rusqlite::Connection, &str, &str) -> rusqlite::Result<()>,
) -> Result<usize, String> {
    if !scanner::ffmpeg_available() {
        return Err("未检测到 ffmpeg，无法生成封面。请安装 ffmpeg 并加入 PATH。".into());
    }

    let total = jobs.len();
    if total == 0 {
        let _ = app.emit(
            event,
            ThumbnailProgress { processed: 0, total: 0, done: true, generated: 0, failed: 0 },
        );
        return Ok(0);
    }

    let dir = thumbnails_dir(app)?;
    let task_app = app.clone();
    let (generated, failed) = tauri::async_runtime::spawn_blocking(move || -> (usize, usize) {
        use tauri::Manager;
        let state = task_app.state::<AppState>();
        let mut generated = 0usize;
        let mut failed = 0usize;
        for (index, job) in jobs.iter().enumerate() {
            let out = dir.join(format!("{}.jpg", job.id));
            // 上次中途关闭留下的孤儿文件：直接登记，不重新抽帧
            let existing_ok = std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false);
            let ok = existing_ok || {
                // 抽帧写临时名，成功才改名——ffmpeg 中途被杀不会留下半张 jpg。
                // 临时名必须以 .jpg 结尾：ffmpeg 按扩展名选封装格式，`.jpg.part` 会直接失败。
                let part = dir.join(format!("{}.part.jpg", job.id));
                let result = scanner::extract_thumbnail(&job.source, &part, job.duration)
                    .and_then(|()| std::fs::rename(&part, &out).map_err(|e| e.to_string()));
                let _ = std::fs::remove_file(&part);
                result.is_ok()
            };
            if ok {
                if let Ok(conn) = state.db.lock() {
                    let _ = persist(&conn, &job.id, &out.to_string_lossy());
                }
                generated += 1;
            } else {
                failed += 1;
            }
            let _ = task_app.emit(
                event,
                ThumbnailProgress {
                    processed: index + 1,
                    total,
                    done: false,
                    generated,
                    failed,
                },
            );
        }
        (generated, failed)
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit(
        event,
        ThumbnailProgress { processed: total, total, done: true, generated, failed },
    );

    Ok(generated)
}

/// 为指定视频生成封面（ffmpeg 抽帧），已有有效缓存文件的会跳过
#[tauri::command]
pub async fn generate_thumbnails(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    video_ids: Vec<String>,
) -> Result<usize, String> {
    let jobs: Vec<ThumbJob> = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let all = db::get_all_videos(&conn).map_err(|e| e.to_string())?;
        all.into_iter()
            .filter(|v| video_ids.iter().any(|id| id == &v.id))
            // 缓存文件仍在的跳过；文件被删掉的会重新生成
            .filter(|v| !cached_thumbnail_usable(&v.thumbnail_path))
            .map(|v| ThumbJob { id: v.id, source: v.path, duration: v.duration })
            .collect()
    };

    run_thumbnail_jobs(&app, jobs, "thumbnail-progress", db::set_thumbnail).await
}

/// 缓存文件仍在磁盘上才算可用
pub(crate) fn cached_thumbnail_usable(path: &Option<String>) -> bool {
    matches!(path, Some(p) if std::path::Path::new(p).exists())
}

/// 播放器截图：截当前播放帧，存到视频同目录（<视频名>_<时间戳>.png），返回输出路径。
/// 用 ffmpeg seek 到指定秒抽 1 帧，原始分辨率不缩放
#[tauri::command]
pub async fn capture_frame(
    state: State<'_, AppState>,
    video_id: String,
    position: f64,
) -> Result<String, String> {
    let path = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_video_path(&conn, &video_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Video not found: {}", video_id))?
    };

    tauri::async_runtime::spawn_blocking(move || {
        let src = std::path::Path::new(&path);
        let parent = src.parent().ok_or("无法确定视频所在目录")?;
        let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "frame".into());
        let stamp = if position > 0.0 { format!("_{:02}m{:02}s", (position / 60.0).floor() as u32, (position % 60.0).round() as u32) } else { "_start".to_string() };
        let out = parent.join(format!("{}_{}.png", stem, stamp));

        let output = crate::scanner::hidden_command("ffmpeg")
            .args([
                "-y",
                "-ss", &position.max(0.0).to_string(),
                "-i", &path,
                "-frames:v", "1",
                &out.to_string_lossy(),
            ])
            .output()
            .map_err(|e| format!("无法运行 ffmpeg: {}", e))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(format!("截图失败: {}", detail.chars().take(200).collect::<String>()));
        }
        Ok(out.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
